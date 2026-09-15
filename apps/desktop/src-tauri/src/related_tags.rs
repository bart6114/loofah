use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tauri::Manager;
use tauri_plugin_settings::SettingsPluginExt;
use tauri_plugin_tantivy::TantivyPluginExt;

use crate::session_store::{
    SessionStore, StoreError, TagSuggestionItem, TagSuggestionStatus, is_tag_automation_candidate,
};

pub const ALGORITHM_VERSION: u32 = 3;
const CANDIDATE_LIMIT: usize = 50;
const SUGGESTION_LIMIT: usize = 3;
const SUGGESTION_THRESHOLD: f32 = 0.35;
const AUTO_ACCEPT_THRESHOLD: f32 = 0.75;
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
];
const NOTE_IDLE_DELAY: Duration = Duration::from_secs(10);
const NOTE_MAX_WAIT: Duration = Duration::from_secs(120);
const TRANSCRIPT_WEIGHT: f32 = 0.65;
const SUMMARY_WEIGHT: f32 = 0.25;
const NOTE_WEIGHT: f32 = 0.10;

enum QueueMessage {
    Process {
        session_id: String,
        revision: u64,
        attempt: usize,
    },
    DebounceElapsed {
        session_id: String,
        generation: u64,
    },
}

struct DebounceEntry {
    generation: u64,
    first_change: Instant,
}

#[derive(Default)]
struct DebounceState {
    next_generation: u64,
    entries: HashMap<String, DebounceEntry>,
    revisions: HashMap<String, u64>,
    // Attempt number and whether its queued message is still awaiting processing.
    attempts: HashMap<String, (usize, bool)>,
}

impl DebounceState {
    fn bump_revision(&mut self, session_id: &str) -> u64 {
        self.attempts.remove(session_id);
        let revision = self
            .revisions
            .get(session_id)
            .copied()
            .unwrap_or_default()
            .wrapping_add(1);
        self.revisions.insert(session_id.to_string(), revision);
        revision
    }

    fn claim_attempt(&mut self, session_id: &str, revision: u64, attempt: usize) -> bool {
        if self.revision(session_id) != revision {
            return false;
        }
        let entry = self
            .attempts
            .entry(session_id.to_string())
            .or_insert((0, true));
        if *entry != (attempt, true) {
            return false;
        }
        entry.1 = false;
        true
    }

    fn retry(&mut self, session_id: &str, revision: u64, attempt: usize) -> Option<Duration> {
        if self.revision(session_id) != revision {
            return None;
        }
        let delay = *RETRY_DELAYS.get(attempt)?;
        let entry = self.attempts.get_mut(session_id)?;
        if *entry != (attempt, false) {
            return None;
        }
        *entry = (attempt + 1, true);
        Some(delay)
    }

    fn schedule(
        &mut self,
        session_id: &str,
        now: Instant,
        idle_delay: Duration,
        max_wait: Duration,
    ) -> (u64, Duration) {
        self.bump_revision(session_id);
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        let first_change = self
            .entries
            .get(session_id)
            .map(|entry| entry.first_change)
            .unwrap_or(now);
        self.entries.insert(
            session_id.to_string(),
            DebounceEntry {
                generation,
                first_change,
            },
        );
        let deadline = (now + idle_delay).min(first_change + max_wait);
        (generation, deadline.saturating_duration_since(now))
    }

    fn take_if_current(&mut self, session_id: &str, generation: u64) -> bool {
        if self
            .entries
            .get(session_id)
            .is_some_and(|entry| entry.generation == generation)
        {
            self.entries.remove(session_id);
            true
        } else {
            false
        }
    }

    fn revision(&self, session_id: &str) -> u64 {
        self.revisions.get(session_id).copied().unwrap_or_default()
    }
}

#[derive(Clone)]
pub struct RelatedTagQueue {
    sender: tokio::sync::mpsc::UnboundedSender<QueueMessage>,
    debounce: Arc<Mutex<DebounceState>>,
}

impl RelatedTagQueue {
    pub fn enqueue(&self, session_id: String) {
        let mut state = self.debounce.lock().unwrap();
        let revision = state.bump_revision(&session_id);
        state.entries.remove(&session_id);
        drop(state);
        let _ = self.sender.send(QueueMessage::Process {
            session_id,
            revision,
            attempt: 0,
        });
    }

    pub fn note_changed(&self, session_id: String) {
        let (generation, delay) = self.debounce.lock().unwrap().schedule(
            &session_id,
            Instant::now(),
            NOTE_IDLE_DELAY,
            NOTE_MAX_WAIT,
        );
        let sender = self.sender.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = sender.send(QueueMessage::DebounceElapsed {
                session_id,
                generation,
            });
        });
    }

    fn revision(&self, session_id: &str) -> u64 {
        self.debounce.lock().unwrap().revision(session_id)
    }

    #[cfg(test)]
    pub(crate) fn new_test() -> Self {
        let (sender, _) = tokio::sync::mpsc::unbounded_channel();
        Self {
            sender,
            debounce: Arc::new(Mutex::new(DebounceState::default())),
        }
    }

    #[cfg(test)]
    pub(crate) fn has_debounced_change(&self, session_id: &str) -> bool {
        self.debounce
            .lock()
            .unwrap()
            .entries
            .contains_key(session_id)
    }
}

pub fn spawn<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    store: Arc<SessionStore>,
) -> RelatedTagQueue {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<QueueMessage>();
    let queue = RelatedTagQueue {
        sender,
        debounce: Arc::new(Mutex::new(DebounceState::default())),
    };
    let worker_queue = queue.clone();

    tauri::async_runtime::spawn(async move {
        if let Err(error) = app
            .state::<crate::startup::StartupState>()
            .wait_until_ready()
            .await
        {
            tracing::debug!(%error, "related tags: vault startup failed; worker stopped");
            return;
        }
        enqueue_pending(&store, &worker_queue);

        while let Some(message) = receiver.recv().await {
            if let Some((message, delay)) =
                handle_message(&app, &store, &worker_queue, message).await
            {
                let sender = worker_queue.sender.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let _ = sender.send(message);
                });
            }
        }
    });

    queue
}

fn enqueue_pending(store: &SessionStore, queue: &RelatedTagQueue) {
    for entry in store.session_list() {
        if entry
            .meta
            .tag_suggestions
            .as_ref()
            .is_some_and(|state| state.status == TagSuggestionStatus::Pending)
            && queue.revision(&entry.meta.id) == 0
        {
            queue.enqueue(entry.meta.id);
        }
    }
}

#[derive(Debug)]
enum ProcessingFailure {
    MissingSession,
    Retryable(String),
    Terminal(String),
}

impl From<StoreError> for ProcessingFailure {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Io(_) | StoreError::Conflict(_) => Self::Retryable(error.to_string()),
            StoreError::Serialize(_) => Self::Terminal(error.to_string()),
        }
    }
}

impl From<tauri_plugin_tantivy::Error> for ProcessingFailure {
    fn from(error: tauri_plugin_tantivy::Error) -> Self {
        use tantivy::TantivyError;
        use tantivy::directory::error::{
            LockError, OpenDirectoryError, OpenReadError, OpenWriteError,
        };
        use tauri_plugin_tantivy::Error as SearchError;
        if matches!(
            error,
            SearchError::Io(_)
                | SearchError::IndexNotInitialized
                | SearchError::CollectionNotFound(_)
                | SearchError::Tantivy(
                    TantivyError::IoError(_)
                        | TantivyError::OpenReadError(OpenReadError::IoError { .. })
                        | TantivyError::OpenWriteError(OpenWriteError::IoError { .. })
                        | TantivyError::OpenDirectoryError(
                            OpenDirectoryError::IoError { .. }
                                | OpenDirectoryError::FailedToCreateTempDir(_)
                        )
                        | TantivyError::LockFailure(LockError::IoError(_), _)
                )
        ) {
            Self::Retryable(error.to_string())
        } else {
            Self::Terminal(error.to_string())
        }
    }
}

async fn handle_message<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    store: &SessionStore,
    queue: &RelatedTagQueue,
    message: QueueMessage,
) -> Option<(QueueMessage, Duration)> {
    let (session_id, revision, attempt) = {
        let mut state = queue.debounce.lock().unwrap();
        let (session_id, revision, attempt) = match message {
            QueueMessage::Process {
                session_id,
                revision,
                attempt,
            } => (session_id, revision, attempt),
            QueueMessage::DebounceElapsed {
                session_id,
                generation,
            } => {
                if !state.take_if_current(&session_id, generation) {
                    return None;
                }
                let revision = state.revision(&session_id);
                (session_id, revision, 0)
            }
        };
        if !state.claim_attempt(&session_id, revision, attempt) {
            return None;
        }
        (session_id, revision, attempt)
    };
    let result = process(app, store, queue, &session_id, revision).await;
    if queue.revision(&session_id) != revision || store.session_get(&session_id).is_none() {
        return None;
    }
    match result {
        Ok(()) | Err(ProcessingFailure::MissingSession) => None,
        Err(ProcessingFailure::Terminal(error)) => {
            tracing::warn!(%session_id, %error, attempt = attempt + 1, "related tags: terminal analysis failure");
            None
        }
        Err(ProcessingFailure::Retryable(error)) => {
            let delay = {
                let mut state = queue.debounce.lock().unwrap();
                if state.revision(&session_id) != revision {
                    return None;
                }
                state.retry(&session_id, revision, attempt)
            };
            if let Some(delay) = delay {
                tracing::debug!(%session_id, %error, attempt = attempt + 2, delay_seconds = delay.as_secs(), "related tags: scheduling analysis retry");
                Some((
                    QueueMessage::Process {
                        session_id,
                        revision,
                        attempt: attempt + 1,
                    },
                    delay,
                ))
            } else {
                tracing::warn!(%session_id, %error, attempt = attempt + 1, "related tags: analysis retries exhausted");
                None
            }
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn session_queue_tag_suggestions<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<(), String> {
    if app
        .try_state::<Arc<SessionStore>>()
        .and_then(|store| store.session_get(&session_id))
        .is_none()
    {
        return Err(format!("session {session_id} does not exist"));
    }
    app.state::<RelatedTagQueue>().enqueue(session_id);
    Ok(())
}

async fn process<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    store: &SessionStore,
    queue: &RelatedTagQueue,
    session_id: &str,
    analysis_revision: u64,
) -> Result<(), ProcessingFailure> {
    let source = source_content(store, session_id).await?;
    if !store
        .mark_tag_suggestions_pending(session_id, source.hash.clone(), ALGORITHM_VERSION)
        .await
        .map_err(ProcessingFailure::from)?
    {
        return Ok(());
    }
    let current = store
        .session_get(session_id)
        .ok_or(ProcessingFailure::MissingSession)?;
    if term_frequencies(&source.combined).len() < 10 {
        let latest_source = source_content(store, session_id).await?;
        if queue.revision(session_id) != analysis_revision || latest_source.hash != source.hash {
            tracing::debug!(%session_id, "related tags: discarded stale analysis");
            return Ok(());
        }
        store
            .complete_tag_suggestions(
                session_id,
                &source.hash,
                ALGORITHM_VERSION,
                Vec::new(),
                None,
            )
            .await
            .map_err(ProcessingFailure::from)?;
        tracing::info!(%session_id, count = 0, "related tags: analysis complete");
        return Ok(());
    }
    let candidate_hits = app
        .tantivy()
        .related_documents(&source.combined, session_id, CANDIDATE_LIMIT)
        .await
        .map_err(ProcessingFailure::from)?;

    let mut candidates = Vec::new();
    for hit in candidate_hits {
        let Some(record) = store.session_get(&hit.id) else {
            continue;
        };
        if record.meta.tags.is_empty() {
            continue;
        }
        let candidate_source = match source_content(store, &hit.id).await {
            Ok(source) => source,
            Err(ProcessingFailure::MissingSession) => continue,
            Err(error) => return Err(error),
        };
        if !candidate_source.combined.is_empty() {
            candidates.push((record.meta.tags, candidate_source));
        }
    }

    let suggestions = rank_tags(&current.meta.tags, &source, candidates);
    let latest_source = source_content(store, session_id).await?;
    if queue.revision(session_id) != analysis_revision || latest_source.hash != source.hash {
        tracing::debug!(%session_id, "related tags: discarded stale analysis");
        return Ok(());
    }
    let auto_accept = app.settings().config().auto_accept_related_tags;
    let completed = store
        .complete_tag_suggestions(
            session_id,
            &source.hash,
            ALGORITHM_VERSION,
            suggestions.clone(),
            auto_accept.then_some(AUTO_ACCEPT_THRESHOLD),
        )
        .await
        .map_err(ProcessingFailure::from)?;

    if completed && auto_accept {
        for suggestion in suggestions
            .iter()
            .filter(|suggestion| suggestion.confidence >= AUTO_ACCEPT_THRESHOLD)
        {
            if let Err(error) = store.ensure_tag(&suggestion.name).await {
                tracing::warn!(tag = %suggestion.name, %error, "related tags: registry sync failed");
            }
        }
    }
    tracing::info!(%session_id, count = suggestions.len(), "related tags: analysis complete");
    Ok(())
}

#[derive(Clone, Default)]
struct SourceContent {
    transcript: String,
    summary: String,
    note: String,
    combined: String,
    hash: String,
}

async fn source_content(
    store: &SessionStore,
    session_id: &str,
) -> Result<SourceContent, ProcessingFailure> {
    source_content_with_transcripts(store, session_id, store.session_transcripts(session_id)).await
}

async fn source_content_with_transcripts(
    store: &SessionStore,
    session_id: &str,
    transcripts: impl std::future::Future<
        Output = Result<Vec<hypr_fs_format::TranscriptWithData>, StoreError>,
    >,
) -> Result<SourceContent, ProcessingFailure> {
    let record = store
        .session_get(session_id)
        .ok_or(ProcessingFailure::MissingSession)?;
    let result = transcripts.await;
    if store.session_get(session_id).is_none() {
        return Err(ProcessingFailure::MissingSession);
    }
    let transcripts = result.map_err(ProcessingFailure::from)?;
    let transcript = transcripts
        .iter()
        .flat_map(|transcript| transcript.words.iter())
        .map(|word| word.text.trim())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let note = record
        .note_markdown
        .as_deref()
        .map(crate::search_index::extract_plain_text)
        .unwrap_or_default();
    let summary = store
        .session_enhanced_docs(session_id)
        .iter()
        .map(|doc| crate::search_index::extract_plain_text(&doc.markdown))
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Ok(assemble_source_content(transcript, summary, note))
}

fn assemble_source_content(transcript: String, summary: String, note: String) -> SourceContent {
    let combined = [&transcript, &summary, &note]
        .into_iter()
        .map(|content| content.trim())
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut hasher = Sha256::new();
    for content in [&transcript, &summary, &note] {
        hasher.update((content.len() as u64).to_le_bytes());
        hasher.update(content.as_bytes());
    }
    let hash = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    SourceContent {
        transcript,
        summary,
        note,
        combined,
        hash,
    }
}

fn rank_tags(
    attached_tags: &[String],
    target: &SourceContent,
    candidates: Vec<(Vec<String>, SourceContent)>,
) -> Vec<TagSuggestionItem> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let target_terms = weighted_term_frequencies(target);
    if target_terms.len() < 10 {
        return Vec::new();
    }
    let candidate_terms: Vec<_> = candidates
        .iter()
        .map(|(_, source)| weighted_term_frequencies(source))
        .collect();
    let mut document_frequency = HashMap::<String, usize>::new();
    for terms in std::iter::once(&target_terms).chain(candidate_terms.iter()) {
        for term in terms.keys() {
            *document_frequency.entry(term.clone()).or_default() += 1;
        }
    }
    let document_count = candidate_terms.len() + 1;
    let attached: HashSet<_> = attached_tags.iter().cloned().collect();
    let mut evidence = HashMap::<String, Vec<f32>>::new();

    for ((tags, _), terms) in candidates.iter().zip(candidate_terms.iter()) {
        let similarity = cosine_tfidf(&target_terms, terms, &document_frequency, document_count);
        if similarity < 0.15 {
            continue;
        }
        for tag in tags {
            let Some(tag) = hypr_vault_read::normalize_tag_name(tag) else {
                continue;
            };
            if is_tag_automation_candidate(&tag) && !attached.contains(&tag) {
                evidence.entry(tag).or_default().push(similarity);
            }
        }
    }

    let mut suggestions: Vec<TagSuggestionItem> = evidence
        .into_iter()
        .filter_map(|(name, similarities)| {
            let strongest = similarities.iter().copied().fold(0.0_f32, f32::max);
            let mut confidence = 1.0
                - similarities
                    .iter()
                    .fold(1.0_f32, |remaining, score| remaining * (1.0 - score));
            if similarities.len() == 1 && strongest < 0.9 {
                confidence = confidence.min(AUTO_ACCEPT_THRESHOLD - 0.01);
            }
            (confidence >= SUGGESTION_THRESHOLD).then_some(TagSuggestionItem { name, confidence })
        })
        .collect();
    suggestions.sort_by(|left, right| {
        right
            .confidence
            .total_cmp(&left.confidence)
            .then_with(|| left.name.cmp(&right.name))
    });
    suggestions.truncate(SUGGESTION_LIMIT);
    suggestions
}

fn term_frequencies(text: &str) -> HashMap<String, usize> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter_map(|term| {
            let term = term.to_lowercase();
            (term.chars().count() >= 3 && !STOP_WORDS.contains(&term.as_str())).then_some(term)
        })
        .fold(HashMap::new(), |mut frequencies, term| {
            *frequencies.entry(term).or_default() += 1;
            frequencies
        })
}

fn weighted_term_frequencies(source: &SourceContent) -> HashMap<String, f32> {
    let mut weighted = HashMap::new();
    for (content, source_weight) in [
        (&source.transcript, TRANSCRIPT_WEIGHT),
        (&source.summary, SUMMARY_WEIGHT),
        (&source.note, NOTE_WEIGHT),
    ] {
        for (term, frequency) in term_frequencies(content) {
            let frequency_weight = 1.0 + (frequency as f32).ln();
            *weighted.entry(term).or_default() += source_weight * frequency_weight;
        }
    }
    weighted
}

fn cosine_tfidf(
    left: &HashMap<String, f32>,
    right: &HashMap<String, f32>,
    document_frequency: &HashMap<String, usize>,
    document_count: usize,
) -> f32 {
    let weight = |term: &str, frequency: f32| {
        let df = document_frequency.get(term).copied().unwrap_or(0) as f32;
        let idf = ((document_count as f32 + 1.0) / (df + 1.0)).ln() + 1.0;
        frequency * idf
    };
    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;
    for (term, frequency) in left {
        let left_weight = weight(term, *frequency);
        left_norm += left_weight * left_weight;
        if let Some(right_frequency) = right.get(term) {
            dot += left_weight * weight(term, *right_frequency);
        }
    }
    for (term, frequency) in right {
        let right_weight = weight(term, *frequency);
        right_norm += right_weight * right_weight;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        return 0.0;
    }
    dot / (left_norm.sqrt() * right_norm.sqrt())
}

const STOP_WORDS: &[&str] = &[
    "and", "are", "but", "for", "from", "have", "that", "the", "their", "this", "was", "were",
    "will", "with", "you", "your",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn source(transcript: &str, summary: &str, note: &str) -> SourceContent {
        assemble_source_content(
            transcript.to_string(),
            summary.to_string(),
            note.to_string(),
        )
    }

    async fn retry_harness() -> (
        tempfile::TempDir,
        SessionStore,
        tauri::App<tauri::test::MockRuntime>,
        RelatedTagQueue,
        tokio::sync::mpsc::UnboundedReceiver<QueueMessage>,
    ) {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(tauri_plugin_tantivy::IndexState::default());
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let queue = RelatedTagQueue {
            sender,
            debounce: Arc::new(Mutex::new(DebounceState::default())),
        };
        (vault, store, app, queue, receiver)
    }

    async fn create_target(store: &SessionStore, id: &str) {
        let meta = serde_json::from_value(serde_json::json!({
            "id": id, "title": "Target", "created_at": "2026-09-15T00:00:00Z", "tags": []
        }))
        .unwrap();
        store.write_meta(&meta).await.unwrap();
        store
            .write_note(
                id,
                "one two three four five six seven eight nine ten eleven twelve",
            )
            .await
            .unwrap();
    }

    #[test]
    fn typed_failure_classification() {
        assert!(matches!(
            ProcessingFailure::from(StoreError::Io("x".into())),
            ProcessingFailure::Retryable(_)
        ));
        assert!(matches!(
            ProcessingFailure::from(StoreError::Conflict("x".into())),
            ProcessingFailure::Retryable(_)
        ));
        assert!(matches!(
            ProcessingFailure::from(StoreError::Serialize("x".into())),
            ProcessingFailure::Terminal(_)
        ));
        use tauri_plugin_tantivy::Error;
        for error in [
            Error::IndexNotInitialized,
            Error::CollectionNotFound("default".into()),
            Error::Io(std::io::Error::other("x")),
            Error::Tantivy(tantivy::TantivyError::IoError(Arc::new(
                std::io::Error::other("x"),
            ))),
        ] {
            assert!(matches!(
                ProcessingFailure::from(error),
                ProcessingFailure::Retryable(_)
            ));
        }
        for error in [
            Error::DocumentNotFound("x".into()),
            Error::InvalidDocumentType("x".into()),
            Error::Tantivy(tantivy::TantivyError::InvalidArgument("x".into())),
        ] {
            assert!(matches!(
                ProcessingFailure::from(error),
                ProcessingFailure::Terminal(_)
            ));
        }
    }

    #[tokio::test]
    async fn retries_stop_after_four_failures_and_pending_survives_restart() {
        let (vault, store, app, queue, mut receiver) = retry_harness().await;
        create_target(&store, "s1").await;
        queue.enqueue("s1".into());
        let mut message = receiver.try_recv().unwrap();
        for delay in RETRY_DELAYS {
            let (retry, actual) = handle_message(app.handle(), &store, &queue, message)
                .await
                .unwrap();
            assert_eq!(actual, delay);
            message = retry;
        }
        assert!(
            handle_message(app.handle(), &store, &queue, message)
                .await
                .is_none()
        );
        assert_eq!(
            store
                .session_get("s1")
                .unwrap()
                .meta
                .tag_suggestions
                .unwrap()
                .status,
            TagSuggestionStatus::Pending
        );
        assert!(!queue.debounce.lock().unwrap().claim_attempt("s1", 0, 0));

        let restarted = SessionStore::new(vault.path().to_path_buf());
        restarted.rebuild_index().await.unwrap();
        let (_, _, _, fresh_queue, mut fresh_receiver) = retry_harness().await;
        enqueue_pending(&restarted, &fresh_queue);
        let message = fresh_receiver.try_recv().unwrap();
        let (_, delay) = handle_message(app.handle(), &restarted, &fresh_queue, message)
            .await
            .unwrap();
        assert_eq!(delay, RETRY_DELAYS[0]);
    }

    #[tokio::test]
    async fn deletion_before_processing_or_during_backoff_cancels_without_recreation() {
        for during_backoff in [false, true] {
            let (vault, store, app, queue, mut receiver) = retry_harness().await;
            create_target(&store, "s1").await;
            let dir = store.session_dir("s1").await.unwrap();
            queue.enqueue("s1".into());
            let mut message = receiver.try_recv().unwrap();
            if during_backoff {
                message = handle_message(app.handle(), &store, &queue, message)
                    .await
                    .unwrap()
                    .0;
            }
            store.delete_session("s1").await.unwrap();
            assert!(
                handle_message(app.handle(), &store, &queue, message)
                    .await
                    .is_none()
            );
            assert!(store.session_get("s1").is_none());
            assert!(!vault.path().join(dir).exists());
        }
    }

    #[tokio::test]
    async fn transient_failure_recovers_and_terminal_failure_does_not_retry() {
        let (vault, store, app, queue, mut receiver) = retry_harness().await;
        create_target(&store, "s1").await;
        store.write_note("s1", "short note").await.unwrap();
        let transcript_path = vault
            .path()
            .join(store.session_dir("s1").await.unwrap())
            .join("transcript.json");
        std::fs::create_dir(&transcript_path).unwrap();
        queue.enqueue("s1".into());
        let retry = handle_message(app.handle(), &store, &queue, receiver.try_recv().unwrap())
            .await
            .unwrap()
            .0;
        std::fs::remove_dir(transcript_path).unwrap();
        assert!(
            handle_message(app.handle(), &store, &queue, retry)
                .await
                .is_none()
        );
        assert_eq!(
            store
                .session_get("s1")
                .unwrap()
                .meta
                .tag_suggestions
                .unwrap()
                .status,
            TagSuggestionStatus::Complete
        );

        std::fs::write(
            vault
                .path()
                .join(store.session_dir("s1").await.unwrap())
                .join("transcript.json"),
            "{broken",
        )
        .unwrap();
        queue.enqueue("s1".into());
        assert!(
            handle_message(app.handle(), &store, &queue, receiver.try_recv().unwrap())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn edits_invalidate_old_timers_and_sessions_have_independent_budgets() {
        let (_vault, store, app, queue, mut receiver) = retry_harness().await;
        for id in ["s1", "s2"] {
            create_target(&store, id).await;
        }
        queue.enqueue("s1".into());
        let old_retry = handle_message(app.handle(), &store, &queue, receiver.try_recv().unwrap())
            .await
            .unwrap()
            .0;
        let (generation, _) = queue.debounce.lock().unwrap().schedule(
            "s1",
            Instant::now(),
            NOTE_IDLE_DELAY,
            NOTE_MAX_WAIT,
        );
        assert!(
            handle_message(app.handle(), &store, &queue, old_retry)
                .await
                .is_none()
        );
        let (_, delay) = handle_message(
            app.handle(),
            &store,
            &queue,
            QueueMessage::DebounceElapsed {
                session_id: "s1".into(),
                generation,
            },
        )
        .await
        .unwrap();
        assert_eq!(delay, RETRY_DELAYS[0]);
        queue.enqueue("s2".into());
        let (_, delay) = handle_message(app.handle(), &store, &queue, receiver.try_recv().unwrap())
            .await
            .unwrap();
        assert_eq!(delay, RETRY_DELAYS[0]);
        queue.enqueue("s1".into());
        let (_, delay) = handle_message(app.handle(), &store, &queue, receiver.try_recv().unwrap())
            .await
            .unwrap();
        assert_eq!(delay, RETRY_DELAYS[0]);
    }

    #[tokio::test]
    async fn deletion_during_source_read_cancels_even_when_the_read_fails() {
        for failed in [false, true] {
            let (_vault, store, _app, _queue, _receiver) = retry_harness().await;
            create_target(&store, "candidate").await;
            let read = async {
                store.delete_session("candidate").await.unwrap();
                if failed {
                    Err(StoreError::Io("file disappeared".into()))
                } else {
                    Ok(Vec::new())
                }
            };
            let result = source_content_with_transcripts(&store, "candidate", read).await;
            assert!(matches!(result, Err(ProcessingFailure::MissingSession)));
        }
    }

    #[test]
    fn only_one_timer_can_be_scheduled_or_claimed_for_an_attempt() {
        let mut state = DebounceState::default();
        let revision = state.bump_revision("s1");
        assert!(state.claim_attempt("s1", revision, 0));
        assert!(!state.claim_attempt("s1", revision, 0));
        assert_eq!(state.retry("s1", revision, 0), Some(RETRY_DELAYS[0]));
        assert_eq!(state.retry("s1", revision, 0), None);
        assert!(state.claim_attempt("s1", revision, 1));
        assert!(!state.claim_attempt("s1", revision, 1));
    }

    #[test]
    fn recurring_topic_transfers_existing_tags() {
        let suggestions = rank_tags(
            &[],
            &source(
                "atlas launch rollout customer acme migration timeline launch rollout readiness deployment milestones ownership support",
                "",
                "",
            ),
            vec![
                (
                    vec!["project/atlas".to_string(), "customer/acme".to_string()],
                    source(
                        "atlas launch rollout customer acme migration timeline rollout readiness deployment milestones ownership support",
                        "",
                        "",
                    ),
                ),
                (
                    vec!["project/atlas".to_string()],
                    source(
                        "atlas rollout launch readiness migration customer acme timeline deployment milestones ownership support",
                        "",
                        "",
                    ),
                ),
                (
                    vec!["hiring".to_string()],
                    source(
                        "candidate interview frontend engineering feedback unrelated topic",
                        "",
                        "",
                    ),
                ),
            ],
        );

        assert_eq!(suggestions[0].name, "project/atlas");
        assert!(suggestions.iter().any(|item| item.name == "customer/acme"));
        assert!(!suggestions.iter().any(|item| item.name == "hiring"));
    }

    #[test]
    fn attached_and_unrelated_tags_are_omitted() {
        let suggestions = rank_tags(
            &["project/atlas".to_string()],
            &source(
                "atlas launch rollout customer acme migration timeline launch rollout readiness deployment milestones ownership support",
                "",
                "",
            ),
            vec![(
                vec!["project/atlas".to_string(), "hiring".to_string()],
                source(
                    "candidate interview frontend engineering feedback unrelated topic",
                    "",
                    "",
                ),
            )],
        );

        assert!(suggestions.is_empty());
    }

    #[test]
    fn tags_containing_import_are_omitted() {
        let target = source(
            "atlas launch rollout customer acme migration timeline readiness deployment milestones ownership support",
            "",
            "",
        );
        let candidates = (0..2)
            .map(|_| {
                (
                    vec![
                        "import".to_string(),
                        "important".to_string(),
                        "project/import-review".to_string(),
                        "customer/acme".to_string(),
                    ],
                    target.clone(),
                )
            })
            .collect();

        let suggestions = rank_tags(&[], &target, candidates);

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "customer/acme");
    }

    #[test]
    fn note_and_summary_terms_match_transcript_terms() {
        let suggestions = rank_tags(
            &[],
            &source(
                "",
                "Atlas deployment readiness and customer migration milestones",
                "Acme rollout owners need training support escalation timeline confirmation",
            ),
            vec![(
                vec!["project/atlas".to_string()],
                source(
                    "Atlas rollout for Acme customer deployment readiness migration milestones owners training support escalation timeline confirmation",
                    "",
                    "",
                ),
            )],
        );

        assert_eq!(suggestions[0].name, "project/atlas");
    }

    #[test]
    fn source_hash_changes_for_each_content_kind() {
        let baseline = source("atlas transcript", "atlas summary", "atlas note");
        assert_ne!(
            baseline.hash,
            source("changed transcript", "atlas summary", "atlas note").hash
        );
        assert_ne!(
            baseline.hash,
            source("atlas transcript", "changed summary", "atlas note").hash
        );
        assert_ne!(
            baseline.hash,
            source("atlas transcript", "atlas summary", "changed note").hash
        );
        assert_eq!(
            baseline.hash,
            source("atlas transcript", "atlas summary", "atlas note").hash
        );
    }

    #[test]
    fn note_changes_debounce_until_idle_but_respect_max_wait() {
        let start = Instant::now();
        let mut state = DebounceState::default();
        let (first_generation, first_delay) = state.schedule(
            "s1",
            start,
            Duration::from_secs(10),
            Duration::from_secs(30),
        );
        assert_eq!(first_delay, Duration::from_secs(10));
        assert_eq!(state.revision("s1"), 1);

        let (second_generation, second_delay) = state.schedule(
            "s1",
            start + Duration::from_secs(8),
            Duration::from_secs(10),
            Duration::from_secs(30),
        );
        assert_eq!(second_delay, Duration::from_secs(10));
        assert_eq!(state.revision("s1"), 2);
        assert!(!state.take_if_current("s1", first_generation));

        let (third_generation, third_delay) = state.schedule(
            "s1",
            start + Duration::from_secs(25),
            Duration::from_secs(10),
            Duration::from_secs(30),
        );
        assert_eq!(third_delay, Duration::from_secs(5));
        assert_eq!(state.revision("s1"), 3);
        assert!(!state.take_if_current("s1", second_generation));
        assert!(state.take_if_current("s1", third_generation));
    }
}

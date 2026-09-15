//! Tantivy search projection (Phase F): rides the in-memory vault index and its
//! change bus. No SQL anywhere in this worker.
//!
//! Dirty tracking: `SessionStore::subscribe_index_changes` fans the store's raw
//! change stream (the same tuples the `index-changed` dispatcher coalesces) into
//! this worker, which folds sessions/docs/transcripts changes into an in-memory
//! `DirtyQueue` keyed by session id. The queue keeps the retired SQL dirty table's
//! acknowledge-by-generation semantics: a session re-dirtied while its document is
//! being indexed carries a bumped generation, so the stale acknowledgement leaves it
//! queued for another pass.
//!
//! Durable state: the retired SQL projection row only carried
//! `projection_version`; that now lives as a `projection_version` file next to the
//! Tantivy files in the app-data `search_index/` dir (alongside the plugin's own
//! `schema_version` file). Deleting `search_index/` stays safe: a missing file
//! reads as version 0 and forces a full rebuild from the vault index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::DateTime;
use serde_json::Value;
use tauri::Manager;
use tauri_plugin_tantivy::{
    SearchDocument, SearchFilters, SearchOptions, SearchRequest, TantivyPluginExt,
};

use crate::session_store::{IndexEntity, SessionStore};

// Increment when the vault-index-to-Tantivy document shape changes so existing
// indexes are rebuilt. 6 -> 7 broadens related-tag candidate retrieval from
// transcript-only text to transcript, summary, and note content.
const PROJECTION_VERSION: i64 = 7;
const BATCH_SIZE: usize = 8;
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// Must match the tantivy plugin's `CollectionConfig.path` for the default
/// collection: the version file lives inside the same rebuildable cache dir.
const INDEX_DIR_NAME: &str = "search_index";

type WorkerResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
type ChangeReceiver = tokio::sync::mpsc::UnboundedReceiver<(IndexEntity, Vec<String>)>;

enum IndexAction {
    Upsert(SearchDocument),
    Remove(String),
}

/// In-memory replacement for the retired SQL dirty-queue table: a FIFO of dirty
/// session ids (front = oldest mark; re-marking moves an id to the back) with a
/// per-session generation that bumps on every mark. `acknowledge` only clears an
/// entry whose generation still matches the one handed out at batch time, so a
/// session re-dirtied mid-index stays queued.
#[derive(Default)]
struct DirtyQueue {
    inner: Mutex<QueueInner>,
}

#[derive(Default)]
struct QueueInner {
    order: Vec<String>,
    generations: HashMap<String, i64>,
}

impl DirtyQueue {
    fn mark(&self, id: &str) {
        let mut queue = self.inner.lock().unwrap();
        if let Some(generation) = queue.generations.get_mut(id) {
            *generation += 1;
            queue.order.retain(|queued| queued != id);
        } else {
            queue.generations.insert(id.to_string(), 1);
        }
        queue.order.push(id.to_string());
    }

    fn batch(&self, limit: usize) -> Vec<(String, i64)> {
        let queue = self.inner.lock().unwrap();
        queue
            .order
            .iter()
            .take(limit)
            .map(|id| (id.clone(), queue.generations[id]))
            .collect()
    }

    fn acknowledge(&self, id: &str, generation: i64) {
        let mut queue = self.inner.lock().unwrap();
        if queue.generations.get(id) == Some(&generation) {
            queue.generations.remove(id);
            queue.order.retain(|queued| queued != id);
        }
    }

    fn len(&self) -> usize {
        self.inner.lock().unwrap().generations.len()
    }
}

/// Call after the store is managed but BEFORE the startup `rebuild_index`: the
/// subscription taken here must observe the rebuild's changes, or edits made while
/// the app was closed would never reach the search index (the count guard only
/// catches added/removed sessions, not content edits).
pub fn spawn(app: tauri::AppHandle, store: Arc<SessionStore>) {
    use tauri_plugin_settings::SettingsPluginExt;

    let index_dir = match app.settings().global_base() {
        Ok(base) => base.join(INDEX_DIR_NAME).into_std_path_buf(),
        Err(error) => {
            tracing::error!(%error, "search projection: cannot resolve app-data dir; search indexing is disabled");
            return;
        }
    };
    let changes = store.subscribe_index_changes();

    tauri::async_runtime::spawn(async move {
        run(app, store, changes, index_dir).await;
    });
}

async fn run<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    store: Arc<SessionStore>,
    mut changes: ChangeReceiver,
    index_dir: PathBuf,
) {
    let queue = DirtyQueue::default();

    if let Err(error) = app
        .state::<crate::startup::StartupState>()
        .wait_until_ready()
        .await
    {
        tracing::warn!(%error, "search projection stopped because vault startup failed");
        return;
    }
    wait_for_tantivy(&app).await;

    loop {
        match initialize(&app, &store, &queue, &mut changes, &index_dir).await {
            Ok(()) => break,
            Err(error) => {
                tracing::error!(%error, "failed to initialize search index projection");
                tokio::time::sleep(RETRY_INTERVAL).await;
            }
        }
    }

    loop {
        if let Err(error) = drain_queue(&app, &store, &queue, &mut changes).await {
            tracing::error!(%error, "failed to update search index projection");
        }

        tokio::select! {
            change = changes.recv() => {
                match change {
                    Some((entity, ids)) => mark_dirty_sessions(&queue, entity, ids),
                    // Store dropped -- nothing can change anymore.
                    None => break,
                }
            }
            _ = tokio::time::sleep(RETRY_INTERVAL) => {}
        }
    }
}

/// Only these three entities feed the session search document (tasks/templates are
/// not indexed); their change ids are session ids.
fn mark_dirty_sessions(queue: &DirtyQueue, entity: IndexEntity, ids: Vec<String>) {
    if matches!(
        entity,
        IndexEntity::Sessions | IndexEntity::Docs | IndexEntity::Transcripts
    ) {
        for id in ids {
            queue.mark(&id);
        }
    }
}

fn absorb_pending_changes(queue: &DirtyQueue, changes: &mut ChangeReceiver) {
    while let Ok((entity, ids)) = changes.try_recv() {
        mark_dirty_sessions(queue, entity, ids);
    }
}

async fn wait_for_tantivy<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    loop {
        match index_document_count(app).await {
            Ok(_) => return,
            Err(tauri_plugin_tantivy::Error::CollectionNotFound(_)) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => {
                tracing::warn!(%error, "search index is not ready");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn initialize<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    store: &SessionStore,
    queue: &DirtyQueue,
    changes: &mut ChangeReceiver,
    index_dir: &Path,
) -> WorkerResult<()> {
    app.state::<crate::startup::StartupState>()
        .wait_until_ready()
        .await?;
    if read_projection_version(index_dir) != PROJECTION_VERSION {
        return rebuild(app, store, queue, changes, index_dir).await;
    }

    loop {
        drain_queue(app, store, queue, changes).await?;
        let session_count = store.session_count();
        let index_count_matches = wait_for_index_count(app, session_count).await?;
        let index_count = index_document_count(app).await?;

        // Writes can land while Tantivy's reader settles. Reconcile those before
        // interpreting a count mismatch as damage to the persisted projection.
        absorb_pending_changes(queue, changes);
        if store.session_count() != session_count || queue.len() > 0 {
            continue;
        }
        if !index_count_matches && index_count != session_count {
            tracing::info!(
                session_count,
                index_count,
                "search index count does not match the vault index; rebuilding projection"
            );
            rebuild(app, store, queue, changes, index_dir).await?;
        }
        break;
    }

    Ok(())
}

async fn rebuild<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    store: &SessionStore,
    queue: &DirtyQueue,
    changes: &mut ChangeReceiver,
    index_dir: &Path,
) -> WorkerResult<()> {
    // Durably drop to version 0 before touching the index, so a crash mid-rebuild
    // forces another full rebuild on the next boot (what resetting the retired
    // SQL projection row's version used to guarantee).
    write_projection_version(index_dir, 0)?;

    app.tantivy().reindex(None).await?;
    for id in store.session_ids() {
        queue.mark(&id);
    }
    drain_queue(app, store, queue, changes).await?;

    write_projection_version(index_dir, PROJECTION_VERSION)?;
    tracing::info!("rebuilt search index projection");
    Ok(())
}

async fn drain_queue<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    store: &SessionStore,
    queue: &DirtyQueue,
    changes: &mut ChangeReceiver,
) -> WorkerResult<()> {
    loop {
        absorb_pending_changes(queue, changes);

        let batch = queue.batch(BATCH_SIZE);
        if batch.is_empty() {
            return Ok(());
        }

        let mut documents = Vec::new();
        let mut removals = Vec::new();
        for (id, _generation) in &batch {
            match build_session_document(store, id).await {
                IndexAction::Upsert(document) => documents.push(document),
                IndexAction::Remove(id) => removals.push(id),
            }
        }

        if !documents.is_empty() {
            app.tantivy().update_documents(None, documents).await?;
        }
        for id in removals {
            app.tantivy().remove_document(None, id).await?;
        }

        // A session re-dirtied while its document was being built must be indexed
        // again: pull in everything that arrived during the batch so a re-dirtied
        // session's generation has moved past the one acknowledged below. The
        // store updates its index before it notifies, so any change this batch's
        // content read missed is either already absorbed here (generation bumped,
        // acknowledge no-ops) or still in the channel (re-marked next iteration).
        absorb_pending_changes(queue, changes);
        for (id, generation) in &batch {
            queue.acknowledge(id, *generation);
        }

        tokio::task::yield_now().await;
    }
}

async fn build_session_document(store: &SessionStore, id: &str) -> IndexAction {
    let Some(record) = store.session_get(id) else {
        return IndexAction::Remove(id.to_string());
    };

    // Same assembly order as the SQL projection: note body, then summary/
    // template_output docs by (sort_order, id), then transcripts by
    // (started_at, created_at, id). A transcript file that fails to read/parse
    // must not starve the rest of the batch: index note/docs only and let the
    // dirty queue re-mark this session on its next change.
    let enhanced_docs = store.session_enhanced_docs(id);
    let transcripts = store.session_transcripts(id).await.unwrap_or_else(|error| {
        tracing::warn!(%id, %error, "search projection: transcript read failed; indexing without transcript content");
        Vec::new()
    });

    let mut content_parts = Vec::with_capacity(1 + enhanced_docs.len() + transcripts.len());
    let mut related_parts = Vec::with_capacity(2 + enhanced_docs.len());
    if let Some(note) = &record.note_markdown {
        let note = extract_plain_text(note);
        content_parts.push(note.clone());
        related_parts.push(note);
    }
    let enhanced_parts: Vec<String> = enhanced_docs
        .iter()
        .map(|doc| extract_plain_text(&doc.markdown))
        .collect();
    content_parts.extend(enhanced_parts.iter().cloned());
    related_parts.extend(enhanced_parts);
    let transcript_parts: Vec<String> = transcripts.iter().map(flatten_transcript_words).collect();
    let transcript_content = merge_content(transcript_parts.iter().map(String::as_str));
    if !transcript_content.is_empty() {
        content_parts.push(transcript_content.clone());
        related_parts.push(transcript_content);
    }
    let related_content = merge_content(related_parts.iter().map(String::as_str));

    IndexAction::Upsert(SearchDocument {
        id: id.to_string(),
        doc_type: "session".to_string(),
        language: None,
        title: fallback_title(&record.meta.title, "Untitled"),
        content: merge_content(content_parts.iter().map(String::as_str)),
        related_content: (!related_content.is_empty()).then_some(related_content),
        created_at: to_epoch_ms(&Value::String(record.meta.created_at.clone())),
        facets: Vec::new(),
    })
}

async fn index_document_count<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<usize, tauri_plugin_tantivy::Error> {
    let result = app
        .tantivy()
        .search(SearchRequest {
            query: String::new(),
            collection: None,
            filters: SearchFilters::default(),
            limit: 1,
            options: SearchOptions::default(),
        })
        .await?;
    Ok(result.count)
}

async fn wait_for_index_count<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    expected: usize,
) -> Result<bool, tauri_plugin_tantivy::Error> {
    for _ in 0..40 {
        if index_document_count(app).await? == expected {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    Ok(false)
}

fn projection_version_path(index_dir: &Path) -> PathBuf {
    index_dir.join("projection_version")
}

/// Missing/unparseable reads as 0 -- deleting the `search_index/` dir (or a fresh
/// install) therefore always forces a full rebuild, never a stale skip.
fn read_projection_version(index_dir: &Path) -> i64 {
    std::fs::read_to_string(projection_version_path(index_dir))
        .ok()
        .and_then(|version| version.trim().parse().ok())
        .unwrap_or(0)
}

fn write_projection_version(index_dir: &Path, version: i64) -> std::io::Result<()> {
    std::fs::create_dir_all(index_dir)?;
    std::fs::write(projection_version_path(index_dir), version.to_string())
}

fn fallback_title(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn merge_content<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn extract_plain_text(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return trimmed.to_string();
    }

    let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
        return trimmed.to_string();
    };
    let Some(object) = parsed.as_object() else {
        return trimmed.to_string();
    };
    if object.get("type").and_then(Value::as_str) != Some("doc")
        || !object.get("content").is_some_and(Value::is_array)
    {
        return trimmed.to_string();
    }

    normalize_whitespace(&extract_tiptap_text(&parsed))
}

fn extract_tiptap_text(node: &Value) -> String {
    if let Some(text) = node.get("text").and_then(Value::as_str)
        && !text.is_empty()
    {
        return text.to_string();
    }

    node.get("content")
        .and_then(Value::as_array)
        .map(|children| {
            children
                .iter()
                .map(extract_tiptap_text)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The typed replacement for the old `words_json` flattening: the store's
/// transcripts are structurally-guaranteed word lists, and the SQL flattener's
/// text/content preference collapsed a well-formed words array to exactly this --
/// each word's `text`, space-joined, empties dropped.
fn flatten_transcript_words(transcript: &hypr_fs_format::TranscriptWithData) -> String {
    merge_content(transcript.words.iter().map(|word| word.text.as_str()))
}

fn to_epoch_ms(value: &Value) -> i64 {
    match value {
        Value::Number(value) => value.as_f64().unwrap_or(0.0) as i64,
        Value::String(value) => DateTime::parse_from_rfc3339(value)
            .map(|date| date.timestamp_millis())
            .ok()
            .or_else(|| value.parse::<f64>().ok().map(|value| value as i64))
            .unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_store::SessionMeta;
    use tauri::Manager;
    use tauri_plugin_tantivy::{CollectionIndex, IndexState, build_schema, register_tokenizers};

    /// Port of the SQL-era generation-race regression test ("acknowledgement does
    /// not drop a concurrent change"): a session re-dirtied WHILE being indexed
    /// must survive the stale acknowledgement and be processed again.
    #[test]
    fn acknowledgement_does_not_drop_a_concurrent_change() {
        let queue = DirtyQueue::default();
        queue.mark("session-1");
        let queued_generation = queue.batch(BATCH_SIZE)[0].1;

        // The concurrent edit, arriving while the batch above is being indexed.
        queue.mark("session-1");

        queue.acknowledge("session-1", queued_generation);
        let current = queue.batch(BATCH_SIZE);
        assert_eq!(
            current,
            vec![("session-1".to_string(), queued_generation + 1)],
            "a stale acknowledgement must leave the re-dirtied session queued"
        );

        queue.acknowledge("session-1", queued_generation + 1);
        assert_eq!(queue.len(), 0);
        assert!(queue.batch(BATCH_SIZE).is_empty());
    }

    #[test]
    fn remarking_moves_a_session_to_the_back_of_the_queue() {
        let queue = DirtyQueue::default();
        queue.mark("s1");
        queue.mark("s2");
        queue.mark("s1");
        let ids: Vec<String> = queue
            .batch(BATCH_SIZE)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, vec!["s2".to_string(), "s1".to_string()]);
    }

    #[test]
    fn extracts_text_only_from_valid_tiptap_documents() {
        assert_eq!(
            extract_plain_text(
                r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"first"},{"type":"text","text":"second"}]}]}"#,
            ),
            "first second"
        );
        assert_eq!(
            extract_plain_text(r#"{"type":"paragraph","text":"unchanged"}"#),
            r#"{"type":"paragraph","text":"unchanged"}"#
        );
        assert_eq!(extract_plain_text("  plain note  "), "plain note");
    }

    #[test]
    fn transcript_words_flatten_to_space_joined_text() {
        let transcript = transcript(
            "t1",
            0.0,
            vec![word("w0", "hello"), word("w1", "  "), word("w2", "world")],
        );
        assert_eq!(flatten_transcript_words(&transcript), "hello world");
    }

    #[test]
    fn session_timestamp_derives_from_created_at() {
        assert_eq!(
            to_epoch_ms(&Value::String("2025-01-01T00:00:00Z".to_string())),
            1_735_689_600_000
        );
        assert_eq!(to_epoch_ms(&Value::String(String::new())), 0);
    }

    // -- end-to-end projection over a real Tantivy index --------------------------

    fn meta(id: &str, title: &str) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            title: title.to_string(),
            started_at: None,
            ended_at: None,
            created_at: "2026-07-24T00:00:00Z".to_string(),
            tags: vec![],
            tag_suggestions: None,
            tracking_id: None,
            folder: None,
            author: None,
            skill: None,
            extra: Default::default(),
        }
    }

    fn word(id: &str, text: &str) -> hypr_fs_format::TranscriptWord {
        hypr_fs_format::TranscriptWord {
            id: Some(id.to_string()),
            text: text.to_string(),
            start_ms: 0.0,
            end_ms: 0.0,
            channel: 0.0,
            speaker: None,
            metadata: None,
        }
    }

    fn transcript(
        id: &str,
        started_at: f64,
        words: Vec<hypr_fs_format::TranscriptWord>,
    ) -> hypr_fs_format::TranscriptWithData {
        hypr_fs_format::TranscriptWithData {
            id: id.to_string(),
            user_id: String::new(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            session_id: String::new(),
            started_at,
            ended_at: None,
            memo_md: String::new(),
            words,
            speaker_hints: vec![],
        }
    }

    struct Harness {
        app: tauri::App<tauri::test::MockRuntime>,
        store: Arc<SessionStore>,
        queue: DirtyQueue,
        changes: ChangeReceiver,
        index_dir: PathBuf,
        vault: tempfile::TempDir,
        _state_dir: tempfile::TempDir,
    }

    /// A real Tantivy collection (in RAM, real schema + tokenizers) behind the
    /// same `IndexState` the plugin manages, plus a store over a tempdir vault
    /// with the projection's change-stream tap already subscribed -- the same
    /// subscribe-before-anything-happens ordering `spawn` relies on.
    async fn harness() -> Harness {
        let vault = tempfile::tempdir().unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        let store =
            Arc::new(crate::session_store::new_test_store(vault.path().to_path_buf()).await);
        let changes = store.subscribe_index_changes();

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(IndexState::default());
        let startup = crate::startup::StartupState::new(Some(vault.path()));
        startup.set_phase(crate::startup::StartupPhase::Ready);
        app.manage(startup);

        let schema = build_schema();
        let index = tantivy::Index::create_in_ram(schema.clone());
        register_tokenizers(&index);
        let reader = index
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .unwrap();
        let writer = index.writer(50_000_000).unwrap();
        app.state::<IndexState>()
            .inner
            .write()
            .await
            .collections
            .insert(
                "default".to_string(),
                CollectionIndex {
                    schema,
                    index,
                    reader,
                    writer,
                },
            );

        Harness {
            app,
            store,
            queue: DirtyQueue::default(),
            changes,
            index_dir: state_dir.path().join(INDEX_DIR_NAME),
            vault,
            _state_dir: state_dir,
        }
    }

    async fn all_documents<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Vec<SearchDocument> {
        app.tantivy()
            .search(SearchRequest {
                query: String::new(),
                collection: None,
                filters: SearchFilters::default(),
                limit: 100,
                options: SearchOptions::default(),
            })
            .await
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.document)
            .collect()
    }

    /// The reader reloads on commit with a delay; poll until the predicate holds.
    async fn wait_for<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        description: &str,
        predicate: impl Fn(&[SearchDocument]) -> bool,
    ) -> Vec<SearchDocument> {
        for _ in 0..100 {
            let documents = all_documents(app).await;
            if predicate(&documents) {
                return documents;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "search index never reached expected state: {description}; documents: {:?}",
            all_documents(app).await
        );
    }

    async fn initialize_harness(h: &mut Harness) {
        let app = h.app.handle().clone();
        initialize(&app, &h.store, &h.queue, &mut h.changes, &h.index_dir)
            .await
            .unwrap();
    }

    async fn drain_harness(h: &mut Harness) {
        let app = h.app.handle().clone();
        drain_queue(&app, &h.store, &h.queue, &mut h.changes)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_edit_and_delete_flow_through_the_change_stream() {
        let mut h = harness().await;

        // Empty vault, no version file: first boot after the cutover rebuilds and
        // stamps the new projection version.
        initialize_harness(&mut h).await;
        assert_eq!(read_projection_version(&h.index_dir), PROJECTION_VERSION);

        // create -> searchable
        h.store
            .write_meta(&meta("s1", "Planning session"))
            .await
            .unwrap();
        h.store
            .write_note("s1", "Quarterly goals discussion")
            .await
            .unwrap();
        drain_harness(&mut h).await;
        let app = h.app.handle().clone();
        let docs = wait_for(&app, "s1 indexed with note", |docs| {
            docs.iter()
                .any(|doc| doc.id == "s1" && doc.content.contains("Quarterly goals discussion"))
        })
        .await;
        let doc = docs.iter().find(|doc| doc.id == "s1").unwrap();
        assert_eq!(doc.title, "Planning session");
        assert_eq!(doc.doc_type, "session");

        // edit -> reindexed (old content gone, new content present)
        h.store
            .write_note("s1", "Revised roadmap details")
            .await
            .unwrap();
        h.store
            .write_transcript("s1", transcript("t1", 5.0, vec![word("w0", "zebra token")]))
            .await
            .unwrap();
        drain_harness(&mut h).await;
        wait_for(&app, "s1 reindexed after edits", |docs| {
            docs.iter().any(|doc| {
                doc.id == "s1"
                    && doc.content.contains("Revised roadmap details")
                    && doc.content.contains("zebra token")
                    && !doc.content.contains("Quarterly goals discussion")
            })
        })
        .await;

        // a full-text query actually finds it
        let result = app
            .tantivy()
            .search(SearchRequest {
                query: "zebra".to_string(),
                collection: None,
                filters: SearchFilters::default(),
                limit: 10,
                options: SearchOptions::default(),
            })
            .await
            .unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].document.id, "s1");

        // delete -> gone
        h.store.delete_session("s1").await.unwrap();
        drain_harness(&mut h).await;
        wait_for(&app, "s1 removed", |docs| docs.is_empty()).await;
    }

    fn mark_no_rebuild(h: &Harness) {
        std::fs::write(
            projection_version_path(&h.index_dir),
            format!("{PROJECTION_VERSION}\n"),
        )
        .unwrap();
    }

    fn assert_no_rebuild(h: &Harness) {
        assert_eq!(
            std::fs::read_to_string(projection_version_path(&h.index_dir)).unwrap(),
            format!("{PROJECTION_VERSION}\n")
        );
    }

    #[tokio::test]
    async fn populated_projection_waits_for_loading_and_preserves_offline_and_loading_edits() {
        let mut h = harness().await;
        h.store
            .write_meta(&meta("s1", "Before restart"))
            .await
            .unwrap();
        initialize_harness(&mut h).await;
        wait_for(h.app.handle(), "existing projection", |docs| {
            docs.len() == 1
        })
        .await;
        mark_no_rebuild(&h);
        h.store.write_note("s1", "offline edit").await.unwrap();
        h.store = Arc::new(SessionStore::new(h.vault.path().to_path_buf()));
        h.changes = h.store.subscribe_index_changes();
        let store = h.store.clone();
        let startup = h
            .app
            .state::<crate::startup::StartupState>()
            .inner()
            .clone();
        startup.set_phase(crate::startup::StartupPhase::OpeningVault);
        let app = h.app.handle().clone();
        let version_path = projection_version_path(&h.index_dir);
        let loading = async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert_eq!(all_documents(&app).await.len(), 1);
            assert_eq!(
                std::fs::read_to_string(version_path).unwrap(),
                format!("{PROJECTION_VERSION}\n")
            );
            store.rebuild_index().await.unwrap();
            store
                .write_meta(&meta("s2", "Created during loading"))
                .await
                .unwrap();
            startup.set_phase(crate::startup::StartupPhase::Ready);
        };
        tokio::join!(initialize_harness(&mut h), loading);
        let docs = wait_for(h.app.handle(), "all startup edits searchable", |docs| {
            docs.len() == 2
        })
        .await;
        assert!(
            docs.iter()
                .any(|doc| doc.id == "s1" && doc.content.contains("offline edit"))
        );
        assert_no_rebuild(&h);
    }

    #[tokio::test]
    async fn failed_startup_leaves_projection_and_version_untouched() {
        let mut h = harness().await;
        h.store.write_meta(&meta("s1", "Existing")).await.unwrap();
        initialize_harness(&mut h).await;
        wait_for(h.app.handle(), "existing projection", |docs| {
            docs.len() == 1
        })
        .await;
        std::fs::write(projection_version_path(&h.index_dir), "old version").unwrap();
        h.app.state::<crate::startup::StartupState>().set_phase(
            crate::startup::StartupPhase::Failed {
                message: "failed".into(),
            },
        );
        assert!(
            initialize(
                h.app.handle(),
                &h.store,
                &h.queue,
                &mut h.changes,
                &h.index_dir
            )
            .await
            .is_err()
        );
        assert_eq!(all_documents(h.app.handle()).await.len(), 1);
        assert_eq!(
            std::fs::read_to_string(projection_version_path(&h.index_dir)).unwrap(),
            "old version"
        );
    }

    #[tokio::test]
    async fn empty_ready_vault_does_not_rebuild_matching_projection() {
        let mut h = harness().await;
        initialize_harness(&mut h).await;
        mark_no_rebuild(&h);
        initialize_harness(&mut h).await;
        assert_no_rebuild(&h);
        assert!(all_documents(h.app.handle()).await.is_empty());
    }

    #[tokio::test]
    async fn changes_during_count_settling_are_reconciled_before_rebuilding() {
        for count_changes in [false, true] {
            let mut h = harness().await;
            h.store.write_meta(&meta("s1", "Alpha")).await.unwrap();
            h.store.write_meta(&meta("s2", "Beta")).await.unwrap();
            initialize_harness(&mut h).await;
            wait_for(h.app.handle(), "two documents", |docs| docs.len() == 2).await;
            h.app
                .handle()
                .tantivy()
                .remove_document(None, "s2".into())
                .await
                .unwrap();
            wait_for(h.app.handle(), "missing projection document", |docs| {
                docs.len() == 1
            })
            .await;
            mark_no_rebuild(&h);
            let store = h.store.clone();
            let edit = async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if count_changes {
                    store.delete_session("s2").await.unwrap();
                } else {
                    store
                        .write_note("s2", "edit during settling")
                        .await
                        .unwrap();
                }
            };
            tokio::join!(initialize_harness(&mut h), edit);
            assert_no_rebuild(&h);
            assert_eq!(
                all_documents(h.app.handle()).await.len(),
                if count_changes { 1 } else { 2 }
            );
        }
    }

    #[tokio::test]
    async fn outdated_projection_version_rebuilds_even_when_counts_match() {
        let mut h = harness().await;
        h.store.write_meta(&meta("s1", "Alpha")).await.unwrap();
        initialize_harness(&mut h).await;
        wait_for(h.app.handle(), "one document", |docs| docs.len() == 1).await;
        write_projection_version(&h.index_dir, PROJECTION_VERSION - 1).unwrap();
        initialize_harness(&mut h).await;
        assert_eq!(read_projection_version(&h.index_dir), PROJECTION_VERSION);
        wait_for(h.app.handle(), "rebuilt document", |docs| docs.len() == 1).await;
    }

    #[tokio::test]
    async fn count_mismatch_triggers_a_full_rebuild() {
        let mut h = harness().await;
        h.store.write_meta(&meta("s1", "Alpha")).await.unwrap();
        h.store.write_meta(&meta("s2", "Beta")).await.unwrap();

        initialize_harness(&mut h).await;
        let app = h.app.handle().clone();
        wait_for(&app, "both sessions indexed", |docs| docs.len() == 2).await;

        // Simulate crash damage: the index silently lost a document while the
        // vault (and projection version) still say everything is fine.
        app.tantivy()
            .remove_document(None, "s2".to_string())
            .await
            .unwrap();
        wait_for(&app, "s2 dropped", |docs| docs.len() == 1).await;

        initialize_harness(&mut h).await;
        let docs = wait_for(&app, "rebuild restored both", |docs| docs.len() == 2).await;
        assert!(docs.iter().any(|doc| doc.id == "s2" && doc.title == "Beta"));
        assert_eq!(read_projection_version(&h.index_dir), PROJECTION_VERSION);
    }

    #[tokio::test]
    async fn external_edit_reindexes_through_the_refresh_path() {
        let mut h = harness().await;
        h.store
            .write_meta(&meta("s1", "Original title"))
            .await
            .unwrap();
        initialize_harness(&mut h).await;
        let app = h.app.handle().clone();
        wait_for(&app, "s1 indexed", |docs| {
            docs.iter().any(|doc| doc.title == "Original title")
        })
        .await;

        // An external editor touches the vault; vault_watch reacts by calling
        // refresh_session, whose index diff notifies the change bus. The created
        // directory has a readable name, so resolve it through the store.
        let dir = h
            .vault
            .path()
            .join(h.store.session_dir("s1").await.unwrap());
        std::fs::write(
            dir.join("_meta.json"),
            serde_json::to_vec_pretty(&meta("s1", "Edited outside")).unwrap(),
        )
        .unwrap();
        // Legacy note name on purpose: an old sync client may still deliver
        // `_memo.md`, and the refresh path must read it through the fallback.
        std::fs::write(dir.join("_memo.md"), "external memo content").unwrap();
        h.store.refresh_session("s1").await.unwrap();

        drain_harness(&mut h).await;
        wait_for(&app, "external edit reindexed", |docs| {
            docs.iter().any(|doc| {
                doc.id == "s1"
                    && doc.title == "Edited outside"
                    && doc.content.contains("external memo content")
            })
        })
        .await;
    }

    #[tokio::test]
    async fn build_session_document_matches_the_sql_projection_shape() {
        let h = harness().await;
        let m = meta("s1", "  ");
        h.store.write_meta(&m).await.unwrap();
        h.store.write_note("s1", "# raw note body").await.unwrap();
        h.store
            .write_enhanced_doc(&crate::session_store::EnhancedDoc {
                id: "doc-0".to_string(),
                session_id: "s1".to_string(),
                kind: "summary".to_string(),
                title: "Summary".to_string(),
                template_id: String::new(),
                sort_order: 1,
                markdown: "first summary body".to_string(),
            })
            .await
            .unwrap();
        h.store
            .write_enhanced_doc(&crate::session_store::EnhancedDoc {
                id: "doc-1".to_string(),
                session_id: "s1".to_string(),
                kind: "template_output".to_string(),
                title: "Review".to_string(),
                template_id: "template-1".to_string(),
                sort_order: 2,
                markdown: "enhanced doc body".to_string(),
            })
            .await
            .unwrap();
        h.store
            .write_transcript(
                "s1",
                transcript("t1", 5.0, vec![word("w0", "spoken"), word("w1", "words")]),
            )
            .await
            .unwrap();

        let IndexAction::Upsert(doc) = build_session_document(&h.store, "s1").await else {
            panic!("expected an upsert for an existing session");
        };
        assert_eq!(doc.id, "s1");
        assert_eq!(
            doc.title, "Untitled",
            "blank title falls back like the SQL build"
        );
        assert_eq!(
            doc.content, "# raw note body first summary body enhanced doc body spoken words",
            "note, then enhanced docs by (sort_order, id), then transcript words"
        );
        assert_eq!(
            doc.created_at, 1_784_851_200_000,
            "timestamp comes from created_at"
        );

        assert!(matches!(
            build_session_document(&h.store, "missing").await,
            IndexAction::Remove(_)
        ));
    }

    /// A corrupt transcript.json must not starve the projection: the session is still
    /// upserted from its note/docs (the dirty queue re-marks it on the next transcript
    /// change), rather than failing the whole batch.
    #[tokio::test]
    async fn malformed_transcript_still_upserts_note_and_doc_content() {
        let h = harness().await;
        h.store
            .write_meta(&meta("s1", "Broken tape"))
            .await
            .unwrap();
        h.store.write_note("s1", "note survives").await.unwrap();
        h.store
            .write_transcript("s1", transcript("t1", 5.0, vec![word("w0", "spoken")]))
            .await
            .unwrap();

        let dir = h
            .vault
            .path()
            .join(h.store.session_dir("s1").await.unwrap());
        std::fs::write(dir.join("transcript.json"), b"{ not json").unwrap();

        let IndexAction::Upsert(doc) = build_session_document(&h.store, "s1").await else {
            panic!("expected an upsert despite the malformed transcript");
        };
        assert_eq!(doc.title, "Broken tape");
        assert_eq!(doc.content, "note survives");
    }
}

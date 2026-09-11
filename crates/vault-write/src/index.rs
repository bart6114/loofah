//! In-memory vault index + `index-changed` event bus (Phase E1).
//!
//! The index is a typed, RwLock'd mirror of the vault files -- sessions
//! (`_meta.json` + `notes.md`), documents (`enhanced/<uuid>.md`),
//! transcripts (`transcript.json`) and tasks (`tasks.json`) -- built at startup by
//! `rebuild_index` and kept current by:
//!
//! 1. **Write-through**: every store write updates the index synchronously right after
//!    the file write lands (the search projection rides this bus since Phase F), then
//!    pushes a change onto the bus.
//! 2. **Rescans**: `rebuild_index` / `refresh_session` (startup, focus rescan,
//!    `vault_watch` external-edit ingestion -- see `rebuild.rs`) re-derive the affected
//!    slice of the index from the files and notify only what actually changed
//!    (`PartialEq` diff), so repeated rescans over unchanged files stay silent.
//!
//! The bus coalesces changes for ~10ms (receive one change, sleep `COALESCE_WINDOW`,
//! drain everything else that arrived)
//! and emits one `index-changed { entity, ids }` Tauri event per entity to all
//! webviews. Granularity is table-level: `entity` names which map changed, `ids` are
//! session ids (docs/transcripts/tasks carry their owning session id; the vault-root
//! tasks file uses the reserved empty-string id).
//!
//! Corruption must never look like deletion (same invariant as `rebuild.rs`): a file
//! that fails to read/parse during a rescan leaves the existing index entry untouched;
//! only a confirmed-absent file removes one.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{EnhancedDoc, PersonItem, SessionMeta, SessionStore, StoreError, TagItem, TaskItem};
use hypr_fs_format::TranscriptWithData;

/// Which index map changed. Serialized as the lowercase strings the frontend matches
/// on in the `index-changed` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum IndexEntity {
    Sessions,
    Docs,
    Transcripts,
    Tasks,
    People,
    /// The vault-root `tags.json` registry changed (not a session's `_meta.json`
    /// tags -- those ride `Sessions`).
    Tags,
    /// A session's *physical directory* changed (rename, move, delete/restore,
    /// external relocation caught by a rebuild) -- content-free, so the search
    /// projection ignores it; the frontend uses it to invalidate every cache
    /// holding an absolute session path.
    Locations,
}

/// Emitted (coalesced) to every webview as the `index-changed` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[cfg_attr(feature = "tauri-events", derive(tauri_specta::Event))]
pub struct IndexChanged {
    pub entity: IndexEntity,
    pub ids: Vec<String>,
}

/// `meta` + note markdown for one session; the unit of the `sessions` map. The note
/// rides here (not in `docs`) because every consumer of the note also wants the meta
/// (`useSession`'s join), and `notes.md` shares the session's identity, not a doc id.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEntry {
    pub meta: SessionMeta,
    pub note_markdown: Option<String>,
}

/// What `session_get` returns: the file-canonical equivalent of the old
/// `SESSION_SELECT_SQL` (sessions row + COALESCE'd note document join). The note is
/// always the note file's content (`notes.md`, or the pre-rename `_memo.md`) -- the
/// SQL fallback's legacy bare-id row was a permanently-empty placeholder, so
/// preferring the file loses nothing.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct SessionRecord {
    pub meta: SessionMeta,
    pub note_markdown: Option<String>,
}

/// One `session_list` entry: full meta plus the derived flags list consumers need
/// without a per-session round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct SessionListEntry {
    pub meta: SessionMeta,
    pub has_transcript_words: bool,
}

/// The slim `session_list_headers` row -- exactly what the always-mounted list
/// subscribers (timeline, summaries, tags, float) consume.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct SessionListHeader {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub folder: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    pub has_transcript_words: bool,
}

/// Key for the vault-root `tasks.json` in the tasks map (sessions can never claim it:
/// folder names are non-empty path segments).
pub(super) const VAULT_TASKS_KEY: &str = "";

/// What the index keeps per `transcript.json` instead of the words themselves --
/// transcript words are read from disk on demand (`session_transcripts`), so a large
/// vault's word corpus never stays resident.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSummary {
    /// Transcript ids in file order -- powers `transcript_get`'s id -> session
    /// resolution, `session_is_empty`'s count, and `RebuildReport.transcripts`.
    pub transcript_ids: Vec<String>,
    /// Any transcript in the file has at least one word.
    pub has_words: bool,
    /// Total words across the file's transcripts. Both producers already walk the
    /// words (write-through has them in hand, rebuild lexes them for `has_words`),
    /// so keeping the count costs nothing and lets `vault_stats` skip re-reading
    /// every `transcript.json` from disk.
    pub word_count: u64,
    /// Truncated sha2 of the raw file bytes. Sole purpose: an external edit that
    /// changes word content without changing the file's shape (same ids, same
    /// counts) must still flip `PartialEq` so a rescan notifies `Transcripts`.
    pub content_hash: u64,
}

fn session_has_attachments(session_dir: &std::path::Path) -> Result<bool, StoreError> {
    let entries = std::fs::read_dir(session_dir).map_err(|error| {
        StoreError::Io(format!(
            "failed to inspect session directory {}: {error}",
            session_dir.display()
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            StoreError::Io(format!(
                "failed to inspect session directory {}: {error}",
                session_dir.display()
            ))
        })?;
        let name = entry.file_name();
        let is_owned = name
            .to_str()
            .is_some_and(hypr_vault_read::is_session_owned_name);
        if !is_owned
            && entry
                .file_type()
                .map_err(|error| {
                    StoreError::Io(format!(
                        "failed to inspect session entry {}: {error}",
                        entry.path().display()
                    ))
                })?
                .is_file()
        {
            return Ok(true);
        }
    }

    let attachments_dir = session_dir.join("attachments");
    let entries = match std::fs::read_dir(&attachments_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(StoreError::Io(format!(
                "failed to inspect attachments directory {}: {error}",
                attachments_dir.display()
            )));
        }
    };

    for entry in entries {
        let entry = entry.map_err(|error| {
            StoreError::Io(format!(
                "failed to inspect attachments directory {}: {error}",
                attachments_dir.display()
            ))
        })?;
        if !entry.file_name().to_string_lossy().starts_with('.') {
            return Ok(true);
        }
    }

    Ok(false)
}

/// The in-memory mirror of the vault. Maps are kept independent rather than nested
/// under one session struct, so a ghost session's transcripts can exist without a meta
/// exactly like the write path allows
/// (`recording_into_unknown_session_still_persists`). Empty
/// collections are normalized to absent keys so `PartialEq` diffs stay meaningful.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct VaultIndex {
    pub sessions: HashMap<String, SessionEntry>,
    /// Session id -> that session's `enhanced/<uuid>.md` docs.
    pub docs: HashMap<String, Vec<EnhancedDoc>>,
    pub transcripts: HashMap<String, TranscriptSummary>,
    /// Session id (or `VAULT_TASKS_KEY`) -> that file's tasks.
    pub tasks: HashMap<String, Vec<TaskItem>>,
    pub people: HashMap<String, PersonItem>,
    pub tags: HashMap<String, TagItem>,
}

pub(crate) type IndexChangeSender = tokio::sync::mpsc::UnboundedSender<(IndexEntity, Vec<String>)>;
pub type IndexChangeReceiver = tokio::sync::mpsc::UnboundedReceiver<(IndexEntity, Vec<String>)>;

// -- queries ---------------------------------------------------------------------

impl SessionStore {
    /// Old `useSession`/`useSessionSummary` semantics: the session's meta plus
    /// note markdown (the COALESCE(store_note, legacy_note) join collapses to
    /// the file -- see `SessionRecord`'s doc). `None` for unknown/ghost sessions.
    pub fn session_get(&self, session_id: &str) -> Option<SessionRecord> {
        let index = self.index.read().unwrap();
        index.sessions.get(session_id).map(|entry| SessionRecord {
            meta: entry.meta.clone(),
            note_markdown: entry.note_markdown.clone(),
        })
    }

    /// Every indexed session, ordered by `(created_at, id)` ascending -- the timeline
    /// query's ordering; list consumers that want newest-first (summaries, snapshot
    /// ids) reverse it. Ghost sessions (content without `_meta.json`) are absent,
    /// like the SQL `sessions` table.
    pub fn session_list(&self) -> Vec<SessionListEntry> {
        let index = self.index.read().unwrap();
        let mut entries: Vec<SessionListEntry> = index
            .sessions
            .values()
            .map(|entry| SessionListEntry {
                meta: entry.meta.clone(),
                has_transcript_words: has_transcript_words(&index, &entry.meta.id),
            })
            .collect();
        entries.sort_by(|a, b| {
            (a.meta.created_at.as_str(), a.meta.id.as_str())
                .cmp(&(b.meta.created_at.as_str(), b.meta.id.as_str()))
        });
        entries
    }

    /// `session_list` minus everything the list consumers never read: at 1,000+
    /// sessions the full metas (with `extra`) are refetched by several subscribers
    /// on every `sessions` event, so the hot path ships only these fields.
    /// Same `(created_at, id)` ascending order as `session_list`.
    pub fn session_list_headers(&self) -> Vec<SessionListHeader> {
        let index = self.index.read().unwrap();
        let mut entries: Vec<SessionListHeader> = index
            .sessions
            .values()
            .map(|entry| SessionListHeader {
                id: entry.meta.id.clone(),
                title: entry.meta.title.clone(),
                created_at: entry.meta.created_at.clone(),
                folder: entry.meta.folder.clone(),
                tags: entry.meta.tags.clone(),
                author: entry.meta.author.clone(),
                has_transcript_words: has_transcript_words(&index, &entry.meta.id),
            })
            .collect();
        entries.sort_by(|a, b| {
            (a.created_at.as_str(), a.id.as_str()).cmp(&(b.created_at.as_str(), b.id.as_str()))
        });
        entries
    }

    /// Number of indexed sessions -- the search projection's consistency-guard
    /// denominator (was `COUNT(*) FROM sessions`).
    pub fn session_count(&self) -> usize {
        self.index.read().unwrap().sessions.len()
    }

    /// Old `loadActiveSessionIds` semantics: ids ordered by `created_at` DESC, id ASC.
    pub fn session_ids(&self) -> Vec<String> {
        let index = self.index.read().unwrap();
        let mut metas: Vec<(&str, &str)> = index
            .sessions
            .values()
            .map(|entry| (entry.meta.created_at.as_str(), entry.meta.id.as_str()))
            .collect();
        metas.sort_by(|a, b| b.0.cmp(a.0).then(a.1.cmp(b.1)));
        metas.into_iter().map(|(_, id)| id.to_string()).collect()
    }

    /// Old `useSessionHasTranscript` semantics: any transcript with at least one word.
    /// The SQL tombstone filter (`deleted_at IS NULL`) has no file counterpart -- the
    /// file's soft-delete shape is "words emptied", which this check already excludes.
    pub fn session_has_transcript(&self, session_id: &str) -> bool {
        let index = self.index.read().unwrap();
        has_transcript_words(&index, session_id)
    }

    /// Old `useEnhancedNoteRecords` semantics: docs with kind `summary` /
    /// `template_output`, ordered `(sort_order, id)`. The tombstone filter is
    /// inherent: deleted docs have no file, hence no entry. No kind filter is needed:
    /// every doc enters the index through `parse_enhanced_file` (which coerces
    /// unknown kinds to `summary`) or the validated persist path.
    pub fn session_enhanced_docs(&self, session_id: &str) -> Vec<EnhancedDoc> {
        let index = self.index.read().unwrap();
        let mut docs: Vec<EnhancedDoc> = index.docs.get(session_id).cloned().unwrap_or_default();
        docs.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.id.cmp(&b.id)));
        docs
    }

    /// Old `useEnhancedNote` semantics: one doc looked up by doc id alone (the
    /// frontend doesn't know the session at that call site).
    pub fn enhanced_doc_get(&self, doc_id: &str) -> Option<EnhancedDoc> {
        let index = self.index.read().unwrap();
        index
            .docs
            .values()
            .flatten()
            .find(|doc| doc.id == doc_id)
            .cloned()
    }

    /// Old `useSessionTranscripts` semantics: full transcripts ordered
    /// `(started_at, id)`. Everything in `transcript.json` is live -- see
    /// `session_has_transcript`'s note on the file's lack of a tombstone.
    ///
    /// Words are read from disk on demand (the index only keeps a
    /// `TranscriptSummary`). During a recording this is refetched roughly once per
    /// debounced flush -- the `Transcripts` event that triggers the refetch is only
    /// emitted *after* the file write lands (`write_transcript_json_locked`), so
    /// the read never observes less than the index copy used to hold, and the file
    /// is page-cache-warm.
    pub async fn session_transcripts(
        &self,
        session_id: &str,
    ) -> Result<Vec<TranscriptWithData>, StoreError> {
        let mut transcripts = self.read_transcript_json(session_id).await?.transcripts;
        // `(started_at, created_at, id)` -- the SQL this replaced ordered by all three, and the
        // tiebreaker is load-bearing: soft-deleted transcripts are written without a
        // `started_at`, so they all collapse to 0 and would otherwise order by random UUID.
        transcripts.sort_by(|a, b| {
            a.started_at
                .partial_cmp(&b.started_at)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(transcripts)
    }

    /// Old `useTranscript` / `mutateTranscript`-read semantics: one transcript by id.
    /// The owning session comes from the summaries; the words come from its file.
    pub async fn transcript_get(
        &self,
        transcript_id: &str,
    ) -> Result<Option<TranscriptWithData>, StoreError> {
        let Some(session_id) = self.transcript_session_id(transcript_id) else {
            return Ok(None);
        };
        Ok(self
            .read_transcript_json(&session_id)
            .await?
            .transcripts
            .into_iter()
            .find(|t| t.id == transcript_id))
    }

    /// Which session's `transcript.json` holds this transcript id, per the summaries.
    /// Ownership never moves between sessions, so callers that only need the session
    /// (speaker assignment) can stay sync and disk-free.
    pub(crate) fn transcript_session_id(&self, transcript_id: &str) -> Option<String> {
        let index = self.index.read().unwrap();
        index
            .transcripts
            .iter()
            .find(|(_, summary)| summary.transcript_ids.iter().any(|id| id == transcript_id))
            .map(|(session_id, _)| session_id.clone())
    }

    /// Whether tab-close cleanup may discard this session. Unknown sessions are empty;
    /// title, meaningful note content, transcripts, enhanced docs and tags are answered
    /// from the index. Only an otherwise-empty session pays for a directory probe so
    /// loose user attachments and editor-managed `attachments/` files also protect it.
    pub async fn session_is_empty(&self, session_id: &str) -> Result<bool, StoreError> {
        {
            let index = self.index.read().unwrap();
            let Some(entry) = index.sessions.get(session_id) else {
                return Ok(true);
            };

            if !entry.meta.title.trim().is_empty() {
                return Ok(false);
            }
            if let Some(note) = &entry.note_markdown {
                let trimmed = note.trim();
                if !trimmed.is_empty() && trimmed != "&nbsp;" {
                    return Ok(false);
                }
            }

            let transcript_count = index
                .transcripts
                .get(session_id)
                .map(|summary| summary.transcript_ids.len())
                .unwrap_or_default();
            let enhanced_count = index.docs.get(session_id).map(Vec::len).unwrap_or_default();
            if transcript_count > 0 || enhanced_count > 0 || !entry.meta.tags.is_empty() {
                return Ok(false);
            }
        }

        let session_dir = self.vault_base.join(self.session_dir(session_id).await?);
        tokio::task::spawn_blocking(move || session_has_attachments(&session_dir))
            .await
            .map_err(|error| StoreError::Io(format!("attachment probe task failed: {error}")))?
            .map(|has_attachments| !has_attachments)
    }

    /// Welcome-note lookup: the first session (by `created_at`, id) carrying this
    /// `tracking_id` -- either as the top-level meta field, or (legacy fallback, one
    /// release's worth of vaults) inside the retired calendar-event envelope that now
    /// round-trips through `extra`.
    pub fn session_find_by_tracking_id(&self, tracking_id: &str) -> Option<SessionMeta> {
        let index = self.index.read().unwrap();
        let mut matches: Vec<&SessionEntry> = index
            .sessions
            .values()
            .filter(|entry| {
                if entry.meta.tracking_id.as_deref() == Some(tracking_id) {
                    return true;
                }
                entry
                    .meta
                    .extra
                    .get("event")
                    .and_then(|event| event.get("tracking_id"))
                    .and_then(|value| value.as_str())
                    == Some(tracking_id)
            })
            .collect();
        matches.sort_by(|a, b| {
            (a.meta.created_at.as_str(), a.meta.id.as_str())
                .cmp(&(b.meta.created_at.as_str(), b.meta.id.as_str()))
        });
        matches.first().map(|entry| entry.meta.clone())
    }
}

fn has_transcript_words(index: &VaultIndex, session_id: &str) -> bool {
    index
        .transcripts
        .get(session_id)
        .is_some_and(|summary| summary.has_words)
}

// -- write-through mutations ------------------------------------------------------

impl SessionStore {
    /// Push a change onto the bus. Infallible by design: the receiver lives inside
    /// this store until the dispatcher takes it, so the channel can't be closed while
    /// writes happen; a store without a spawned dispatcher (tests) just accumulates.
    /// Every tap (`subscribe_index_changes`) gets its own copy; a tap whose receiver
    /// was dropped is pruned here.
    pub(super) fn notify_index_changed(&self, entity: IndexEntity, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        self.index_change_taps
            .lock()
            .unwrap()
            .retain(|tap| tap.send((entity, ids.clone())).is_ok());
        let _ = self.index_changes_tx.send((entity, ids));
    }

    /// Fan out a private copy of the raw change stream (the same tuples the
    /// `index-changed` dispatcher coalesces). The Tantivy search projection is the
    /// intended consumer -- subscribe before the startup `rebuild_index` so changes
    /// found on disk while the app was closed reach the projection too.
    pub fn subscribe_index_changes(&self) -> IndexChangeReceiver {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.index_change_taps.lock().unwrap().push(tx);
        rx
    }

    pub(super) fn notify_many(&self, changes: Vec<(IndexEntity, String)>) {
        let mut grouped: HashMap<IndexEntity, Vec<String>> = HashMap::new();
        for (entity, id) in changes {
            let ids = grouped.entry(entity).or_default();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        for (entity, ids) in grouped {
            self.notify_index_changed(entity, ids);
        }
    }

    pub(super) fn index_upsert_meta(&self, meta: &SessionMeta) {
        let mut index = self.index.write().unwrap();
        match index.sessions.get_mut(&meta.id) {
            Some(entry) => entry.meta = meta.clone(),
            None => {
                index.sessions.insert(
                    meta.id.clone(),
                    SessionEntry {
                        meta: meta.clone(),
                        note_markdown: None,
                    },
                );
            }
        }
    }

    /// No-op for a session without a meta entry: a bare note write into an unknown
    /// session leaves it a ghost, same as the SQL index only gaining a `sessions` row
    /// via `write_meta`/rebuild. The next rescan reconciles either way.
    pub(super) fn index_set_note(&self, session_id: &str, markdown: Option<String>) {
        let mut index = self.index.write().unwrap();
        if let Some(entry) = index.sessions.get_mut(session_id) {
            entry.note_markdown = markdown;
        }
    }

    pub(super) fn index_upsert_doc(&self, doc: &EnhancedDoc) {
        let mut index = self.index.write().unwrap();
        let docs = index.docs.entry(doc.session_id.clone()).or_default();
        match docs.iter_mut().find(|existing| existing.id == doc.id) {
            Some(existing) => *existing = doc.clone(),
            None => docs.push(doc.clone()),
        }
    }

    pub(super) fn index_remove_doc(&self, session_id: &str, doc_id: &str) {
        let mut index = self.index.write().unwrap();
        if let Some(docs) = index.docs.get_mut(session_id) {
            docs.retain(|doc| doc.id != doc_id);
            if docs.is_empty() {
                index.docs.remove(session_id);
            }
        }
    }

    pub(super) fn index_set_transcript_summary(
        &self,
        session_id: &str,
        summary: TranscriptSummary,
    ) {
        let mut index = self.index.write().unwrap();
        if summary.transcript_ids.is_empty() {
            index.transcripts.remove(session_id);
        } else {
            index.transcripts.insert(session_id.to_string(), summary);
        }
    }

    pub(super) fn index_set_tasks(&self, key: &str, tasks: Vec<TaskItem>) {
        let mut index = self.index.write().unwrap();
        if tasks.is_empty() {
            index.tasks.remove(key);
        } else {
            index.tasks.insert(key.to_string(), tasks);
        }
    }

    pub(super) fn index_upsert_person(&self, person: &PersonItem) {
        let mut index = self.index.write().unwrap();
        index.people.insert(person.id.clone(), person.clone());
    }

    pub(super) fn index_upsert_tag(&self, tag: &TagItem) {
        let mut index = self.index.write().unwrap();
        index.tags.insert(tag.id.clone(), tag.clone());
    }

    /// Removes every trace of a session (delete path). Returns which entities
    /// actually held something, so the caller only notifies what changed.
    pub(super) fn index_remove_session(&self, session_id: &str) -> Vec<(IndexEntity, String)> {
        let mut index = self.index.write().unwrap();
        let mut changes = Vec::new();
        if index.sessions.remove(session_id).is_some() {
            changes.push((IndexEntity::Sessions, session_id.to_string()));
        }
        if index.docs.remove(session_id).is_some() {
            changes.push((IndexEntity::Docs, session_id.to_string()));
        }
        if index.transcripts.remove(session_id).is_some() {
            changes.push((IndexEntity::Transcripts, session_id.to_string()));
        }
        if index.tasks.remove(session_id).is_some() {
            changes.push((IndexEntity::Tasks, session_id.to_string()));
        }
        changes
    }

    pub(super) fn index_remove_session_and_notify(&self, session_id: &str) {
        let changes = self.index_remove_session(session_id);
        self.notify_many(changes);
    }
}

// -- rescans (file -> index reconciliation) ---------------------------------------
//
// The full rescan entry points (`rebuild_index` / `refresh_session`) live in
// `rebuild.rs`; the helpers below are the vault-root entity paths.

impl SessionStore {
    /// Reload the people map from the vault-root `people.json` and notify changed person
    /// ids -- also the `vault_watch` entry point for external `people.json` edits. A
    /// missing file reads as empty, so an external delete notifies every removed id.
    pub async fn index_refresh_people(&self) {
        let people = match self.list_people().await {
            Ok(people) => people,
            Err(error) => {
                tracing::warn!(%error, "index: failed to rescan people; keeping current entries");
                return;
            }
        };

        let new_map: HashMap<String, PersonItem> = people
            .into_iter()
            .map(|person| (person.id.clone(), person))
            .collect();

        let changed_ids: Vec<String> = {
            let mut index = self.index.write().unwrap();
            let mut changed: Vec<String> = index
                .people
                .keys()
                .chain(new_map.keys())
                .filter(|id| index.people.get(*id) != new_map.get(*id))
                .cloned()
                .collect::<HashSet<String>>()
                .into_iter()
                .collect();
            changed.sort();
            index.people = new_map;
            changed
        };
        self.notify_index_changed(IndexEntity::People, changed_ids);
    }

    /// Reload the tags map from the vault-root `tags.json` and notify changed tag
    /// ids -- also the `vault_watch` entry point for external `tags.json` edits. A
    /// missing file reads as empty, so an external delete notifies every removed id.
    pub async fn index_refresh_tags(&self) {
        let tags = match self.list_tags().await {
            Ok(tags) => tags,
            Err(error) => {
                tracing::warn!(%error, "index: failed to rescan tags; keeping current entries");
                return;
            }
        };

        let new_map: HashMap<String, TagItem> =
            tags.into_iter().map(|tag| (tag.id.clone(), tag)).collect();

        let changed_ids: Vec<String> = {
            let mut index = self.index.write().unwrap();
            let mut changed: Vec<String> = index
                .tags
                .keys()
                .chain(new_map.keys())
                .filter(|id| index.tags.get(*id) != new_map.get(*id))
                .cloned()
                .collect::<HashSet<String>>()
                .into_iter()
                .collect();
            changed.sort();
            index.tags = new_map;
            changed
        };
        self.notify_index_changed(IndexEntity::Tags, changed_ids);
    }

    /// `None` on read/parse failure (keep the old entry); a missing file is an empty
    /// list (remove the entry -- "no tasks" is a real state).
    pub(super) async fn read_index_tasks(
        &self,
        relative: std::path::PathBuf,
    ) -> Option<Vec<TaskItem>> {
        let path = self.vault_base.join(relative);
        tokio::task::spawn_blocking(move || -> Option<Vec<TaskItem>> {
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
                Err(_) => return None,
            };
            serde_json::from_slice::<hypr_vault_read::TasksFile>(&bytes)
                .map(|file| file.tasks)
                .ok()
        })
        .await
        .ok()
        .flatten()
    }
}

/// Replace `map[key]` with `value` (empty -> absent), returning whether anything
/// observable changed.
pub(super) fn apply_map_value<V: PartialEq>(
    map: &mut HashMap<String, Vec<V>>,
    key: &str,
    value: Vec<V>,
) -> bool {
    let old = map.get(key);
    if value.is_empty() {
        if old.is_none() {
            return false;
        }
        map.remove(key);
        true
    } else {
        if old == Some(&value) {
            return false;
        }
        map.insert(key.to_string(), value);
        true
    }
}

/// `apply_map_value`'s sibling for the summaries map -- same empty-means-absent
/// normalization and `PartialEq` diff contract (`transcript_ids.is_empty()` is the
/// empty state).
pub(super) fn apply_transcript_summary(
    map: &mut HashMap<String, TranscriptSummary>,
    key: &str,
    summary: TranscriptSummary,
) -> bool {
    let old = map.get(key);
    if summary.transcript_ids.is_empty() {
        if old.is_none() {
            return false;
        }
        map.remove(key);
        true
    } else {
        if old == Some(&summary) {
            return false;
        }
        map.insert(key.to_string(), summary);
        true
    }
}

// -- event bus --------------------------------------------------------------------

/// How long the dispatcher waits after the first change before draining and emitting
/// -- receive, `sleep(10ms)`, drain, refresh. Carried over from the retired SQL
/// live-query dispatcher, whose coalescing window this preserves.
pub(crate) const COALESCE_WINDOW: std::time::Duration = std::time::Duration::from_millis(10);

/// Stable emission order so bursts serialize deterministically (and tests can assert
/// exact sequences).
const ENTITY_ORDER: [IndexEntity; 7] = [
    IndexEntity::Sessions,
    IndexEntity::Docs,
    IndexEntity::Transcripts,
    IndexEntity::Tasks,
    IndexEntity::People,
    IndexEntity::Tags,
    IndexEntity::Locations,
];

/// One coalesced flush: group a drained batch by entity, dedupe ids preserving
/// first-seen order, one `IndexChanged` per entity.
fn coalesce(batch: Vec<(IndexEntity, Vec<String>)>) -> Vec<IndexChanged> {
    let mut grouped: HashMap<IndexEntity, Vec<String>> = HashMap::new();
    for (entity, ids) in batch {
        let seen = grouped.entry(entity).or_default();
        for id in ids {
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
    }
    ENTITY_ORDER
        .into_iter()
        .filter_map(|entity| {
            grouped
                .remove(&entity)
                .map(|ids| IndexChanged { entity, ids })
        })
        .collect()
}

/// The dispatcher loop, generic over the emit sink for testability: block on the
/// first change, wait `COALESCE_WINDOW`, drain whatever else arrived, emit one event
/// per entity. Ends when the store (all senders) is dropped.
pub async fn run_index_change_dispatcher(mut rx: IndexChangeReceiver, emit: impl Fn(IndexChanged)) {
    while let Some(first) = rx.recv().await {
        tokio::time::sleep(COALESCE_WINDOW).await;
        let mut batch = vec![first];
        while let Ok(next) = rx.try_recv() {
            batch.push(next);
        }
        for event in coalesce(batch) {
            emit(event);
        }
    }
}

impl SessionStore {
    /// Hand the receiving end to the dispatcher (once). `None` when already taken.
    pub fn take_index_change_receiver(&self) -> Option<IndexChangeReceiver> {
        self.index_changes_rx.lock().unwrap().take()
    }
}

#[cfg(test)]
mod tests {
    use super::super::content::SessionMeta;
    use super::*;

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
    ) -> TranscriptWithData {
        TranscriptWithData {
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

    fn enhanced_doc(session_id: &str, doc_id: &str, sort_order: i32) -> EnhancedDoc {
        EnhancedDoc {
            id: doc_id.to_string(),
            session_id: session_id.to_string(),
            kind: "template_output".to_string(),
            title: "Customer review".to_string(),
            template_id: "template-1".to_string(),
            sort_order,
            markdown: "# Review".to_string(),
        }
    }

    async fn test_store() -> (SessionStore, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().to_path_buf();
        let store = SessionStore::new(vault);
        (store, temp)
    }

    /// `Locations` rides the same coalescing bus as every other entity: a burst of
    /// location changes collapses to one deduped event, emitted in the stable order.
    #[test]
    fn coalesce_groups_and_dedupes_locations_changes() {
        let events = coalesce(vec![
            (IndexEntity::Locations, vec!["s1".to_string()]),
            (IndexEntity::Sessions, vec!["s1".to_string()]),
            (
                IndexEntity::Locations,
                vec!["s2".to_string(), "s1".to_string()],
            ),
        ]);
        assert_eq!(
            events,
            vec![
                IndexChanged {
                    entity: IndexEntity::Sessions,
                    ids: vec!["s1".to_string()],
                },
                IndexChanged {
                    entity: IndexEntity::Locations,
                    ids: vec!["s1".to_string(), "s2".to_string()],
                },
            ]
        );
    }

    /// Physical directory of a session: creation now picks a human-readable name, so
    /// tests resolve it through the store instead of assuming `sessions/<id>`.
    async fn session_path(
        store: &SessionStore,
        vault: &tempfile::TempDir,
        id: &str,
    ) -> std::path::PathBuf {
        vault.path().join(store.session_dir(id).await.unwrap())
    }

    fn drain_changes(store: &SessionStore) -> Vec<(IndexEntity, Vec<String>)> {
        let mut rx = store
            .take_index_change_receiver()
            .expect("receiver taken once per test");
        let mut changes = Vec::new();
        while let Ok(change) = rx.try_recv() {
            changes.push(change);
        }
        // put it back for a later drain in the same test
        *store.index_changes_rx.lock().unwrap() = Some(rx);
        changes
    }

    fn changed_entities(store: &SessionStore) -> HashSet<IndexEntity> {
        drain_changes(store)
            .into_iter()
            .map(|(entity, _)| entity)
            .collect()
    }

    // -- startup build from a hand-seeded vault --

    #[tokio::test]
    async fn rebuild_builds_the_full_index_from_a_seeded_vault() {
        let (store, vault) = test_store().await;
        let dir = vault.path().join("sessions/s1");
        std::fs::create_dir_all(dir.join("enhanced")).unwrap();
        std::fs::write(
            dir.join("_meta.json"),
            serde_json::to_vec_pretty(&meta("s1", "Planning")).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("notes.md"), "# notes").unwrap();
        // A loose markdown file directly in the session dir is a user attachment,
        // not a document -- rebuild must leave it out of the index.
        std::fs::write(dir.join("summary.md"), "user attachment body").unwrap();
        std::fs::write(
            dir.join("enhanced/doc-1.md"),
            hypr_vault_read::render_enhanced_file(&enhanced_doc("s1", "doc-1", 2)).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("transcript.json"),
            serde_json::to_vec_pretty(&hypr_fs_format::TranscriptJson {
                transcripts: vec![transcript("t1", 100.0, vec![word("w0", "hello")])],
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("tasks.json"),
            serde_json::json!({
                "tasks": [{
                    "id": "task-1",
                    "source_type": "session_raw_note",
                    "source_id": "s1",
                    "source_order": 0,
                    "status": "todo",
                    "text": "Ship it",
                    "body": [],
                    "created_at": "2026-07-01T00:00:00Z",
                    "updated_at": "2026-07-01T00:00:00Z",
                }],
            })
            .to_string(),
        )
        .unwrap();

        store.rebuild_index().await.unwrap();

        let record = store.session_get("s1").unwrap();
        assert_eq!(record.meta.title, "Planning");
        assert_eq!(record.note_markdown.as_deref(), Some("# notes"));

        let docs = store.session_enhanced_docs("s1");
        assert_eq!(
            docs.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            vec!["doc-1"],
            "only enhanced/ docs are indexed; the loose summary.md attachment is ignored"
        );

        assert!(store.session_has_transcript("s1"));
        assert_eq!(
            store.session_transcripts("s1").await.unwrap()[0].words[0].text,
            "hello"
        );
        assert_eq!(store.transcript_get("t1").await.unwrap().unwrap().id, "t1");

        let index = store.index.read().unwrap();
        assert_eq!(index.tasks.get("s1").unwrap()[0].text, "Ship it");
    }

    #[tokio::test]
    async fn rebuild_removes_index_entries_for_vanished_sessions() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store.write_note("s1", "note").await.unwrap();
        assert!(store.session_get("s1").is_some());

        std::fs::remove_dir_all(session_path(&store, &vault, "s1").await).unwrap();
        store.rebuild_index().await.unwrap();

        assert!(store.session_get("s1").is_none());
        assert!(store.session_list().is_empty());
    }

    /// A second rebuild over unchanged files must not notify anything -- the
    /// in-memory analogue of rebuild's change-guarded SQL upserts, load-bearing
    /// because focus rescans run `rebuild_index` repeatedly.
    #[tokio::test]
    async fn rebuild_of_unchanged_files_notifies_nothing() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store.write_note("s1", "note").await.unwrap();
        store
            .write_enhanced_doc(&enhanced_doc("s1", "doc-1", 0))
            .await
            .unwrap();
        store.rebuild_index().await.unwrap();
        drain_changes(&store);

        store.rebuild_index().await.unwrap();
        assert_eq!(drain_changes(&store), vec![]);
    }

    // -- write-through --

    #[tokio::test]
    async fn writes_update_the_index_without_a_rebuild() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "Live")).await.unwrap();
        store.write_note("s1", "# memo").await.unwrap();
        store
            .write_enhanced_doc(&enhanced_doc("s1", "doc-1", 1))
            .await
            .unwrap();
        store
            .write_transcript("s1", transcript("t1", 5.0, vec![word("w0", "hi")]))
            .await
            .unwrap();
        store
            .replace_tasks(
                "session_raw_note",
                "s1",
                vec![super::super::TaskInput {
                    id: "task-1".to_string(),
                    source_order: 0,
                    status: "todo".to_string(),
                    text: "Do".to_string(),
                    body: serde_json::json!([]),
                    due_at: String::new(),
                }],
            )
            .await
            .unwrap();

        let record = store.session_get("s1").unwrap();
        assert_eq!(record.meta.title, "Live");
        assert_eq!(record.note_markdown.as_deref(), Some("# memo"));
        assert_eq!(store.session_enhanced_docs("s1").len(), 1);
        assert!(store.session_has_transcript("s1"));
        assert_eq!(store.session_transcripts("s1").await.unwrap().len(), 1);
        {
            let index = store.index.read().unwrap();
            assert_eq!(index.tasks.get("s1").unwrap().len(), 1);
        }

        let entities = changed_entities(&store);
        for entity in [
            IndexEntity::Sessions,
            IndexEntity::Docs,
            IndexEntity::Transcripts,
            IndexEntity::Tasks,
        ] {
            assert!(entities.contains(&entity), "missing {entity:?}");
        }
    }

    #[tokio::test]
    async fn delete_session_clears_every_map_and_notifies() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_transcript("s1", transcript("t1", 0.0, vec![word("w0", "hi")]))
            .await
            .unwrap();
        drain_changes(&store);

        store.delete_session("s1").await.unwrap();

        assert!(store.session_get("s1").is_none());
        assert!(store.session_transcripts("s1").await.unwrap().is_empty());
        let entities = changed_entities(&store);
        assert!(entities.contains(&IndexEntity::Sessions));
        assert!(entities.contains(&IndexEntity::Transcripts));
    }

    // -- command semantics --

    #[tokio::test]
    async fn session_get_falls_back_to_none_note_when_memo_file_is_absent() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "No note yet")).await.unwrap();
        let record = store.session_get("s1").unwrap();
        assert_eq!(record.note_markdown, None);

        store.write_note("s1", "now present").await.unwrap();
        assert_eq!(
            store.session_get("s1").unwrap().note_markdown.as_deref(),
            Some("now present"),
            "the store-written note (the old COALESCE's first branch) must win"
        );
    }

    #[tokio::test]
    async fn session_list_and_ids_are_ordered_like_the_old_queries() {
        let (store, _vault) = test_store().await;
        let mut a = meta("s-b", "B");
        a.created_at = "2026-07-02T00:00:00Z".to_string();
        let mut b = meta("s-a", "A");
        b.created_at = "2026-07-01T00:00:00Z".to_string();
        let mut c = meta("s-c", "C");
        c.created_at = "2026-07-02T00:00:00Z".to_string();
        for m in [&a, &b, &c] {
            store.write_meta(m).await.unwrap();
        }

        let listed: Vec<String> = store
            .session_list()
            .into_iter()
            .map(|entry| entry.meta.id)
            .collect();
        assert_eq!(
            listed,
            vec!["s-a", "s-b", "s-c"],
            "session_list orders by (created_at, id) ascending like the timeline query"
        );

        assert_eq!(
            store.session_ids(),
            vec!["s-b", "s-c", "s-a"],
            "session_ids orders by created_at DESC then id ASC like loadActiveSessionIds"
        );
    }

    #[tokio::test]
    async fn session_has_transcript_requires_words_like_json_array_length() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_transcript("s1", transcript("t1", 0.0, vec![]))
            .await
            .unwrap();
        assert!(
            !store.session_has_transcript("s1"),
            "a zero-word transcript (the file-level soft-delete shape) must not count"
        );

        store
            .write_transcript("s1", transcript("t1", 0.0, vec![word("w0", "hi")]))
            .await
            .unwrap();
        assert!(store.session_has_transcript("s1"));

        let entry = &store.session_list()[0];
        assert!(entry.has_transcript_words);
    }

    #[tokio::test]
    async fn enhanced_docs_are_ordered_by_sort_order_then_id() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_enhanced_doc(&enhanced_doc("s1", "doc-b", 1))
            .await
            .unwrap();
        store
            .write_enhanced_doc(&enhanced_doc("s1", "doc-a", 1))
            .await
            .unwrap();
        store
            .write_enhanced_doc(&{
                let mut doc = enhanced_doc("s1", "doc-z", 0);
                doc.kind = "summary".to_string();
                doc
            })
            .await
            .unwrap();
        let docs = store.session_enhanced_docs("s1");
        assert_eq!(
            docs.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            vec!["doc-z", "doc-a", "doc-b"]
        );

        assert_eq!(store.enhanced_doc_get("doc-a").unwrap().session_id, "s1");
    }

    #[tokio::test]
    async fn deleted_enhanced_doc_disappears_from_queries() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_enhanced_doc(&enhanced_doc("s1", "doc-1", 0))
            .await
            .unwrap();
        store.delete_enhanced_doc("s1", "doc-1").await.unwrap();

        assert!(store.session_enhanced_docs("s1").is_empty());
        assert!(store.enhanced_doc_get("doc-1").is_none());
    }

    #[tokio::test]
    async fn session_transcripts_are_ordered_by_started_at_then_id() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_transcript("s1", transcript("t-late", 200.0, vec![word("w0", "b")]))
            .await
            .unwrap();
        store
            .write_transcript("s1", transcript("t-early", 100.0, vec![word("w1", "a")]))
            .await
            .unwrap();

        let ids: Vec<String> = store
            .session_transcripts("s1")
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec!["t-early", "t-late"]);
    }

    #[tokio::test]
    async fn transcripts_sharing_a_started_at_fall_back_to_created_at_not_id() {
        // Soft-deleted transcripts are written without a `started_at`, so they all collapse to
        // 0.0 and tie. `created_at` is store-managed (insertion time), so ties must resolve in
        // insertion order; the ids here are chosen so ordering by id would disagree.
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();

        store
            .write_transcript(
                "s1",
                transcript("zzz-written-first", 0.0, vec![word("w0", "a")]),
            )
            .await
            .unwrap();
        store
            .write_transcript(
                "s1",
                transcript("aaa-written-second", 0.0, vec![word("w1", "b")]),
            )
            .await
            .unwrap();

        let ids: Vec<String> = store
            .session_transcripts("s1")
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(
            ids,
            vec!["zzz-written-first", "aaa-written-second"],
            "ties must resolve chronologically, not by id"
        );
    }

    #[tokio::test]
    async fn session_is_empty_matches_the_old_sql_semantics() {
        let (store, _vault) = test_store().await;
        assert!(
            store.session_is_empty("ghost").await.unwrap(),
            "unknown session is empty"
        );

        store.write_meta(&meta("s1", "")).await.unwrap();
        assert!(
            store.session_is_empty("s1").await.unwrap(),
            "untitled contentless session is empty"
        );

        store.write_note("s1", "  &nbsp;  ").await.unwrap();
        assert!(
            store.session_is_empty("s1").await.unwrap(),
            "the editor's &nbsp; placeholder is not content"
        );

        store.write_note("s1", "real words").await.unwrap();
        assert!(!store.session_is_empty("s1").await.unwrap());
        store.write_note("s1", "").await.unwrap();

        // A bare title makes it non-empty...
        store
            .update_meta(
                "s1",
                super::super::SessionMetaPatch {
                    title: Some("Titled".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(!store.session_is_empty("s1").await.unwrap());

        // Clear the title again so the tags check below stands on its own.
        store
            .update_meta(
                "s1",
                super::super::SessionMetaPatch {
                    title: Some(String::new()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(store.session_is_empty("s1").await.unwrap());

        // tags count as content (session_tags stand-in)
        store
            .update_meta(
                "s1",
                super::super::SessionMetaPatch {
                    tags: Some(vec!["q3".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(!store.session_is_empty("s1").await.unwrap());
    }

    #[tokio::test]
    async fn session_is_empty_counts_loose_and_managed_attachments() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "")).await.unwrap();
        let session_dir = vault.path().join(store.session_dir("s1").await.unwrap());

        std::fs::write(session_dir.join("contract.pdf"), b"attachment").unwrap();
        assert!(!store.session_is_empty("s1").await.unwrap());
        std::fs::remove_file(session_dir.join("contract.pdf")).unwrap();

        let attachments_dir = session_dir.join("attachments");
        std::fs::create_dir(&attachments_dir).unwrap();
        assert!(store.session_is_empty("s1").await.unwrap());

        std::fs::write(attachments_dir.join(".DS_Store"), b"hidden").unwrap();
        assert!(store.session_is_empty("s1").await.unwrap());

        std::fs::write(attachments_dir.join("shot.png"), b"attachment").unwrap();
        assert!(!store.session_is_empty("s1").await.unwrap());
        std::fs::remove_file(attachments_dir.join("shot.png")).unwrap();

        std::fs::create_dir(attachments_dir.join("bundle")).unwrap();
        assert!(!store.session_is_empty("s1").await.unwrap());
    }

    #[tokio::test]
    async fn session_is_empty_ignores_owned_files_hidden_files_and_unknown_directories() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "")).await.unwrap();
        let session_dir = vault.path().join(store.session_dir("s1").await.unwrap());

        std::fs::write(session_dir.join(".DS_Store"), b"hidden").unwrap();
        std::fs::write(session_dir.join("audio.mp3"), b"recording").unwrap();
        std::fs::write(session_dir.join("audio.peaks.json"), b"cache").unwrap();
        std::fs::create_dir(session_dir.join("user-folder")).unwrap();

        assert!(store.session_is_empty("s1").await.unwrap());
    }

    #[tokio::test]
    async fn session_is_empty_fails_closed_when_an_indexed_session_cannot_be_inspected() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "")).await.unwrap();
        let session_dir = vault.path().join(store.session_dir("s1").await.unwrap());
        std::fs::remove_dir_all(session_dir).unwrap();

        assert!(store.session_is_empty("s1").await.is_err());
    }

    #[tokio::test]
    async fn session_find_by_tracking_id_picks_the_oldest_match() {
        let (store, _vault) = test_store().await;
        let mut newer = meta("s-new", "Newer");
        newer.created_at = "2026-07-02T00:00:00Z".to_string();
        newer.tracking_id = Some("evt-1".to_string());
        let mut older = meta("s-old", "Older");
        older.created_at = "2026-07-01T00:00:00Z".to_string();
        older.tracking_id = Some("evt-1".to_string());
        // Legacy shape: pre-removal builds carried the marker inside the calendar
        // event envelope, which now round-trips through `extra`.
        let mut other = meta("s-other", "Other");
        other.extra.insert(
            "event".to_string(),
            serde_json::json!({"tracking_id": "evt-2"}),
        );
        for m in [&newer, &older, &other] {
            store.write_meta(m).await.unwrap();
        }

        assert_eq!(
            store.session_find_by_tracking_id("evt-1").unwrap().id,
            "s-old"
        );
        assert_eq!(
            store.session_find_by_tracking_id("evt-2").unwrap().id,
            "s-other",
            "the legacy event-envelope marker must still be found"
        );
        assert_eq!(store.session_find_by_tracking_id("missing"), None);
    }

    // -- external-edit refresh (vault_watch path) --

    #[tokio::test]
    async fn refresh_session_ingests_an_external_edit_and_notifies() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Original")).await.unwrap();
        drain_changes(&store);

        let dir = session_path(&store, &vault, "s1").await;
        std::fs::write(
            dir.join("_meta.json"),
            serde_json::to_vec_pretty(&meta("s1", "Edited outside")).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("notes.md"), "external note").unwrap();

        store.refresh_session("s1").await.unwrap();

        let record = store.session_get("s1").unwrap();
        assert_eq!(record.meta.title, "Edited outside");
        assert_eq!(record.note_markdown.as_deref(), Some("external note"));
        let changes = drain_changes(&store);
        assert!(changes.iter().any(
            |(entity, ids)| *entity == IndexEntity::Sessions && ids.contains(&"s1".to_string())
        ));
    }

    #[tokio::test]
    async fn refresh_session_with_deleted_meta_removes_the_session() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store.write_note("s1", "keep the file").await.unwrap();
        let dir = session_path(&store, &vault, "s1").await;
        std::fs::remove_file(dir.join("_meta.json")).unwrap();

        store.refresh_session("s1").await.unwrap();

        assert!(store.session_get("s1").is_none());
        assert!(
            dir.join("notes.md").is_file(),
            "index refresh must never touch files"
        );
    }

    /// Corruption never looks like deletion: a meta that stops parsing keeps the old
    /// index entry (same invariant as rebuild's SQL half).
    #[tokio::test]
    async fn refresh_session_keeps_the_old_entry_for_a_corrupt_meta() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Original")).await.unwrap();
        std::fs::write(
            session_path(&store, &vault, "s1").await.join("_meta.json"),
            "{ not json",
        )
        .unwrap();

        let _ = store.refresh_session("s1").await;

        assert_eq!(store.session_get("s1").unwrap().meta.title, "Original");
    }

    #[tokio::test]
    async fn index_refresh_people_ingests_external_edits_and_deletions() {
        let (store, vault) = test_store().await;
        std::fs::write(
            vault.path().join("people.json"),
            serde_json::json!({ "people": [{ "id": "kim", "name": "Kim" }] }).to_string(),
        )
        .unwrap();

        store.index_refresh_people().await;

        {
            let index = store.index.read().unwrap();
            assert_eq!(index.people.get("kim").unwrap().name, "Kim");
        }
        let changes = drain_changes(&store);
        assert_eq!(
            changes,
            vec![(IndexEntity::People, vec!["kim".to_string()])]
        );

        std::fs::remove_file(vault.path().join("people.json")).unwrap();
        store.index_refresh_people().await;

        assert!(store.index.read().unwrap().people.is_empty());
        let changes = drain_changes(&store);
        assert_eq!(
            changes,
            vec![(IndexEntity::People, vec!["kim".to_string()])]
        );
    }

    // -- coalescing --

    #[tokio::test]
    async fn dispatcher_coalesces_a_burst_into_one_event_per_entity() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let emitted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = emitted.clone();
        let handle = tokio::spawn(run_index_change_dispatcher(rx, move |event| {
            sink.lock().unwrap().push(event);
        }));

        tx.send((IndexEntity::Sessions, vec!["s1".to_string()]))
            .unwrap();
        tx.send((
            IndexEntity::Sessions,
            vec!["s2".to_string(), "s1".to_string()],
        ))
        .unwrap();
        tx.send((IndexEntity::Docs, vec!["s1".to_string()]))
            .unwrap();
        drop(tx);
        handle.await.unwrap();

        let events = emitted.lock().unwrap().clone();
        assert_eq!(
            events,
            vec![
                IndexChanged {
                    entity: IndexEntity::Sessions,
                    ids: vec!["s1".to_string(), "s2".to_string()],
                },
                IndexChanged {
                    entity: IndexEntity::Docs,
                    ids: vec!["s1".to_string()],
                },
            ],
            "three sends within the window collapse to one event per entity, ids deduped"
        );
    }

    #[tokio::test]
    async fn dispatcher_emits_again_for_changes_after_a_flush() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let emitted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = emitted.clone();
        let handle = tokio::spawn(run_index_change_dispatcher(rx, move |event| {
            sink.lock().unwrap().push(event);
        }));

        tx.send((IndexEntity::Sessions, vec!["s1".to_string()]))
            .unwrap();
        // let the first window elapse and flush before the second change arrives
        tokio::time::sleep(COALESCE_WINDOW * 5).await;
        assert_eq!(emitted.lock().unwrap().len(), 1);

        tx.send((IndexEntity::Sessions, vec!["s2".to_string()]))
            .unwrap();
        drop(tx);
        handle.await.unwrap();

        let events = emitted.lock().unwrap().clone();
        assert_eq!(
            events.len(),
            2,
            "a change after the flush gets its own event"
        );
        assert_eq!(events[1].ids, vec!["s2".to_string()]);
    }

    #[test]
    fn index_changed_serializes_with_lowercase_entity_names() {
        let payload = serde_json::to_value(IndexChanged {
            entity: IndexEntity::Sessions,
            ids: vec!["s1".to_string()],
        })
        .unwrap();
        assert_eq!(
            payload,
            serde_json::json!({ "entity": "sessions", "ids": ["s1"] })
        );
        for (entity, expected) in [
            (IndexEntity::Sessions, "sessions"),
            (IndexEntity::Docs, "docs"),
            (IndexEntity::Transcripts, "transcripts"),
            (IndexEntity::Tasks, "tasks"),
            (IndexEntity::People, "people"),
        ] {
            assert_eq!(
                serde_json::to_value(entity).unwrap(),
                serde_json::json!(expected)
            );
        }
    }
}

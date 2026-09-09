use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub mod agents_doc;
pub mod attachments;
pub mod audio;
pub mod content;
#[cfg(test)]
mod dual_layout_tests;
pub mod enhanced;
pub mod index;
pub mod journal;
pub mod layout_name;
pub mod locations;
pub mod migrate;
pub mod paths;
pub mod people;
pub mod rebuild;
pub mod stats;
pub mod tags;
pub mod tasks;
pub mod templates;
pub mod transcript;

pub use attachments::SavedAttachment;
pub use content::{
    SessionMeta, SessionMetaPatch, TagSuggestionItem, TagSuggestionState, TagSuggestionStatus,
    is_tag_automation_candidate,
};
pub use enhanced::{EnhancedDoc, EnhancedDocPatch};
pub use index::{IndexChanged, IndexEntity, SessionListEntry, SessionListHeader, SessionRecord};
pub use people::PersonItem;
pub use rebuild::RebuildReport;
pub use stats::{VaultStats, VaultYearStats};
pub use tags::TagItem;
pub use tasks::{TaskInput, TaskItem};
pub use templates::{TemplateInput, TemplateItem};
pub use transcript::TranscriptDelta;

#[derive(Debug, Clone)]
pub struct SessionStore {
    vault_base: PathBuf,
    journal: Arc<journal::WriteJournal>,
    write_lock: Arc<tokio::sync::Mutex<()>>, // single store-wide lock; can become per-path if contention matters
    // one live buffer per actively-recording session; guards the debounced-flush lifecycle
    live: Arc<tokio::sync::Mutex<HashMap<String, transcript::LiveTranscriptBuffer>>>,
    /// In-memory vault index (Phase E1); see `index.rs`'s module doc.
    index: Arc<std::sync::RwLock<index::VaultIndex>>,
    /// Producer half of the `index-changed` bus; every write-through/rescan change
    /// lands here and the coalescing dispatcher (`index::spawn_dispatcher`) emits.
    index_changes_tx: index::IndexChangeSender,
    /// Held until the dispatcher takes it via `take_index_change_receiver`.
    index_changes_rx: Arc<std::sync::Mutex<Option<index::IndexChangeReceiver>>>,
    /// Extra change-stream consumers (`subscribe_index_changes`) -- Phase F: the
    /// Tantivy search projection rides one of these instead of SQL triggers.
    index_change_taps: Arc<std::sync::Mutex<Vec<index::IndexChangeSender>>>,
    /// Session-location catalog: logical id -> vault-relative physical directory
    /// (see `locations.rs`). Refreshed wholesale by `rebuild_index`, maintained
    /// incrementally by writes/deletes/restores, warmed lazily on cache misses.
    locations: Arc<std::sync::RwLock<HashMap<String, PathBuf>>>,
    /// Recent `delete_session` records backing the process-local undo toast
    /// (see `locations::DeletedSession`).
    recent_deletions: Arc<std::sync::Mutex<HashMap<String, locations::DeletedSession>>>,
    /// Per-session recording path leases. The provisional-to-final directory rename
    /// is deferred while a session holds any lease: `listener-core`'s DiskSink holds
    /// absolute paths into the directory and uses them during finalization, so
    /// renaming mid-recording is unsafe. A count (not a set) because both the
    /// frontend's `prepare_recording` and the transcription command reserve the path
    /// independently -- a failed duplicate start must release only its own
    /// reservation, never unprotect an already-active recording. The `Stopped`
    /// lifecycle (`mark_recording_ended`) clears every lease for the session.
    active_recordings: Arc<std::sync::Mutex<HashMap<String, usize>>>,
    /// Ids the last rebuild scan found claimed by more than one directory. Resolution
    /// checks this before `find_session`, whose legacy fast path would otherwise
    /// silently pick the canonical claimant and let reads/writes diverge the copies
    /// while rebuild keeps the id unindexed.
    known_duplicates: Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
    /// Bumped on every `catalog_remove`. A cold-miss discovery scan runs without the
    /// store write lock; its catalog warming is valid only if no entry was removed
    /// while the walk ran -- otherwise the scan's snapshot could resurrect a just-
    /// deleted session's location and let a late write recreate the trashed
    /// directory (breaking restore, whose rename refuses an occupied destination).
    catalog_removals: Arc<std::sync::atomic::AtomicU64>,
}

/// Product of `normalize_startup_layout`: one discovery snapshot -- with paths
/// updated by the migration/reconciliation renames -- plus their diagnostics.
/// Hand it to `rebuild_index_from_startup_layout` so the first index rebuild
/// reuses the walk instead of scanning again.
pub struct StartupLayout {
    pub(crate) scan: rebuild::SessionLayoutScan,
    pub migration: migrate::MigrationReport,
}

impl StartupLayout {
    pub fn session_count(&self) -> usize {
        self.scan.sessions.len()
    }
}

#[derive(Debug)]
pub enum StoreError {
    Io(String),
    Serialize(String),
    /// A compare-and-swap guard didn't match current file content. Stringifies with a
    /// stable `conflict:` prefix so the frontend can tell a benign CAS miss apart from a
    /// real failure across the IPC string boundary.
    Conflict(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(msg) => write!(f, "I/O error: {}", msg),
            StoreError::Serialize(msg) => write!(f, "Serialization error: {}", msg),
            StoreError::Conflict(msg) => write!(f, "conflict: {}", msg),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<hypr_vault_read::Error> for StoreError {
    fn from(err: hypr_vault_read::Error) -> Self {
        match err {
            hypr_vault_read::Error::Io(msg) => StoreError::Io(msg),
            hypr_vault_read::Error::Parse(msg) => StoreError::Serialize(msg),
        }
    }
}

impl SessionStore {
    pub fn new(vault_base: PathBuf) -> Self {
        let (index_changes_tx, index_changes_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            vault_base,
            journal: Arc::new(journal::WriteJournal::new()),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            live: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            index: Arc::new(std::sync::RwLock::new(index::VaultIndex::default())),
            index_changes_tx,
            index_changes_rx: Arc::new(std::sync::Mutex::new(Some(index_changes_rx))),
            index_change_taps: Arc::new(std::sync::Mutex::new(Vec::new())),
            locations: Arc::new(std::sync::RwLock::new(HashMap::new())),
            recent_deletions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            active_recordings: Arc::new(std::sync::Mutex::new(HashMap::new())),
            known_duplicates: Arc::new(std::sync::RwLock::new(std::collections::HashSet::new())),
            catalog_removals: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    pub fn vault_base(&self) -> &std::path::Path {
        &self.vault_base
    }

    /// Whether the current on-disk bytes at `relative` match the hash this store itself last
    /// wrote there via `write_file`. `false` for anything this store has never written, or
    /// whose bytes have since changed. This is `vault_watch.rs`'s authoritative own-write
    /// filter -- no TTL, unlike `plugins/notify`'s upstream `mark_own_writes` mechanism (see
    /// that module's doc for why a TTL caused a real data-loss incident in the previous
    /// watcher).
    pub fn journal_matches_current_file(&self, relative: &str) -> bool {
        self.journal
            .matches_current_file(&self.vault_base, relative)
    }

    /// Takes the store-wide write lock and hands back the guard, so a read-modify-write
    /// caller can hold it across its own read and the matching `write_file_locked`. Without
    /// this, `write_file`'s internal lock only spans the write, and two callers computing a
    /// new whole-file value from the same starting bytes silently drop one of the updates.
    pub(crate) async fn lock_writes(&self) -> WriteGuard<'_> {
        self.write_lock.lock().await
    }

    pub async fn write_file(&self, relative: PathBuf, bytes: Vec<u8>) -> Result<(), StoreError> {
        let guard = self.lock_writes().await;
        self.write_file_locked(&guard, relative, bytes).await
    }

    /// `write_file` for callers that already hold the write lock (see `lock_writes`). The
    /// guard is a proof token only -- taking the lock again here would deadlock.
    pub(crate) async fn write_file_locked(
        &self,
        _guard: &WriteGuard<'_>,
        relative: PathBuf,
        bytes: Vec<u8>,
    ) -> Result<(), StoreError> {
        validate_relative_path(&relative)?;

        let relative_str = relative
            .to_str()
            .ok_or_else(|| StoreError::Io("invalid relative path".to_string()))?
            .to_string();

        let abs = self.vault_base.join(&relative);
        let parent = abs
            .parent()
            .ok_or_else(|| StoreError::Io("failed to get parent directory".to_string()))?;

        let parent_path = parent.to_path_buf();
        let abs_path = abs.clone();
        let vault_base = self.vault_base.clone();
        let journal = self.journal.clone();
        let journal_relative = relative_str.clone();

        let hash = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&parent_path)
                .map_err(|e| StoreError::Io(format!("failed to create parent directory: {}", e)))?;

            trash_foreign_bytes(&journal, &vault_base, &abs_path, &journal_relative, &bytes)?;

            let tmp_path = hypr_fs_sync_core::export::tmp_sibling_path(&abs_path);
            hypr_storage::fs::write_staged_file(&abs_path, &tmp_path, &bytes)
                .map_err(|e| StoreError::Io(format!("failed to write file atomically: {}", e)))?;

            Ok::<String, StoreError>(sha256(&bytes))
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {}", e)))??;

        self.journal.record(&relative_str, &hash);

        Ok(())
    }

    /// Prepares the store for an external vault relocation (copy or move of the files on
    /// disk) and blocks every writer until the returned guard is dropped.
    ///
    /// Flushes the live transcript buffers first -- those words exist nowhere else, and the
    /// flush takes the write lock internally, so it cannot run while the guard is held.
    /// Refuses when a recording holds a path lease: listener-core's DiskSink writes through
    /// absolute paths into the vault until finalization, so files would keep landing in the
    /// old location mid-copy. Holding the store-wide write lock for the relocation's
    /// duration also blocks `prepare_recording`, so no new lease can appear once this
    /// returns.
    pub async fn freeze_for_vault_move(&self) -> Result<VaultMoveGuard<'_>, StoreError> {
        self.flush_all().await?;
        let guard = self.lock_writes().await;
        if !self.active_recordings.lock().unwrap().is_empty() {
            return Err(StoreError::Conflict(
                "a recording is in progress; stop it before moving the vault".to_string(),
            ));
        }
        // A dirty buffer here means an append raced in between the flush above and taking
        // the lock, or the flush failed -- either way the words are not on disk yet.
        if self.live.lock().await.values().any(|buffer| buffer.dirty) {
            return Err(StoreError::Conflict(
                "transcript data is still being saved; try again in a moment".to_string(),
            ));
        }
        Ok(VaultMoveGuard { _guard: guard })
    }
}

/// Blocks all store writes and new recording-path leases while a vault relocation copies
/// files out from under the store. Dropping it unblocks writers.
pub struct VaultMoveGuard<'a> {
    _guard: WriteGuard<'a>,
}

/// Proof that the caller holds the store-wide write lock.
pub(crate) type WriteGuard<'a> = tokio::sync::MutexGuard<'a, ()>;

/// Preserves bytes at `abs` that this store did not write, before they are overwritten.
///
/// The vault is the only copy of the user's data and other programs (Obsidian, sync
/// clients) edit it while the app runs, so an overwrite that destroys content the store
/// never produced is unrecoverable data loss -- the same reasoning as
/// `hypr_fs_sync_core::export::write_file_atomic`, applied at this store's primitive so
/// every writer (note, meta, docs, transcript, tasks, templates) inherits it.
///
/// The write journal decides ownership: if the on-disk bytes still hash to what this store
/// last wrote to this path, this is our own file and the overwrite is silent -- which is the
/// steady state, so ordinary editing never accumulates trash. Anything else (an external
/// edit, or a file predating this process, since the journal is in-memory) is trashed first.
/// Bytes identical to what we are about to write lose nothing, so they are left alone too.
fn trash_foreign_bytes(
    journal: &journal::WriteJournal,
    vault_base: &std::path::Path,
    abs: &std::path::Path,
    relative: &str,
    next: &[u8],
) -> Result<(), StoreError> {
    if journal.matches_current_file(vault_base, relative) {
        return Ok(());
    }

    match std::fs::read(abs) {
        Ok(existing) if existing == next => return Ok(()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        // An unreadable existing file must be preserved. The backup attempt below
        // fails the write if we cannot safely retain its bytes.
        Err(_) => {}
    }

    hypr_fs_sync_core::export::copy_to_trash(vault_base, abs)
        .map(|_| ())
        .map_err(|e| StoreError::Io(format!("failed to back up overwritten file to trash: {e}")))
}

/// Rejects a vault-relative path that could escape the vault. Guards the id/kind segments
/// every path helper interpolates (`sessions/<id>/<kind>.md`), so a hostile or empty
/// segment can't turn a write into a write outside the vault base.
fn validate_relative_path(relative: &std::path::Path) -> Result<(), StoreError> {
    use std::path::Component;

    if relative.as_os_str().is_empty() {
        return Err(StoreError::Io("empty vault-relative path".to_string()));
    }
    for component in relative.components() {
        match component {
            Component::Normal(segment) if !segment.is_empty() => {}
            _ => {
                return Err(StoreError::Io(format!(
                    "unsafe vault-relative path: {}",
                    relative.display()
                )));
            }
        }
    }
    Ok(())
}

/// A session id becomes a directory name directly under `sessions/`, so it must be a single
/// safe path segment -- same rule (and rationale) as `templates::validate_template_id`. An
/// empty id would make `sessions/<id>` resolve to `sessions/` itself, which
/// `delete_session` would then move the user's entire vault of sessions to trash;
/// an absolute id escapes the vault outright, because `Path::join` with an absolute path
/// replaces rather than appends.
pub(crate) fn validate_session_id(id: &str) -> Result<(), StoreError> {
    validate_path_segment("session id", id)
}

/// Enhanced doc ids become `enhanced/<id>.md`; same rule as session ids.
pub(crate) fn validate_doc_id(id: &str) -> Result<(), StoreError> {
    validate_path_segment("enhanced doc id", id)
}

fn validate_path_segment(kind: &str, id: &str) -> Result<(), StoreError> {
    if id.is_empty()
        || id.starts_with('.')
        || id.contains('/')
        || id.contains('\\')
        || id.contains('\0')
        || std::path::Path::new(id).is_absolute()
    {
        return Err(StoreError::Io(format!("invalid {kind}: {id:?}")));
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output: String, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

// The legacy-exporter frontmatter strip is shared with the read-only vault consumers
// (loof CLI/MCP); see `hypr_vault_read::strip_leading_frontmatter` for the full rationale.
pub(crate) use hypr_vault_read::strip_leading_frontmatter;

#[cfg(test)]
mod tests {
    use super::*;

    /// Reproduces the boot-1 corruption artifact exactly: two nested `vault_export` wrappers
    /// (each with `id`/`position` keys) around real content. A single call must unwrap both
    /// layers, not just the outer one, converging to the true inner content.
    #[test]
    fn strip_leading_frontmatter_unwraps_nested_exporter_layers() {
        let input = "---\nid: s1:note\nposition: 0\nsession_id: s1\n---\n\n\
                     ---\nid: s1\nposition: 0\nsession_id: s1\n---\n\nreal content";
        assert_eq!(strip_leading_frontmatter(input.to_string()), "real content");
    }

    /// A block that parses as well-formed frontmatter but carries neither `id` nor `position`
    /// is not the exporter's wrapper -- it's genuine user/third-party content that merely
    /// opens with a valid-looking `---` block, and must be left completely untouched.
    #[test]
    fn strip_leading_frontmatter_leaves_non_exporter_frontmatter_untouched() {
        let input = "---\ntitle: My Doc\nauthor: me\n---\n\nActual user content.";
        assert_eq!(strip_leading_frontmatter(input.to_string()), input);
    }

    /// A file that is *only* an exporter wrapper (empty body) strips to the empty string, not
    /// to some leftover fragment of the wrapper.
    #[test]
    fn strip_leading_frontmatter_of_an_empty_exporter_wrapper_returns_empty_string() {
        let input = "---\nid: s1:note\nposition: 0\nsession_id: s1\n---\n\n";
        assert_eq!(strip_leading_frontmatter(input.to_string()), "");
    }

    async fn test_store() -> (SessionStore, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path().to_path_buf());
        (store, temp)
    }

    #[tokio::test]
    async fn freeze_for_vault_move_refuses_while_a_recording_lease_is_held() {
        let (store, _temp) = test_store().await;
        store.note_recording_active("s1");

        let err = store
            .freeze_for_vault_move()
            .await
            .err()
            .expect("freeze must refuse while a recording lease is held");
        assert!(matches!(err, StoreError::Conflict(_)), "got: {err}");
    }

    /// The freeze must first land buffered transcript words on disk (they exist nowhere
    /// else), and must hold the store-wide write lock until dropped so no writer can land
    /// files in the old location mid-relocation.
    #[tokio::test]
    async fn freeze_for_vault_move_flushes_buffers_and_blocks_writes_until_dropped() {
        let (store, temp) = test_store().await;
        store
            .append_transcript(
                "s1",
                TranscriptDelta {
                    transcript_id: "t1".to_string(),
                    new_words: vec![hypr_fs_format::TranscriptWord {
                        id: Some("w0".to_string()),
                        text: "move-survivor".to_string(),
                        start_ms: 0.0,
                        end_ms: 0.0,
                        channel: 0.0,
                        speaker: None,
                        metadata: None,
                    }],
                    replaced_ids: vec![],
                    new_hints: vec![],
                    started_at_ms: 1000.0,
                },
            )
            .await
            .unwrap();

        let freeze = store.freeze_for_vault_move().await.unwrap();

        let transcript = temp.path().join("sessions/s1/transcript.json");
        assert!(
            std::fs::read_to_string(&transcript)
                .unwrap()
                .contains("move-survivor")
        );

        let blocked = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            store.write_file(PathBuf::from("sessions/s1/notes.md"), b"late".to_vec()),
        )
        .await;
        assert!(
            blocked.is_err(),
            "write must block while the freeze is held"
        );

        drop(freeze);
        store
            .write_file(PathBuf::from("sessions/s1/notes.md"), b"late".to_vec())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn write_file_creates_parents_and_is_atomic() {
        let (store, temp) = test_store().await;
        let vault = temp.path();
        store
            .write_file(PathBuf::from("sessions/s1/notes.md"), b"hello".to_vec())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(vault.join("sessions/s1/notes.md")).unwrap(),
            b"hello"
        );
        // no tmp leftovers
        assert_eq!(
            std::fs::read_dir(vault.join("sessions/s1"))
                .unwrap()
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn journal_recognizes_own_write_and_external_change() {
        let (store, temp) = test_store().await;
        let vault = temp.path();
        store
            .write_file(PathBuf::from("sessions/s1/notes.md"), b"hello".to_vec())
            .await
            .unwrap();
        assert!(
            store
                .journal
                .matches_current_file(vault, "sessions/s1/notes.md")
        );
        std::fs::write(vault.join("sessions/s1/notes.md"), b"edited outside").unwrap();
        assert!(
            !store
                .journal
                .matches_current_file(vault, "sessions/s1/notes.md")
        );
    }

    fn trash_root(vault: &std::path::Path) -> PathBuf {
        vault
            .join(".trash")
            .join(chrono::Utc::now().format("%Y-%m-%d").to_string())
    }

    /// The critical fix from whole-branch review, at the store's primitive: the vault is the
    /// only copy of the user's data and other programs edit it while the app runs, so bytes
    /// this store did not write must land in `.trash/` before they are overwritten.
    #[tokio::test]
    async fn write_file_trashes_externally_edited_bytes_before_overwriting_them() {
        let (store, temp) = test_store().await;
        let vault = temp.path();
        store
            .write_file(
                PathBuf::from("sessions/s1/notes.md"),
                b"written by the app".to_vec(),
            )
            .await
            .unwrap();

        // Another program (Obsidian, a sync client) rewrites the note behind our back.
        std::fs::write(
            vault.join("sessions/s1/notes.md"),
            b"typed in Obsidian, never seen by the app",
        )
        .unwrap();

        store
            .write_file(
                PathBuf::from("sessions/s1/notes.md"),
                b"app overwrite".to_vec(),
            )
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(vault.join("sessions/s1/notes.md")).unwrap(),
            b"app overwrite"
        );
        let trashed = trash_root(vault).join("sessions/s1/notes.md");
        assert_eq!(
            std::fs::read(&trashed).unwrap(),
            b"typed in Obsidian, never seen by the app",
            "the external edit must be recoverable from .trash"
        );
    }

    /// The journal is in-memory, so a file that predates this process has no entry at all --
    /// which means it is not ours and must be preserved, not silently replaced.
    #[tokio::test]
    async fn write_file_trashes_a_pre_existing_file_this_process_never_wrote() {
        let (store, temp) = test_store().await;
        let vault = temp.path();
        std::fs::create_dir_all(vault.join("sessions/s1")).unwrap();
        std::fs::write(vault.join("sessions/s1/notes.md"), b"from a previous run").unwrap();

        store
            .write_file(PathBuf::from("sessions/s1/notes.md"), b"this run".to_vec())
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(trash_root(vault).join("sessions/s1/notes.md")).unwrap(),
            b"from a previous run"
        );
    }

    /// The other half of the fix, and the one that decides whether it is usable: an
    /// overwrite of bytes this store itself wrote is silent. Getting this backwards would
    /// mean a trash file per keystroke, filling the user's disk with copies of their note.
    #[tokio::test]
    async fn repeated_normal_writes_create_zero_trash_files() {
        let (store, temp) = test_store().await;
        let vault = temp.path();

        for i in 0..25 {
            store
                .write_file(
                    PathBuf::from("sessions/s1/notes.md"),
                    format!("keystroke {i}").into_bytes(),
                )
                .await
                .unwrap();
            store
                .write_file(
                    PathBuf::from("sessions/s1/_meta.json"),
                    format!("{{\"n\":{i}}}").into_bytes(),
                )
                .await
                .unwrap();
            store
                .write_file(
                    PathBuf::from("sessions/s1/transcript.json"),
                    format!("[{i}]").into_bytes(),
                )
                .await
                .unwrap();
        }

        assert!(
            !vault.join(".trash").exists(),
            "normal repeated writes must never produce a trash file"
        );
        assert_eq!(
            std::fs::read(vault.join("sessions/s1/notes.md")).unwrap(),
            b"keystroke 24"
        );
    }

    /// An external write that happens to produce exactly the bytes we are about to write
    /// loses nothing, so it is not worth a trash copy either.
    #[tokio::test]
    async fn write_file_does_not_trash_byte_identical_existing_content() {
        let (store, temp) = test_store().await;
        let vault = temp.path();
        std::fs::create_dir_all(vault.join("sessions/s1")).unwrap();
        std::fs::write(vault.join("sessions/s1/notes.md"), b"same bytes").unwrap();

        store
            .write_file(
                PathBuf::from("sessions/s1/notes.md"),
                b"same bytes".to_vec(),
            )
            .await
            .unwrap();

        assert!(!vault.join(".trash").exists());
    }

    /// A path segment interpolated from frontend input must not be able to walk out of
    /// the vault -- enforced at the primitive so every writer inherits it.
    #[tokio::test]
    async fn write_file_rejects_a_relative_path_that_escapes_the_vault() {
        let (store, temp) = test_store().await;
        let outside = temp.path().parent().unwrap().join("escaped.md");

        let result = store
            .write_file(
                PathBuf::from("sessions/s1/../../../escaped.md"),
                b"pwned".to_vec(),
            )
            .await;

        assert!(result.is_err());
        assert!(!outside.exists());
    }

    #[test]
    fn session_and_doc_ids_must_be_a_single_safe_path_segment() {
        for id in [
            "",
            ".",
            "..",
            "../evil",
            "a/b",
            "a\\b",
            "/Users/me/Documents",
            ".hidden",
        ] {
            assert!(validate_session_id(id).is_err(), "{id:?}");
            assert!(validate_doc_id(id).is_err(), "{id:?}");
        }
        assert!(validate_session_id("01JABCDEF").is_ok());
        assert!(validate_doc_id("doc-1").is_ok());
    }

    #[tokio::test]
    async fn concurrent_writes_to_same_path_maintain_journal_consistency() {
        let (store, temp) = test_store().await;
        let vault = temp.path();
        let store = std::sync::Arc::new(store);

        let store1 = store.clone();
        let store2 = store.clone();
        let task1 = async {
            store1
                .write_file(PathBuf::from("sessions/s1/notes.md"), b"content1".to_vec())
                .await
        };
        let task2 = async {
            store2
                .write_file(PathBuf::from("sessions/s1/notes.md"), b"content2".to_vec())
                .await
        };

        let (r1, r2) = tokio::join!(task1, task2);
        r1.unwrap();
        r2.unwrap();

        assert!(
            store
                .journal
                .matches_current_file(vault, "sessions/s1/notes.md"),
            "journal hash must match whichever content won the race"
        );
    }
}

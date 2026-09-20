use serde::{Deserialize, Serialize};

use super::session_path::DeletedSession;
use super::{SessionStore, StoreError, WriteGuard, paths, validate_session_id};

// The `_meta.json` schema is shared with the read-only vault consumers (loof CLI/MCP);
// the type lives in `hypr-vault-read` so both sides parse the same shape.
pub use hypr_vault_read::SessionMeta;
pub use hypr_vault_read::{TagSuggestionItem, TagSuggestionState, TagSuggestionStatus};

pub fn is_tag_automation_candidate(name: &str) -> bool {
    !name.to_lowercase().contains("import")
}

/// Partial update for `_meta.json`: `None` means "leave as-is", so callers can patch a single
/// field without knowing the rest. There is deliberately no way to clear a field back to
/// absent -- no mutation site needs that today.
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, Default, PartialEq)]
pub struct SessionMetaPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

impl SessionStore {
    pub async fn write_meta(&self, meta: &SessionMeta) -> Result<(), StoreError> {
        validate_session_id(&meta.id)?;
        let guard = self.lock_writes().await;
        self.write_meta_locked(&guard, meta).await
    }

    /// Creation and updates share the canonical path and identity checks.
    pub async fn create_session_meta(&self, meta: &SessionMeta) -> Result<(), StoreError> {
        validate_session_id(&meta.id)?;
        let guard = self.lock_writes().await;
        let dir = self.session_dir(&meta.id).await?;
        self.finish_meta_write_locked(&guard, meta, dir).await
    }

    async fn write_meta_locked(
        &self,
        guard: &WriteGuard<'_>,
        meta: &SessionMeta,
    ) -> Result<(), StoreError> {
        let dir = self.session_dir(&meta.id).await?;
        self.finish_meta_write_locked(guard, meta, dir).await
    }

    async fn finish_meta_write_locked(
        &self,
        guard: &WriteGuard<'_>,
        meta: &SessionMeta,
        dir: std::path::PathBuf,
    ) -> Result<(), StoreError> {
        self.read_meta(&meta.id).await?;
        let meta_json =
            serde_json::to_vec_pretty(meta).map_err(|e| StoreError::Serialize(e.to_string()))?;

        self.write_file_locked(guard, paths::meta_path_in(&dir), meta_json)
            .await?;
        // Index write-through directly after the file write (file truth).
        self.index_upsert_meta(meta);
        self.notify_index_changed(super::IndexEntity::Sessions, vec![meta.id.clone()]);

        Ok(())
    }

    /// Read-modify-write partial update of `_meta.json`, then the same file-first dual-write
    /// as `write_meta`. Errors (rather than synthesizing a fresh meta) when the session has no
    /// `_meta.json`: every store-created session has one, and inventing one here would let a
    /// title edit racing a delete quietly resurrect the session folder.
    ///
    /// The write lock is held across the read *and* the write: two concurrent patches that
    /// both read the pre-patch meta would otherwise each write a whole file back, and the
    /// loser's fields would vanish.
    pub async fn update_meta(&self, id: &str, patch: SessionMetaPatch) -> Result<(), StoreError> {
        validate_session_id(id)?;
        let guard = self.lock_writes().await;

        let mut meta = self
            .read_meta(id)
            .await?
            .ok_or_else(|| StoreError::Io(format!("session {id} has no _meta.json to update")))?;

        let SessionMetaPatch {
            title,
            started_at,
            ended_at,
            created_at,
            tags,
            tracking_id,
            folder,
            author,
        } = patch;

        if let Some(title) = title {
            meta.title = title;
        }
        if let Some(started_at) = started_at {
            meta.started_at = Some(started_at);
        }
        if let Some(ended_at) = ended_at {
            meta.ended_at = Some(ended_at);
        }
        if let Some(created_at) = created_at {
            meta.created_at = created_at;
        }
        if let Some(tags) = tags {
            meta.tags = tags;
            if let Some(suggestions) = &mut meta.tag_suggestions {
                suggestions
                    .items
                    .retain(|suggestion| !meta.tags.contains(&suggestion.name));
            }
        }
        if let Some(tracking_id) = tracking_id {
            meta.tracking_id = Some(tracking_id);
        }
        if let Some(folder) = folder {
            meta.folder = Some(folder);
        }
        if let Some(author) = author {
            meta.author = Some(author);
        }

        self.write_meta_locked(&guard, &meta).await
    }

    pub async fn mark_tag_suggestions_pending(
        &self,
        id: &str,
        source_hash: String,
        algorithm_version: u32,
    ) -> Result<bool, StoreError> {
        validate_session_id(id)?;
        let guard = self.lock_writes().await;
        let mut meta = self
            .read_meta(id)
            .await?
            .ok_or_else(|| StoreError::Io(format!("session {id} has no _meta.json to update")))?;

        if let Some(state) = meta.tag_suggestions.as_ref().filter(|state| {
            state.source_hash == source_hash && state.algorithm_version == algorithm_version
        }) {
            return Ok(state.status == TagSuggestionStatus::Pending);
        }

        let dismissed = meta
            .tag_suggestions
            .as_ref()
            .filter(|state| state.algorithm_version == algorithm_version)
            .map(|state| state.dismissed.clone())
            .unwrap_or_default();
        meta.tag_suggestions = Some(TagSuggestionState {
            source_hash,
            algorithm_version,
            status: TagSuggestionStatus::Pending,
            items: Vec::new(),
            dismissed,
        });
        self.write_meta_locked(&guard, &meta).await?;
        Ok(true)
    }

    pub async fn complete_tag_suggestions(
        &self,
        id: &str,
        source_hash: &str,
        algorithm_version: u32,
        suggestions: Vec<TagSuggestionItem>,
        auto_accept_threshold: Option<f32>,
    ) -> Result<bool, StoreError> {
        validate_session_id(id)?;
        let guard = self.lock_writes().await;
        let mut meta = self
            .read_meta(id)
            .await?
            .ok_or_else(|| StoreError::Io(format!("session {id} has no _meta.json to update")))?;

        let Some(current) = &meta.tag_suggestions else {
            return Ok(false);
        };
        if current.source_hash != source_hash
            || current.algorithm_version != algorithm_version
            || current.status != TagSuggestionStatus::Pending
        {
            return Ok(false);
        }

        let dismissed = current.dismissed.clone();
        let mut remaining = Vec::new();
        for suggestion in suggestions {
            if !is_tag_automation_candidate(&suggestion.name)
                || meta.tags.contains(&suggestion.name)
                || dismissed.contains(&suggestion.name)
            {
                continue;
            }
            if auto_accept_threshold.is_some_and(|threshold| suggestion.confidence >= threshold) {
                meta.tags.push(suggestion.name);
            } else {
                remaining.push(suggestion);
            }
        }
        meta.tags.sort();
        meta.tags.dedup();
        meta.tag_suggestions = Some(TagSuggestionState {
            source_hash: source_hash.to_string(),
            algorithm_version,
            status: TagSuggestionStatus::Complete,
            items: remaining,
            dismissed,
        });
        self.write_meta_locked(&guard, &meta).await?;
        Ok(true)
    }

    pub async fn accept_tag_suggestion(&self, id: &str, name: &str) -> Result<bool, StoreError> {
        validate_session_id(id)?;
        let Some(name) = hypr_vault_read::normalize_tag_name(name) else {
            return Err(StoreError::Io("tag name cannot be empty".to_string()));
        };
        let guard = self.lock_writes().await;
        let mut meta = self
            .read_meta(id)
            .await?
            .ok_or_else(|| StoreError::Io(format!("session {id} has no _meta.json to update")))?;
        let Some(state) = &mut meta.tag_suggestions else {
            return Ok(false);
        };
        let before = state.items.len();
        state.items.retain(|suggestion| suggestion.name != name);
        if before == state.items.len() {
            return Ok(false);
        }
        if !meta.tags.contains(&name) {
            meta.tags.push(name);
            meta.tags.sort();
        }
        self.write_meta_locked(&guard, &meta).await?;
        Ok(true)
    }

    pub async fn dismiss_tag_suggestion(&self, id: &str, name: &str) -> Result<bool, StoreError> {
        validate_session_id(id)?;
        let Some(name) = hypr_vault_read::normalize_tag_name(name) else {
            return Err(StoreError::Io("tag name cannot be empty".to_string()));
        };
        let guard = self.lock_writes().await;
        let mut meta = self
            .read_meta(id)
            .await?
            .ok_or_else(|| StoreError::Io(format!("session {id} has no _meta.json to update")))?;
        let Some(state) = &mut meta.tag_suggestions else {
            return Ok(false);
        };
        let before = state.items.len();
        state.items.retain(|suggestion| suggestion.name != name);
        if before == state.items.len() {
            return Ok(false);
        }
        if !state.dismissed.contains(&name) {
            state.dismissed.push(name);
            state.dismissed.sort();
        }
        self.write_meta_locked(&guard, &meta).await?;
        Ok(true)
    }

    /// Recording lifecycle stamps for `_meta.json`, driven by capture start/stop events.
    /// `started_at` keeps the first recording's start -- a paused-and-resumed recording
    /// must not shift the session forward on the timeline -- while `ended_at` always
    /// advances to the latest stop, so together they span every take in the session.
    /// Errors on a missing `_meta.json` for the same reason as `update_meta`: a stamp
    /// racing a delete must not resurrect the session folder.
    pub async fn mark_recording_started(&self, id: &str, at: &str) -> Result<(), StoreError> {
        validate_session_id(id)?;
        let guard = self.lock_writes().await;

        // Tracked even if the stamp below fails: the recorder holds paths into the
        // directory either way, so whole-vault relocation must stay blocked.
        // Ensure-at-least-one (never stack): a `prepare_recording` lease for this
        // same recording may already be counted.
        self.active_recordings
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_insert(1);

        let mut meta = self
            .read_meta(id)
            .await?
            .ok_or_else(|| StoreError::Io(format!("session {id} has no _meta.json to update")))?;

        if meta.started_at.is_some() {
            return Ok(());
        }
        meta.started_at = Some(at.to_string());
        self.write_meta_locked(&guard, &meta).await
    }

    pub async fn mark_recording_ended(&self, id: &str, at: &str) -> Result<(), StoreError> {
        validate_session_id(id)?;
        let guard = self.lock_writes().await;

        // A resumed capture may already hold another reservation while this stamp
        // waits in the ordered lifecycle queue.
        self.release_recording_reservation(id);
        self.notify_artifacts_changed(id);

        let mut meta = self
            .read_meta(id)
            .await?
            .ok_or_else(|| StoreError::Io(format!("session {id} has no _meta.json to update")))?;

        meta.ended_at = Some(at.to_string());
        self.write_meta_locked(&guard, &meta).await
    }

    pub async fn read_meta(&self, id: &str) -> Result<Option<SessionMeta>, StoreError> {
        validate_session_id(id)?;
        let vault = self.vault_base.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            hypr_vault_read::meta::read_session_meta(&vault, &id).map_err(StoreError::from)
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {e}")))?
    }

    pub async fn write_note(&self, id: &str, markdown: &str) -> Result<(), StoreError> {
        validate_session_id(id)?;
        let note_bytes = markdown.as_bytes().to_vec();
        let guard = self.lock_writes().await;
        let dir = self.session_dir_locked(&guard, id).await?;
        self.write_file_locked(&guard, paths::note_path_in(&dir), note_bytes)
            .await?;

        // Migrate-on-first-edit: once `notes.md` lands, a leftover pre-rename `_memo.md`
        // would only ever be the stale copy (readers prefer `notes.md`), and an external
        // editor could still pick the wrong file -- so move it to trash (hand-recoverable,
        // never synced), same as any other superseded file. Sessions never edited keep
        // their `_memo.md` and stay readable through the fallback.
        // Best-effort: `notes.md` already landed and wins on every read path, so a failed
        // trash move must not fail the write (the index update below would be skipped and,
        // with the write journaled, no rescan would ever repair it) -- retry next write.
        let legacy_abs = self.vault_base.join(paths::legacy_note_path_in(&dir));
        if legacy_abs.is_file() {
            let vault_base = self.vault_base.clone();
            let moved = tokio::task::spawn_blocking(move || {
                hypr_fs_sync_core::export::move_to_trash(&vault_base, &legacy_abs)
            })
            .await;
            match moved {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(session_id = %id, %error, "failed to move legacy note to trash; keeping it for the next write");
                }
                Err(error) => {
                    tracing::warn!(session_id = %id, %error, "failed to move legacy note to trash; keeping it for the next write");
                }
            }
        }
        drop(guard);

        // Store what `read_note` would return, not the raw bytes: a body that starts with an
        // exporter-shaped frontmatter block would otherwise sit un-stripped in the index and
        // change under the user on the next rescan.
        self.index_set_note(
            id,
            Some(super::strip_leading_frontmatter(markdown.to_string())),
        );
        self.notify_index_changed(super::IndexEntity::Sessions, vec![id.to_string()]);

        Ok(())
    }

    pub async fn read_note(&self, id: &str) -> Result<Option<String>, StoreError> {
        validate_session_id(id)?;
        let dir = self.session_dir(id).await?;
        let vault_base = self.vault_base.clone();

        let result = tokio::task::spawn_blocking(move || -> Result<Option<String>, StoreError> {
            // `notes.md` first, then the pre-rename `_memo.md` -- `notes.md` always wins
            // when both exist (the store only writes `notes.md`; see write_note).
            for path in [
                vault_base.join(paths::note_path_in(&dir)),
                vault_base.join(paths::legacy_note_path_in(&dir)),
            ] {
                // Same attempt-then-match rationale as read_meta above.
                match std::fs::read_to_string(&path) {
                    Ok(content) => return Ok(Some(super::strip_leading_frontmatter(content))),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => {
                        return Err(StoreError::Io(format!("failed to read note file: {}", e)));
                    }
                }
            }
            Ok(None)
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {}", e)))??;

        Ok(result)
    }

    /// Move a validated session directory to recoverable trash and return its exact
    /// destination. A missing session returns `None`; invalid metadata or a failed
    /// move leaves the source and in-memory state intact. Undo remains process-local.
    pub async fn delete_session(&self, id: &str) -> Result<Option<std::path::PathBuf>, StoreError> {
        validate_session_id(id)?;
        let _operation = self.transcript_operations.lock(id).await;
        let guard = self.lock_writes().await;
        let relative_dir = self.session_dir_locked(&guard, id).await?;

        let vault_base = self.vault_base.clone();
        let trash_path = tokio::task::spawn_blocking(
            move || -> Result<Option<std::path::PathBuf>, StoreError> {
                let session_path = vault_base.join(&relative_dir);
                for path in [
                    vault_base.join("sessions"),
                    session_path.clone(),
                    session_path.join("_meta.json"),
                ] {
                    match std::fs::symlink_metadata(&path) {
                        Ok(meta) if meta.file_type().is_symlink() => {
                            return Err(StoreError::Io(format!(
                                "refusing to delete a symlinked session: {}",
                                path.display()
                            )));
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            return Ok(None);
                        }
                        Err(error) => return Err(StoreError::Io(error.to_string())),
                    }
                }
                if hypr_vault_read::meta::read_session_meta_in(&vault_base, &relative_dir)?
                    .is_none()
                {
                    return Ok(None);
                }
                hypr_fs_sync_core::export::move_to_trash(&vault_base, &session_path)
                    .map_err(|e| StoreError::Io(format!("failed to move session to trash: {e}")))
            },
        )
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {e}")))??;

        if let Some(trash_path) = &trash_path {
            self.live.lock().await.remove(id);
            self.recent_deletions.lock().unwrap().insert(
                id.to_string(),
                DeletedSession {
                    trash_path: trash_path.clone(),
                },
            );
            self.deleted_sessions.lock().unwrap().insert(id.to_string());
            self.index_remove_session_and_notify(id);
        }
        Ok(trash_path)
    }

    /// Undoes a `delete_session` from this process: renames the exact trashed directory
    /// back to its original relative path. Backed only by the in-memory recent-deletions
    /// map -- the undo toast is process-local and short-lived, so there is no on-disk
    /// tombstone; after a restart the trashed directory remains available for manual
    /// recovery. `Ok(false)` (not an error) when there's nothing to restore: no record,
    /// or the trash entry has since disappeared.
    pub async fn restore_session(&self, id: &str) -> Result<bool, StoreError> {
        validate_session_id(id)?;
        let Some(record) = self.recent_deletions.lock().unwrap().get(id).cloned() else {
            return Ok(false);
        };

        let guard = self.lock_writes().await;
        let vault_base = self.vault_base.clone();
        let id_owned = id.to_string();
        let deletion = record.clone();
        let restored = tokio::task::spawn_blocking(move || -> Result<bool, StoreError> {
            // The trash entry must still be this session: a parseable `_meta.json`
            // claiming the requested full id. A vanished entry is an expired undo; a
            // tampered one fails loudly rather than restoring someone else's bytes.
            let meta_bytes = match std::fs::read(deletion.trash_path.join("_meta.json")) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(e) => {
                    return Err(StoreError::Io(format!(
                        "failed to read trashed session meta: {e}"
                    )));
                }
            };
            let meta: SessionMeta = serde_json::from_slice(&meta_bytes).map_err(|e| {
                StoreError::Serialize(format!(
                    "trashed session at {} is not restorable: {e}",
                    deletion.trash_path.display()
                ))
            })?;
            if meta.id != id_owned {
                return Err(StoreError::Io(format!(
                    "trashed directory {} claims session id {:?}, not {:?}; refusing to restore",
                    deletion.trash_path.display(),
                    meta.id,
                    id_owned
                )));
            }

            let destination = vault_base.join(paths::validated_session_dir(&id_owned)?);
            // Never merge onto an occupied destination -- fail safely and leave the
            // trash entry for manual recovery.
            if destination.exists() {
                return Err(StoreError::Io(format!(
                    "restore destination {} is already occupied",
                    destination.display()
                )));
            }
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| StoreError::Io(format!("failed to create parent dir: {}", e)))?;
            }
            hypr_storage::fs::rename_no_replace(&deletion.trash_path, &destination).map_err(
                |e| StoreError::Io(format!("failed to restore session from trash: {}", e)),
            )?;
            Ok(true)
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {}", e)))??;

        self.recent_deletions.lock().unwrap().remove(id);
        if restored {
            self.deleted_sessions.lock().unwrap().remove(id);
        }
        drop(guard);

        if restored {
            self.refresh_session(id).await?;
        }

        Ok(restored)
    }
}

#[cfg(test)]
mod tests {
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

    async fn test_store() -> (SessionStore, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().to_path_buf();
        let store = SessionStore::new(vault);
        (store, temp)
    }

    async fn session_path(
        store: &SessionStore,
        vault: &tempfile::TempDir,
        id: &str,
    ) -> std::path::PathBuf {
        vault.path().join(store.session_dir(id).await.unwrap())
    }

    #[tokio::test]
    async fn write_meta_writes_file_and_index() {
        let (store, vault) = test_store().await;
        store
            .write_meta(&meta("s1", "Jury feedback"))
            .await
            .unwrap();
        assert!(
            session_path(&store, &vault, "s1")
                .await
                .join("_meta.json")
                .is_file()
        );
        assert_eq!(store.session_get("s1").unwrap().meta.title, "Jury feedback");
        assert_eq!(
            store.read_meta("s1").await.unwrap().unwrap().title,
            "Jury feedback"
        );
    }

    #[tokio::test]
    async fn tag_suggestions_are_persisted_and_explicitly_resolved() {
        let (store, _) = test_store().await;
        store.write_meta(&meta("s1", "Atlas launch")).await.unwrap();

        assert!(
            store
                .mark_tag_suggestions_pending("s1", "hash-1".to_string(), 1)
                .await
                .unwrap()
        );
        assert!(
            store
                .complete_tag_suggestions(
                    "s1",
                    "hash-1",
                    1,
                    vec![
                        TagSuggestionItem {
                            name: "project/atlas".to_string(),
                            confidence: 0.8,
                        },
                        TagSuggestionItem {
                            name: "customer/acme".to_string(),
                            confidence: 0.6,
                        },
                    ],
                    None,
                )
                .await
                .unwrap()
        );

        assert!(
            store
                .accept_tag_suggestion("s1", "project/atlas")
                .await
                .unwrap()
        );
        assert!(
            store
                .dismiss_tag_suggestion("s1", "customer/acme")
                .await
                .unwrap()
        );
        let meta = store.read_meta("s1").await.unwrap().unwrap();
        assert_eq!(meta.tags, vec!["project/atlas"]);
        let state = meta.tag_suggestions.unwrap();
        assert_eq!(state.items, Vec::new());
        assert_eq!(state.dismissed, vec!["customer/acme"]);

        store
            .mark_tag_suggestions_pending("s1", "hash-2".to_string(), 1)
            .await
            .unwrap();
        store
            .complete_tag_suggestions(
                "s1",
                "hash-2",
                1,
                vec![TagSuggestionItem {
                    name: "customer/acme".to_string(),
                    confidence: 0.9,
                }],
                None,
            )
            .await
            .unwrap();
        assert!(
            store
                .read_meta("s1")
                .await
                .unwrap()
                .unwrap()
                .tag_suggestions
                .unwrap()
                .items
                .is_empty()
        );
    }

    #[tokio::test]
    async fn stale_tag_suggestion_results_do_not_overwrite_new_work() {
        let (store, _) = test_store().await;
        store.write_meta(&meta("s1", "Atlas launch")).await.unwrap();
        store
            .mark_tag_suggestions_pending("s1", "new-hash".to_string(), 1)
            .await
            .unwrap();

        assert!(
            !store
                .complete_tag_suggestions(
                    "s1",
                    "old-hash",
                    1,
                    vec![TagSuggestionItem {
                        name: "project/atlas".to_string(),
                        confidence: 0.9,
                    }],
                    None,
                )
                .await
                .unwrap()
        );
        let state = store
            .read_meta("s1")
            .await
            .unwrap()
            .unwrap()
            .tag_suggestions
            .unwrap();
        assert_eq!(state.status, TagSuggestionStatus::Pending);
        assert_eq!(state.source_hash, "new-hash");
    }

    #[tokio::test]
    async fn auto_accept_ignores_import_tags_and_keeps_lower_confidence_suggestions_pending() {
        let (store, _) = test_store().await;
        store.write_meta(&meta("s1", "Atlas launch")).await.unwrap();
        store
            .mark_tag_suggestions_pending("s1", "hash-1".to_string(), 1)
            .await
            .unwrap();
        store
            .complete_tag_suggestions(
                "s1",
                "hash-1",
                1,
                vec![
                    TagSuggestionItem {
                        name: "project/atlas".to_string(),
                        confidence: 0.9,
                    },
                    TagSuggestionItem {
                        name: "customer/acme".to_string(),
                        confidence: 0.6,
                    },
                    TagSuggestionItem {
                        name: "Imported".to_string(),
                        confidence: 0.95,
                    },
                    TagSuggestionItem {
                        name: "project/import-review".to_string(),
                        confidence: 0.6,
                    },
                ],
                Some(0.75),
            )
            .await
            .unwrap();

        let meta = store.read_meta("s1").await.unwrap().unwrap();
        assert_eq!(meta.tags, vec!["project/atlas"]);
        assert_eq!(
            meta.tag_suggestions.unwrap().items,
            vec![TagSuggestionItem {
                name: "customer/acme".to_string(),
                confidence: 0.6,
            }]
        );
    }

    #[tokio::test]
    async fn create_session_meta_writes_a_canonical_dir_and_indexes_without_a_scan() {
        let (store, vault) = test_store().await;
        store
            .create_session_meta(&meta("s1", "Jury feedback"))
            .await
            .unwrap();

        let dir = session_path(&store, &vault, "s1").await;
        assert!(dir.join("_meta.json").is_file());
        assert_eq!(dir, vault.path().join("sessions/s1"));
        assert_eq!(store.session_get("s1").unwrap().meta.title, "Jury feedback");
        assert_eq!(
            store.read_meta("s1").await.unwrap().unwrap().title,
            "Jury feedback"
        );
    }

    #[tokio::test]
    async fn create_session_meta_adopts_a_legacy_ghost_dir() {
        let (store, vault) = test_store().await;
        // Recorder fallback: artifacts can land in `sessions/<id>` before the
        // first meta write; creation must adopt that directory, not orphan it.
        let ghost = vault.path().join("sessions/s1");
        std::fs::create_dir_all(&ghost).unwrap();
        std::fs::write(ghost.join("transcript.json"), "{}").unwrap();

        store
            .create_session_meta(&meta("s1", "Recovered"))
            .await
            .unwrap();
        assert_eq!(session_path(&store, &vault, "s1").await, ghost);
        assert!(ghost.join("_meta.json").is_file());
    }

    #[tokio::test]
    async fn create_session_meta_reuses_the_legacy_dir_when_it_claims_the_id() {
        let (store, vault) = test_store().await;
        let dir = vault.path().join("sessions/s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("_meta.json"),
            serde_json::to_vec_pretty(&meta("s1", "Existing")).unwrap(),
        )
        .unwrap();
        // A corrupt legacy meta must also keep its directory rather than mint a
        // sibling — same tolerance as the scanning lookup.
        let corrupt = vault.path().join("sessions/s2");
        std::fs::create_dir_all(&corrupt).unwrap();
        std::fs::write(corrupt.join("_meta.json"), "{ invalid").unwrap();

        store
            .create_session_meta(&meta("s1", "Rewritten"))
            .await
            .unwrap();
        assert!(
            store
                .create_session_meta(&meta("s2", "Repaired"))
                .await
                .is_err()
        );

        assert_eq!(session_path(&store, &vault, "s1").await, dir);
        assert_eq!(
            store.read_meta("s1").await.unwrap().unwrap().title,
            "Rewritten"
        );
        assert_eq!(session_path(&store, &vault, "s2").await, corrupt);
        assert_eq!(
            std::fs::read_to_string(corrupt.join("_meta.json")).unwrap(),
            "{ invalid"
        );
    }

    #[tokio::test]
    async fn create_session_meta_twice_for_one_id_keeps_a_single_directory() {
        let (store, vault) = test_store().await;
        store
            .create_session_meta(&meta("s1", "First"))
            .await
            .unwrap();
        let dir = session_path(&store, &vault, "s1").await;
        store
            .create_session_meta(&meta("s1", "Second"))
            .await
            .unwrap();

        assert_eq!(session_path(&store, &vault, "s1").await, dir);
        assert_eq!(
            store.read_meta("s1").await.unwrap().unwrap().title,
            "Second"
        );
        let dirs = std::fs::read_dir(vault.path().join("sessions"))
            .unwrap()
            .count();
        assert_eq!(dirs, 1, "a re-create must never mint a sibling directory");
    }

    #[tokio::test]
    async fn write_meta_round_trips_tracking_folder_and_tags_through_file_and_index() {
        let (store, vault) = test_store().await;
        let mut m = meta("s1", "Sprint sync");
        m.tracking_id = Some("evt-1".to_string());
        // A legacy calendar-event envelope rides the `extra` catch-all.
        m.extra.insert(
            "event".to_string(),
            serde_json::json!({"meeting_link": "https://example.com/x"}),
        );
        m.folder = Some("work/standups".to_string());
        m.tags = vec!["planning".to_string(), "q3".to_string()];
        store.write_meta(&m).await.unwrap();

        assert_eq!(store.read_meta("s1").await.unwrap().unwrap(), m);

        let indexed = store.session_get("s1").unwrap().meta;
        assert_eq!(indexed.tracking_id, m.tracking_id);
        assert_eq!(indexed.extra.get("event"), m.extra.get("event"));
        assert_eq!(indexed.folder.as_deref(), Some("work/standups"));

        let raw =
            std::fs::read_to_string(session_path(&store, &vault, "s1").await.join("_meta.json"))
                .unwrap();
        assert!(raw.contains("planning"));
    }

    /// Old `_meta.json` files (written before `tracking_id`/`folder` existed) must keep
    /// deserializing -- the new fields default to absent, not an error.
    #[tokio::test]
    async fn read_meta_accepts_pre_tracking_folder_files() {
        let (store, vault) = test_store().await;
        let dir = vault.path().join("sessions/s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("_meta.json"),
            br#"{"id":"s1","title":"Old","started_at":null,"ended_at":null,"created_at":"2026-07-01T00:00:00Z","tags":[]}"#,
        )
        .unwrap();

        let m = store.read_meta("s1").await.unwrap().unwrap();
        assert_eq!(m.tracking_id, None);
        assert_eq!(m.folder, None);

        store.write_meta(&m).await.unwrap();
        let indexed = store.session_get("s1").unwrap().meta;
        assert_eq!(indexed.tracking_id, None);
        assert_eq!(indexed.folder, None);
    }

    #[tokio::test]
    async fn update_meta_patches_only_the_given_fields() {
        let (store, _vault) = test_store().await;
        let mut m = meta("s1", "Original");
        m.tracking_id = Some("evt-1".to_string());
        store.write_meta(&m).await.unwrap();

        store
            .update_meta(
                "s1",
                SessionMetaPatch {
                    title: Some("Renamed".to_string()),
                    tags: Some(vec!["kept".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let after = store.read_meta("s1").await.unwrap().unwrap();
        assert_eq!(after.title, "Renamed");
        assert_eq!(after.tags, vec!["kept".to_string()]);
        assert_eq!(
            after.tracking_id, m.tracking_id,
            "unpatched fields must survive"
        );
        assert_eq!(after.created_at, m.created_at);

        assert_eq!(
            store.session_get("s1").unwrap().meta.title,
            "Renamed",
            "write-through must reach the index"
        );
    }

    /// `author` marks not-vault-owner notes (agents); it must survive unrelated patches
    /// and reach both list projections, or the "not written by you" UI silently lies.
    #[tokio::test]
    async fn author_round_trips_and_survives_unrelated_patches() {
        let (store, _vault) = test_store().await;
        let mut m = meta("s1", "Agent note");
        m.author = Some("claude-code".to_string());
        store.write_meta(&m).await.unwrap();

        store
            .update_meta(
                "s1",
                SessionMetaPatch {
                    title: Some("Renamed".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let after = store.read_meta("s1").await.unwrap().unwrap();
        assert_eq!(after.author.as_deref(), Some("claude-code"));
        assert_eq!(
            store.session_get("s1").unwrap().meta.author.as_deref(),
            Some("claude-code")
        );
        let headers = store.session_list_headers();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].author.as_deref(), Some("claude-code"));

        store
            .update_meta(
                "s1",
                SessionMetaPatch {
                    author: Some("other-agent".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .read_meta("s1")
                .await
                .unwrap()
                .unwrap()
                .author
                .as_deref(),
            Some("other-agent")
        );
    }

    /// A patch against a session with no `_meta.json` must fail loudly instead of inventing
    /// one -- otherwise a title edit racing a delete would resurrect the session folder.
    #[tokio::test]
    async fn update_meta_errors_when_meta_file_is_missing() {
        let (store, vault) = test_store().await;
        let result = store
            .update_meta(
                "ghost",
                SessionMetaPatch {
                    title: Some("nope".to_string()),
                    ..Default::default()
                },
            )
            .await;
        assert!(result.is_err());
        assert!(!vault.path().join("sessions/ghost").exists());
    }

    #[tokio::test]
    async fn write_note_writes_file_and_index() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_note("s1", "# Meeting notes\n\nDiscussed: X, Y, Z")
            .await
            .unwrap();
        assert!(
            session_path(&store, &vault, "s1")
                .await
                .join("notes.md")
                .is_file()
        );
        assert_eq!(
            store.session_get("s1").unwrap().note_markdown.as_deref(),
            Some("# Meeting notes\n\nDiscussed: X, Y, Z")
        );
        assert_eq!(
            store.read_note("s1").await.unwrap().unwrap(),
            "# Meeting notes\n\nDiscussed: X, Y, Z"
        );
    }

    #[tokio::test]
    async fn write_note_indexes_what_read_note_would_return() {
        // An exporter-shaped frontmatter block is stripped on read, so it must be stripped on
        // the write-through too -- otherwise the index and the file disagree until the next
        // rescan, at which point the displayed note silently changes under the user.
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_note("s1", "---\nid: legacy-1\nposition: 0\n---\n\nReal body")
            .await
            .unwrap();

        let indexed = store.session_get("s1").unwrap().note_markdown;
        let on_read = store.read_note("s1").await.unwrap();
        assert_eq!(
            indexed, on_read,
            "index must hold exactly what read_note returns"
        );
        assert_eq!(indexed.as_deref(), Some("Real body"));
    }

    #[tokio::test]
    async fn read_note_falls_back_to_legacy_memo_and_prefers_notes_md() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        let dir = session_path(&store, &vault, "s1").await;

        std::fs::write(dir.join("_memo.md"), "legacy note").unwrap();
        assert_eq!(store.read_note("s1").await.unwrap().unwrap(), "legacy note");

        std::fs::write(dir.join("notes.md"), "current note").unwrap();
        assert_eq!(
            store.read_note("s1").await.unwrap().unwrap(),
            "current note",
            "notes.md must win when both files exist"
        );
    }

    #[tokio::test]
    async fn write_note_migrates_legacy_memo_file_to_trash() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        let dir = session_path(&store, &vault, "s1").await;
        std::fs::write(dir.join("_memo.md"), "pre-rename note").unwrap();

        store.write_note("s1", "edited note").await.unwrap();

        assert!(dir.join("notes.md").is_file());
        assert!(
            !dir.join("_memo.md").exists(),
            "the pre-rename file must not linger where an external editor could pick it"
        );
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let rel = store.session_dir("s1").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(
                vault
                    .path()
                    .join(".trash")
                    .join(&date)
                    .join(&rel)
                    .join("_memo.md")
            )
            .unwrap(),
            "pre-rename note",
            "the legacy note must be hand-recoverable from .trash"
        );
        assert_eq!(store.read_note("s1").await.unwrap().unwrap(), "edited note");
    }

    /// A failed legacy-note trash move must not fail the note write: `notes.md` already
    /// landed (journaled, so no rescan would repair a skipped index update) and wins on
    /// every read path, so the migration just retries on a later write.
    #[tokio::test]
    async fn write_note_succeeds_even_when_legacy_memo_cannot_be_trashed() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        let dir = session_path(&store, &vault, "s1").await;
        std::fs::write(dir.join("_memo.md"), "pre-rename note").unwrap();
        // A regular file at `.trash` makes the dated-trash-dir creation fail.
        std::fs::write(vault.path().join(".trash"), b"not a directory").unwrap();

        store.write_note("s1", "edited note").await.unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.join("notes.md")).unwrap(),
            "edited note"
        );
        assert!(
            dir.join("_memo.md").is_file(),
            "migration is deferred, not silently dropped"
        );
        assert_eq!(
            store.session_get("s1").unwrap().note_markdown.as_deref(),
            Some("edited note"),
            "the index must reflect the successful write"
        );
    }

    #[tokio::test]
    async fn delete_session_moves_folder_to_trash_and_clears_index() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Test Session")).await.unwrap();
        store.write_note("s1", "Some notes").await.unwrap();
        store
            .write_enhanced_doc(&hypr_vault_read::EnhancedDoc {
                id: "doc-1".to_string(),
                session_id: "s1".to_string(),
                kind: "summary".to_string(),
                title: "Summary".to_string(),
                template_id: String::new(),
                sort_order: 0,
                markdown: "Summary content".to_string(),
            })
            .await
            .unwrap();

        store
            .write_transcript(
                "s1",
                hypr_fs_format::TranscriptWithData {
                    id: "t1".to_string(),
                    user_id: String::new(),
                    created_at: "2026-07-24T00:00:00Z".to_string(),
                    session_id: "s1".to_string(),
                    started_at: 0.0,
                    ended_at: None,
                    memo_md: String::new(),
                    words: vec![],
                    speaker_hints: vec![],
                },
            )
            .await
            .unwrap();

        let rel = store.session_dir("s1").await.unwrap();
        let dir = vault.path().join(&rel);
        assert!(dir.is_dir());
        let trash = store.delete_session("s1").await.unwrap().unwrap();
        assert!(trash.join("_meta.json").is_file());

        assert!(!dir.is_dir());

        assert!(store.session_get("s1").is_none());
        assert!(store.session_enhanced_docs("s1").is_empty());
        assert!(store.session_transcripts("s1").await.unwrap().is_empty());

        // The whole folder moved to .trash/<date>/<its vault-relative path>.
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert!(
            vault
                .path()
                .join(".trash")
                .join(&date)
                .join(&rel)
                .join("_meta.json")
                .is_file(),
            "trashed session's _meta.json should exist under .trash/<date>/{}",
            rel.display()
        );
    }

    #[tokio::test]
    async fn failed_delete_keeps_live_buffer_index_and_recording_reservation() {
        let (store, vault) = test_store().await;
        store
            .write_meta(&meta("s1", "Keep recording"))
            .await
            .unwrap();
        store.note_recording_active("s1");
        store.live.lock().await.insert(
            "s1".to_string(),
            super::super::transcript::LiveTranscriptBuffer {
                transcript_id: "pending".to_string(),
                dirty: true,
                ..Default::default()
            },
        );
        std::fs::write(vault.path().join(".trash"), b"obstruction").unwrap();

        assert!(store.delete_session("s1").await.is_err());
        assert!(store.session_get("s1").is_some());
        assert!(store.is_recording("s1"));
        assert!(store.live.lock().await.get("s1").unwrap().dirty);
        assert!(!store.deleted_sessions.lock().unwrap().contains("s1"));
        assert!(!store.recent_deletions.lock().unwrap().contains_key("s1"));
        assert!(store.read_meta("s1").await.unwrap().is_some());
    }

    /// `sessions/<id>` for an empty id is `sessions/` itself, so an unguarded delete would
    /// move the user's ENTIRE session tree to trash in one call -- and `restore_session("")`
    /// could never bring it back, because it looks for `sessions/` *inside* the trashed
    /// sessions dir.
    #[tokio::test]
    async fn delete_session_with_an_empty_id_is_rejected_and_spares_the_whole_sessions_tree() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Keep me")).await.unwrap();
        store.write_meta(&meta("s2", "Me too")).await.unwrap();

        assert!(store.delete_session("").await.is_err());

        assert!(
            session_path(&store, &vault, "s1")
                .await
                .join("_meta.json")
                .is_file()
        );
        assert!(
            session_path(&store, &vault, "s2")
                .await
                .join("_meta.json")
                .is_file()
        );
        assert!(!vault.path().join(".trash").exists());
        assert!(store.session_get("s1").is_some());
    }

    /// `Path::join` with an absolute path replaces rather than appends, so an absolute id
    /// escapes the vault entirely -- and `move_to_trash`'s `strip_prefix` fails open on it.
    #[tokio::test]
    async fn delete_session_with_an_absolute_id_is_rejected_and_touches_nothing_outside_the_vault()
    {
        let (store, vault) = test_store().await;
        let outside = vault.path().parent().unwrap().join("precious-documents");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("thesis.md"), b"years of work").unwrap();

        let result = store.delete_session(outside.to_str().unwrap()).await;

        assert!(result.is_err());
        assert!(outside.join("thesis.md").is_file());
    }

    #[tokio::test]
    async fn restore_session_rejects_unsafe_ids() {
        let (store, _vault) = test_store().await;
        for id in ["", "..", "/tmp"] {
            assert!(store.restore_session(id).await.is_err(), "{id:?}");
        }
    }

    /// REGRESSION (reviewer-found): a debounced transcript flush still in flight when a
    /// session is deleted used to call `persist_transcript` -> `write_file` ->
    /// `create_dir_all`, recreating `sessions/<id>/`. That resurrected a ghost session AND
    /// permanently broke undo, because `restore_session`'s rename onto the (now existing)
    /// destination fails with ENOTEMPTY.
    #[tokio::test]
    async fn delete_session_drops_the_live_buffer_so_a_pending_flush_cannot_resurrect_the_folder() {
        let (store, vault) = test_store().await;
        store
            .write_meta(&meta("s1", "Being deleted"))
            .await
            .unwrap();
        store
            .write_note("s1", "notes worth restoring")
            .await
            .unwrap();

        // A dirty buffer with a debounce timer already armed and never flushed.
        store
            .append_transcript(
                "s1",
                super::super::TranscriptDelta {
                    transcript_id: "t1".to_string(),
                    new_words: vec![hypr_fs_format::TranscriptWord {
                        id: Some("w0".to_string()),
                        text: "mid-recording".to_string(),
                        start_ms: 0.0,
                        end_ms: 0.0,
                        channel: 0.0,
                        speaker: None,
                        metadata: None,
                    }],
                    replaced_ids: vec![],
                    new_hints: vec![],
                    started_at_ms: 0.0,
                },
            )
            .await
            .unwrap();

        let dir = session_path(&store, &vault, "s1").await;
        store.delete_session("s1").await.unwrap();
        assert!(!dir.exists());

        // Let the armed debounce timer fire well past its deadline.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        assert!(
            !dir.exists(),
            "a pending flush must not recreate the deleted session folder"
        );
        assert!(
            !vault.path().join("sessions/s1").exists(),
            "a pending flush must not resurrect a legacy uuid-named folder either"
        );

        let restored = store.restore_session("s1").await.unwrap();
        assert!(restored, "undo-delete must still work");
        assert_eq!(
            std::fs::read_to_string(dir.join("notes.md")).unwrap(),
            "notes worth restoring"
        );
    }

    /// Two patches of different fields racing each other must both survive: the write lock
    /// spans the read *and* the write, so neither can compute its new whole-file value from
    /// bytes the other is about to replace.
    #[tokio::test]
    async fn concurrent_update_meta_calls_do_not_lose_each_others_fields() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "Original")).await.unwrap();
        let store = std::sync::Arc::new(store);

        let a = {
            let store = store.clone();
            async move {
                store
                    .update_meta(
                        "s1",
                        SessionMetaPatch {
                            title: Some("Renamed".to_string()),
                            ..Default::default()
                        },
                    )
                    .await
            }
        };
        let b = {
            let store = store.clone();
            async move {
                store
                    .update_meta(
                        "s1",
                        SessionMetaPatch {
                            tags: Some(vec!["tagged".to_string()]),
                            ..Default::default()
                        },
                    )
                    .await
            }
        };
        let (ra, rb) = tokio::join!(a, b);
        ra.unwrap();
        rb.unwrap();

        let after = store.read_meta("s1").await.unwrap().unwrap();
        assert_eq!(after.title, "Renamed");
        assert_eq!(after.tags, vec!["tagged".to_string()]);
    }

    /// REGRESSION: sessions are created with `started_at`/`ended_at` null and nothing on
    /// either side of the IPC boundary ever patched them, so every real recording kept
    /// null timestamps forever. A start/stop cycle must leave both stamped in the
    /// persisted file (not just the in-memory index).
    #[tokio::test]
    async fn recording_start_stop_cycle_persists_non_null_timestamps() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Standup")).await.unwrap();

        store
            .mark_recording_started("s1", "2026-07-31T10:00:00.000Z")
            .await
            .unwrap();
        store
            .mark_recording_ended("s1", "2026-07-31T10:30:00.000Z")
            .await
            .unwrap();

        let raw: serde_json::Value = serde_json::from_slice(
            &std::fs::read(session_path(&store, &vault, "s1").await.join("_meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(raw["started_at"], "2026-07-31T10:00:00.000Z");
        assert_eq!(raw["ended_at"], "2026-07-31T10:30:00.000Z");

        let indexed = store.session_get("s1").unwrap().meta;
        assert_eq!(
            indexed.started_at.as_deref(),
            Some("2026-07-31T10:00:00.000Z"),
            "write-through must reach the index"
        );
        assert_eq!(
            indexed.ended_at.as_deref(),
            Some("2026-07-31T10:30:00.000Z")
        );
    }

    #[tokio::test]
    async fn a_second_recording_keeps_the_first_start_and_advances_the_end() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "Standup")).await.unwrap();

        store
            .mark_recording_started("s1", "2026-07-31T10:00:00.000Z")
            .await
            .unwrap();
        store
            .mark_recording_ended("s1", "2026-07-31T10:30:00.000Z")
            .await
            .unwrap();
        store
            .mark_recording_started("s1", "2026-07-31T11:00:00.000Z")
            .await
            .unwrap();
        store
            .mark_recording_ended("s1", "2026-07-31T11:10:00.000Z")
            .await
            .unwrap();

        let after = store.read_meta("s1").await.unwrap().unwrap();
        assert_eq!(
            after.started_at.as_deref(),
            Some("2026-07-31T10:00:00.000Z")
        );
        assert_eq!(after.ended_at.as_deref(), Some("2026-07-31T11:10:00.000Z"));
    }

    /// Same rationale as `update_meta_errors_when_meta_file_is_missing`: a stamp racing a
    /// delete must fail loudly, not invent a meta and resurrect the session folder.
    #[tokio::test]
    async fn mark_recording_timestamps_error_when_meta_file_is_missing() {
        let (store, vault) = test_store().await;
        assert!(
            store
                .mark_recording_started("ghost", "2026-07-31T10:00:00.000Z")
                .await
                .is_err()
        );
        assert!(
            store
                .mark_recording_ended("ghost", "2026-07-31T10:30:00.000Z")
                .await
                .is_err()
        );
        assert!(!vault.path().join("sessions/ghost").exists());
    }

    #[tokio::test]
    async fn read_meta_returns_none_for_absent_file() {
        let (store, _vault) = test_store().await;
        assert_eq!(store.read_meta("nonexistent").await.unwrap(), None);
    }

    #[tokio::test]
    async fn read_note_returns_none_for_absent_file() {
        let (store, _vault) = test_store().await;
        assert_eq!(store.read_note("nonexistent").await.unwrap(), None);
    }

    /// `write_note` never writes frontmatter, but a file can gain a wrapper from outside it
    /// (an external edit, or -- before Task 13 removed it -- the legacy `vault_export`
    /// mirror, which always wrapped a `session_documents` row on export). `read_note` must strip
    /// a well-formed leading block rather than index it verbatim: otherwise `rebuild_index`,
    /// which Task 10 now runs on every startup and window focus, feeds the wrapper back into
    /// `session_documents.body`, and the next export wraps *that* in another layer -- one more
    /// nested frontmatter block per boot/focus, forever.
    #[tokio::test]
    async fn read_note_strips_a_leading_frontmatter_block() {
        let (store, vault) = test_store().await;
        let dir = vault.path().join("sessions/s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("notes.md"),
            "---\nid: s1:note\nposition: 0\nsession_id: s1\n---\n\nreal content",
        )
        .unwrap();

        assert_eq!(
            store.read_note("s1").await.unwrap().unwrap(),
            "real content"
        );
    }

    /// The mirror image of the test above: content that merely *starts* with `---` but isn't
    /// a well-formed, closed frontmatter block (here, a legitimate note whose first line is a
    /// markdown horizontal rule) must never be mangled -- `read_note` only strips a block it
    /// can unambiguously parse.
    #[tokio::test]
    async fn read_note_leaves_unparseable_leading_dashes_untouched() {
        let (store, vault) = test_store().await;
        let dir = vault.path().join("sessions/s1");
        std::fs::create_dir_all(&dir).unwrap();
        let content = "---\n\nActual note that opens with a horizontal rule.";
        std::fs::write(dir.join("notes.md"), content).unwrap();

        assert_eq!(store.read_note("s1").await.unwrap().unwrap(), content);
    }

    #[tokio::test]
    async fn delete_session_on_nonexistent_session_succeeds() {
        let (store, _vault) = test_store().await;
        assert_eq!(store.delete_session("nonexistent").await.unwrap(), None);
    }

    #[tokio::test]
    async fn restore_session_moves_folder_back_and_reindexes() {
        let (store, vault) = test_store().await;
        store
            .write_meta(&meta("s1", "Jury feedback"))
            .await
            .unwrap();
        store.write_note("s1", "Some notes").await.unwrap();

        let dir = session_path(&store, &vault, "s1").await;
        store.delete_session("s1").await.unwrap();
        assert!(!dir.is_dir());

        let restored = store.restore_session("s1").await.unwrap();
        assert!(restored);

        assert!(dir.join("_meta.json").is_file());
        assert!(dir.join("notes.md").is_file());

        let record = store.session_get("s1").unwrap();
        assert_eq!(record.meta.title, "Jury feedback");
        assert_eq!(record.note_markdown.as_deref(), Some("Some notes"));
    }

    #[tokio::test]
    async fn restore_session_returns_false_when_nothing_was_trashed_today() {
        let (store, _vault) = test_store().await;
        let restored = store.restore_session("never-deleted").await.unwrap();
        assert!(!restored);
    }

    /// REGRESSION (reviewer-found): `move_to_trash`'s `unique_path` disambiguates a same-day
    /// repeat trash of the same id as `<id>`, `<id>-1`, ... -- restore must pick the *latest*
    /// (highest suffix) one, not just whichever bare `<id>` entry happens to exist.
    #[tokio::test]
    async fn restore_session_picks_the_most_recently_trashed_same_day_duplicate() {
        let (store, vault) = test_store().await;

        store
            .write_meta(&meta("s1", "Jury feedback"))
            .await
            .unwrap();
        store.write_note("s1", "first content").await.unwrap();
        let rel = store.session_dir("s1").await.unwrap();
        store.delete_session("s1").await.unwrap();

        // A separate process explicitly recreated the same ID; its deletion must
        // retain the actual collision-suffixed trash path returned for that copy.
        let store = SessionStore::new(vault.path().to_path_buf());
        store
            .write_meta(&meta("s1", "Jury feedback"))
            .await
            .unwrap();
        store.write_note("s1", "second content").await.unwrap();
        assert_eq!(store.session_dir("s1").await.unwrap(), rel);
        store.delete_session("s1").await.unwrap();

        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let trash_sessions_dir = vault
            .path()
            .join(".trash")
            .join(&date)
            .join(rel.parent().unwrap());
        let name = rel.file_name().unwrap().to_str().unwrap();
        assert!(trash_sessions_dir.join(name).is_dir());
        assert!(trash_sessions_dir.join(format!("{name}-1")).is_dir());

        let restored = store.restore_session("s1").await.unwrap();
        assert!(restored);

        let note = std::fs::read_to_string(vault.path().join(&rel).join("notes.md")).unwrap();
        assert_eq!(
            note, "second content",
            "restore must bring back the most recently deleted duplicate, not the oldest"
        );
        // The older duplicate is left alone in trash, not silently consumed or merged.
        assert!(trash_sessions_dir.join(name).is_dir());
    }

    #[tokio::test]
    async fn read_meta_detects_corrupted_file() {
        let (store, vault) = test_store().await;
        // Write a valid meta first
        store.write_meta(&meta("s1", "Original")).await.unwrap();

        // Corrupt the file on disk
        let meta_path = session_path(&store, &vault, "s1").await.join("_meta.json");
        std::fs::write(&meta_path, b"{ invalid json").unwrap();

        // read_meta should return Err(StoreError::Serialize)
        let result = store.read_meta("s1").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StoreError::Serialize(_)));
    }
}

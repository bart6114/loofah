use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{SessionStore, StoreError, WriteGuard, paths, validate_doc_id, validate_session_id};

// The `tasks.json` schema is shared with the read-only vault consumers (loof CLI/MCP);
// the types live in `hypr-vault-read` so both sides parse the same shape.
pub use hypr_vault_read::{TaskItem, TasksFile};

/// What the frontend sends on a write: source coordinates come from the command arguments,
/// timestamps and `assignee` are managed store-side (preserved from the existing entry when
/// the task already exists).
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
pub struct TaskInput {
    pub id: String,
    pub source_order: i32,
    pub status: String,
    pub text: String,
    pub body: serde_json::Value,
    #[serde(default)]
    pub due_at: String,
}

/// Which `tasks.json` a source's tasks live in. Both production source types are
/// session-scoped; `Vault` only catches a source type this build doesn't know, so a future
/// non-session source degrades to the vault-root file instead of erroring or misfiling.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum TaskScope {
    Session(String),
    Vault,
}

impl TaskScope {
    /// This scope's key in the in-memory index's tasks map (and the id carried by the
    /// `index-changed` tasks event).
    fn index_key(&self) -> &str {
        match self {
            TaskScope::Session(id) => id,
            TaskScope::Vault => super::index::VAULT_TASKS_KEY,
        }
    }
}

fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn same_content(existing: &TaskItem, next: &TaskItem) -> bool {
    existing.source_type == next.source_type
        && existing.source_id == next.source_id
        && existing.source_order == next.source_order
        && existing.status == next.status
        && existing.text == next.text
        && existing.body == next.body
        && existing.due_at == next.due_at
}

impl SessionStore {
    pub async fn list_tasks(
        &self,
        source_type: &str,
        source_id: &str,
    ) -> Result<Vec<TaskItem>, StoreError> {
        let scope = self.resolve_task_scope(source_type, source_id).await?;
        let mut tasks: Vec<TaskItem> = self
            .read_tasks_at(&scope)
            .await?
            .into_iter()
            .filter(|t| t.source_type == source_type && t.source_id == source_id)
            .collect();
        tasks.sort_by(|a, b| a.source_order.cmp(&b.source_order).then(a.id.cmp(&b.id)));
        Ok(tasks)
    }

    /// Replace the full task list of one source: entries for the source not in `inputs` are
    /// dropped, entries elsewhere in the same file whose id collides are re-homed to this
    /// source (the old SQL upsert's `ON CONFLICT(id)` behavior). Timestamps and `assignee`
    /// survive for tasks that already exist; a content-identical replace is a no-op that
    /// never touches the file (so the write journal only records real changes).
    pub async fn replace_tasks(
        &self,
        source_type: &str,
        source_id: &str,
        inputs: Vec<TaskInput>,
    ) -> Result<(), StoreError> {
        let scope = self.resolve_task_scope(source_type, source_id).await?;
        self.ensure_task_scope_writable(&scope).await?;

        // One guard across read-modify-write: a `tasks.json` holds every source's tasks, so
        // two concurrent replaces that each read the same starting file and write a whole new
        // one back would silently drop the loser's changes.
        let guard = self.lock_writes().await;

        let existing = self.read_tasks_at(&scope).await?;
        let prior_by_id: HashMap<&str, &TaskItem> =
            existing.iter().map(|t| (t.id.as_str(), t)).collect();
        let input_ids: std::collections::HashSet<&str> =
            inputs.iter().map(|t| t.id.as_str()).collect();

        let now = now_iso();
        let mut next: Vec<TaskItem> = existing
            .iter()
            .filter(|t| {
                !(t.source_type == source_type && t.source_id == source_id)
                    && !input_ids.contains(t.id.as_str())
            })
            .cloned()
            .collect();
        for input in inputs {
            let prior = prior_by_id.get(input.id.as_str());
            let mut item = TaskItem {
                id: input.id,
                source_type: source_type.to_string(),
                source_id: source_id.to_string(),
                source_order: input.source_order,
                status: input.status,
                text: input.text,
                body: input.body,
                due_at: input.due_at,
                assignee: prior.map(|p| p.assignee.clone()).unwrap_or_default(),
                created_at: prior
                    .map(|p| p.created_at.clone())
                    .unwrap_or_else(|| now.clone()),
                updated_at: now.clone(),
            };
            if let Some(prior) = prior {
                if same_content(prior, &item) {
                    item.updated_at = prior.updated_at.clone();
                }
            }
            next.push(item);
        }

        if next == existing {
            return Ok(());
        }
        self.write_tasks_at_locked(&guard, &scope, &next).await
    }

    /// Remove the listed task ids from one source. Removal is scoped: an id that lives
    /// under a different source in the same file is left alone (parity with the old SQL
    /// `WHERE source_type = ? AND source_id = ?` guard). Idempotent.
    pub async fn remove_tasks(
        &self,
        source_type: &str,
        source_id: &str,
        task_ids: Vec<String>,
    ) -> Result<(), StoreError> {
        if task_ids.is_empty() {
            return Ok(());
        }
        let scope = self.resolve_task_scope(source_type, source_id).await?;
        let guard = self.lock_writes().await;
        let existing = self.read_tasks_at(&scope).await?;
        let ids: std::collections::HashSet<&str> = task_ids.iter().map(|s| s.as_str()).collect();
        let next: Vec<TaskItem> = existing
            .iter()
            .filter(|t| {
                !(t.source_type == source_type
                    && t.source_id == source_id
                    && ids.contains(t.id.as_str()))
            })
            .cloned()
            .collect();
        if next == existing {
            return Ok(());
        }
        self.write_tasks_at_locked(&guard, &scope, &next).await
    }

    /// Re-home tasks (found by id anywhere -- any session's `tasks.json` or the vault-root
    /// file) to `next_source`, assigning `insertion_order + index` in the given id order.
    /// Ids that exist nowhere are skipped, like the old SQL `UPDATE ... WHERE id = ?`
    /// matching zero rows.
    pub async fn move_tasks(
        &self,
        task_ids: Vec<String>,
        next_source_type: &str,
        next_source_id: &str,
        insertion_order: i32,
    ) -> Result<(), StoreError> {
        if task_ids.is_empty() {
            return Ok(());
        }
        let dest_scope = self
            .resolve_task_scope(next_source_type, next_source_id)
            .await?;
        self.ensure_task_scope_writable(&dest_scope).await?;

        let mut scopes = vec![dest_scope.clone()];
        for scope in self.scan_task_scopes().await? {
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
        }

        // Same read-modify-write guard as `replace_tasks`, spanning every file this move
        // touches (a move rewrites both the source and the destination `tasks.json`).
        let guard = self.lock_writes().await;

        let mut files: Vec<(TaskScope, Vec<TaskItem>, bool)> = Vec::new();
        for scope in scopes {
            let tasks = self.read_tasks_at(&scope).await?;
            files.push((scope, tasks, false));
        }

        let now = now_iso();
        for (index, task_id) in task_ids.iter().enumerate() {
            let Some((file_index, task_index)) =
                files
                    .iter()
                    .enumerate()
                    .find_map(|(file_index, (_, tasks, _))| {
                        tasks
                            .iter()
                            .position(|t| &t.id == task_id)
                            .map(|task_index| (file_index, task_index))
                    })
            else {
                continue;
            };

            let mut task = files[file_index].1.remove(task_index);
            files[file_index].2 = true;
            task.source_type = next_source_type.to_string();
            task.source_id = next_source_id.to_string();
            task.source_order = insertion_order + index as i32;
            task.updated_at = now.clone();

            let dest = files
                .iter_mut()
                .find(|(scope, _, _)| scope == &dest_scope)
                .expect("dest scope is always loaded");
            dest.1.push(task);
            dest.2 = true;
        }

        for (scope, tasks, dirty) in files {
            if dirty {
                self.write_tasks_at_locked(&guard, &scope, &tasks).await?;
            }
        }
        Ok(())
    }

    /// Map a task source to the file its tasks live in. `session_raw_note` carries the
    /// session id directly; `enhanced_note` carries a doc id, resolved to its session via
    /// the in-memory docs index first (write-through on every doc write) and a
    /// filesystem scan for `sessions/*/enhanced/<id>.md` as the file-canonical fallback.
    /// An enhanced doc that exists nowhere is an error -- silently picking a file would
    /// strand its tasks once the doc becomes resolvable. Unknown source types get the
    /// vault-root file.
    async fn resolve_task_scope(
        &self,
        source_type: &str,
        source_id: &str,
    ) -> Result<TaskScope, StoreError> {
        match source_type {
            "session_raw_note" => {
                validate_session_id(source_id)?;
                Ok(TaskScope::Session(source_id.to_string()))
            }
            "enhanced_note" => {
                validate_doc_id(source_id)?;
                let indexed_session = {
                    let index = self.index.read().unwrap();
                    index.docs.iter().find_map(|(session_id, docs)| {
                        docs.iter()
                            .any(|doc| doc.id == source_id)
                            .then(|| session_id.clone())
                    })
                };
                if let Some(session_id) = indexed_session {
                    if !session_id.is_empty() {
                        return Ok(TaskScope::Session(session_id));
                    }
                }

                let vault_base = self.vault_base.clone();
                let doc_id = source_id.to_string();
                let found = tokio::task::spawn_blocking(move || -> Option<String> {
                    let discovery = hypr_vault_read::discover_sessions(&vault_base).ok()?;
                    discovery.sessions.into_iter().find_map(|(location, _)| {
                        vault_base
                            .join(paths::enhanced_doc_path_in(&location.relative_dir, &doc_id))
                            .is_file()
                            .then_some(location.id)
                    })
                })
                .await
                .map_err(|e| StoreError::Io(format!("task join error: {e}")))?;

                found.map(TaskScope::Session).ok_or_else(|| {
                    StoreError::Io(format!(
                        "enhanced note {source_id} belongs to no known session; refusing to guess a tasks.json"
                    ))
                })
            }
            _ => Ok(TaskScope::Vault),
        }
    }

    /// Same rule as `write_enhanced_doc`: a session-scoped write must never resurrect a
    /// session folder that a racing delete just trashed, so the session's `_meta.json` has
    /// to exist before we create or grow its `tasks.json`.
    async fn ensure_task_scope_writable(&self, scope: &TaskScope) -> Result<(), StoreError> {
        if let TaskScope::Session(session_id) = scope {
            if self.read_meta(session_id).await?.is_none() {
                return Err(StoreError::Io(format!(
                    "session {session_id} has no _meta.json; refusing to write tasks"
                )));
            }
        }
        Ok(())
    }

    /// Vault-relative path of this scope's `tasks.json`; session scopes resolve their
    /// canonical directory directly from its validated ID.
    async fn task_scope_path(&self, scope: &TaskScope) -> Result<PathBuf, StoreError> {
        match scope {
            TaskScope::Session(id) => {
                Ok(paths::session_tasks_path_in(&self.session_dir(id).await?))
            }
            TaskScope::Vault => Ok(paths::vault_tasks_path()),
        }
    }

    async fn read_tasks_at(&self, scope: &TaskScope) -> Result<Vec<TaskItem>, StoreError> {
        let path = self.vault_base.join(self.task_scope_path(scope).await?);
        tokio::task::spawn_blocking(move || -> Result<Vec<TaskItem>, StoreError> {
            let raw = match std::fs::read(&path) {
                Ok(raw) => raw,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(StoreError::Io(format!("failed to read tasks file: {e}"))),
            };
            let file: TasksFile = serde_json::from_slice(&raw)
                .map_err(|e| StoreError::Serialize(format!("failed to parse tasks file: {e}")))?;
            Ok(file.tasks)
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {e}")))?
    }

    async fn write_tasks_at_locked(
        &self,
        guard: &WriteGuard<'_>,
        scope: &TaskScope,
        tasks: &[TaskItem],
    ) -> Result<(), StoreError> {
        let file = TasksFile {
            tasks: tasks.to_vec(),
        };
        let bytes =
            serde_json::to_vec_pretty(&file).map_err(|e| StoreError::Serialize(e.to_string()))?;
        let relative = self.task_scope_path(scope).await?;
        self.write_file_locked(guard, relative, bytes).await?;

        self.index_set_tasks(scope.index_key(), tasks.to_vec());
        self.notify_index_changed(
            super::IndexEntity::Tasks,
            vec![scope.index_key().to_string()],
        );
        Ok(())
    }

    /// Every `tasks.json` that exists: one per discovered session that has tasks, plus
    /// the vault-root file. Cheap because sessions without tasks have no file at all.
    async fn scan_task_scopes(&self) -> Result<Vec<TaskScope>, StoreError> {
        let vault_base = self.vault_base.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<TaskScope>, StoreError> {
            let mut scopes = Vec::new();
            if vault_base.join(paths::vault_tasks_path()).is_file() {
                scopes.push(TaskScope::Vault);
            }
            let discovery = hypr_vault_read::discover_sessions(&vault_base)?;
            for (location, _) in discovery.sessions {
                if vault_base
                    .join(paths::session_tasks_path_in(&location.relative_dir))
                    .is_file()
                {
                    scopes.push(TaskScope::Session(location.id));
                }
            }
            Ok(scopes)
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::super::content::SessionMeta;
    use super::super::enhanced::EnhancedDoc;
    use super::*;

    fn meta(id: &str) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            title: "Session".to_string(),
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

    fn input(id: &str, order: i32, text: &str) -> TaskInput {
        TaskInput {
            id: id.to_string(),
            source_order: order,
            status: "todo".to_string(),
            text: text.to_string(),
            body: serde_json::json!([
                { "type": "paragraph", "content": [{ "type": "text", "text": text }] }
            ]),
            due_at: String::new(),
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
    async fn replace_then_list_round_trips_ordered_tasks() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();

        store
            .replace_tasks(
                "session_raw_note",
                "s1",
                vec![input("t-b", 1, "Second"), input("t-a", 0, "First")],
            )
            .await
            .unwrap();

        assert!(
            session_path(&store, &vault, "s1")
                .await
                .join("tasks.json")
                .is_file()
        );
        let listed = store.list_tasks("session_raw_note", "s1").await.unwrap();
        assert_eq!(
            listed.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            vec!["t-a", "t-b"],
            "list must sort by source_order"
        );
        assert_eq!(listed[0].text, "First");
        assert_eq!(listed[0].status, "todo");
        assert!(listed[0].body.is_array());
        assert!(!listed[0].created_at.is_empty());
    }

    #[tokio::test]
    async fn replace_drops_missing_ids_and_preserves_created_at() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store
            .replace_tasks(
                "session_raw_note",
                "s1",
                vec![input("t-a", 0, "Keep"), input("t-b", 1, "Drop")],
            )
            .await
            .unwrap();
        let before = store.list_tasks("session_raw_note", "s1").await.unwrap();

        store
            .replace_tasks(
                "session_raw_note",
                "s1",
                vec![input("t-a", 0, "Keep edited")],
            )
            .await
            .unwrap();

        let after = store.list_tasks("session_raw_note", "s1").await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, "t-a");
        assert_eq!(after[0].text, "Keep edited");
        assert_eq!(
            after[0].created_at, before[0].created_at,
            "created_at must survive an update"
        );
    }

    /// Two sources in the same session file must not clobber each other: replacing the raw
    /// note's tasks leaves the enhanced note's tasks alone.
    #[tokio::test]
    async fn sources_are_isolated_within_one_session_file() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store
            .write_enhanced_doc(&EnhancedDoc {
                id: "doc-1".to_string(),
                session_id: "s1".to_string(),
                kind: "summary".to_string(),
                title: String::new(),
                template_id: String::new(),
                sort_order: 0,
                markdown: "body".to_string(),
            })
            .await
            .unwrap();

        store
            .replace_tasks("session_raw_note", "s1", vec![input("t-raw", 0, "Raw")])
            .await
            .unwrap();
        store
            .replace_tasks(
                "enhanced_note",
                "doc-1",
                vec![input("t-enh", 0, "Enhanced")],
            )
            .await
            .unwrap();
        store
            .replace_tasks("session_raw_note", "s1", vec![])
            .await
            .unwrap();

        assert!(
            store
                .list_tasks("session_raw_note", "s1")
                .await
                .unwrap()
                .is_empty()
        );
        let enhanced = store.list_tasks("enhanced_note", "doc-1").await.unwrap();
        assert_eq!(enhanced.len(), 1, "other source's tasks must survive");
        assert_eq!(enhanced[0].id, "t-enh");
    }

    /// Enhanced-note tasks land in the owning session's file, resolved doc id -> session.
    #[tokio::test]
    async fn enhanced_note_tasks_live_in_the_owning_sessions_file() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store
            .write_enhanced_doc(&EnhancedDoc {
                id: "doc-1".to_string(),
                session_id: "s1".to_string(),
                kind: "summary".to_string(),
                title: String::new(),
                template_id: String::new(),
                sort_order: 0,
                markdown: "body".to_string(),
            })
            .await
            .unwrap();

        store
            .replace_tasks("enhanced_note", "doc-1", vec![input("t-1", 0, "Task")])
            .await
            .unwrap();

        assert!(
            session_path(&store, &vault, "s1")
                .await
                .join("tasks.json")
                .is_file()
        );
        assert!(!vault.path().join("tasks.json").exists());
    }

    /// The index row is the fast path; with it gone, the filesystem scan still finds the
    /// owning session (file-canonical fallback).
    #[tokio::test]
    async fn enhanced_note_resolution_falls_back_to_a_filesystem_scan() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        let dir = session_path(&store, &vault, "s1").await;
        std::fs::create_dir_all(dir.join("enhanced")).unwrap();
        std::fs::write(dir.join("enhanced/doc-x.md"), "dropped in by hand").unwrap();

        store
            .replace_tasks("enhanced_note", "doc-x", vec![input("t-1", 0, "Task")])
            .await
            .unwrap();

        assert_eq!(
            store
                .list_tasks("enhanced_note", "doc-x")
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(dir.join("tasks.json").is_file());
    }

    #[tokio::test]
    async fn enhanced_note_with_no_owning_session_is_an_error() {
        let (store, _vault) = test_store().await;
        let result = store
            .replace_tasks("enhanced_note", "ghost-doc", vec![input("t-1", 0, "Task")])
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn replace_refuses_a_session_without_meta() {
        let (store, vault) = test_store().await;
        let result = store
            .replace_tasks("session_raw_note", "ghost", vec![input("t-1", 0, "Task")])
            .await;
        assert!(result.is_err());
        assert!(!vault.path().join("sessions/ghost").exists());
    }

    #[tokio::test]
    async fn unknown_source_types_go_to_the_vault_root_file() {
        let (store, vault) = test_store().await;
        store
            .replace_tasks("daily_note", "2026-07-26", vec![input("t-1", 0, "Task")])
            .await
            .unwrap();

        assert!(vault.path().join("tasks.json").is_file());
        assert_eq!(
            store
                .list_tasks("daily_note", "2026-07-26")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn remove_is_scoped_to_the_source() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store
            .replace_tasks("session_raw_note", "s1", vec![input("t-1", 0, "Task")])
            .await
            .unwrap();

        // same id under a different source must survive a scoped removal
        store
            .remove_tasks("some_other_source", "other", vec!["t-1".to_string()])
            .await
            .unwrap();
        assert_eq!(
            store
                .list_tasks("session_raw_note", "s1")
                .await
                .unwrap()
                .len(),
            1
        );

        store
            .remove_tasks("session_raw_note", "s1", vec!["t-1".to_string()])
            .await
            .unwrap();
        assert!(
            store
                .list_tasks("session_raw_note", "s1")
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// Move re-homes tasks across sessions by id alone: from s1's file into s2's file, with
    /// contiguous orders starting at the insertion point. Unknown ids are skipped.
    #[tokio::test]
    async fn move_re_homes_tasks_across_session_files() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store.write_meta(&meta("s2")).await.unwrap();
        store
            .replace_tasks(
                "session_raw_note",
                "s1",
                vec![input("t-1", 0, "One"), input("t-2", 1, "Two")],
            )
            .await
            .unwrap();

        store
            .move_tasks(
                vec!["t-1".to_string(), "t-2".to_string(), "t-ghost".to_string()],
                "session_raw_note",
                "s2",
                4,
            )
            .await
            .unwrap();

        assert!(
            store
                .list_tasks("session_raw_note", "s1")
                .await
                .unwrap()
                .is_empty()
        );
        let moved = store.list_tasks("session_raw_note", "s2").await.unwrap();
        assert_eq!(moved.len(), 2);
        assert_eq!(moved[0].id, "t-1");
        assert_eq!(moved[0].source_order, 4);
        assert_eq!(moved[1].id, "t-2");
        assert_eq!(moved[1].source_order, 5);
        assert!(
            session_path(&store, &vault, "s2")
                .await
                .join("tasks.json")
                .is_file()
        );
    }

    /// A content-identical replace must not rewrite the file: the bytes (including every
    /// updated_at) stay identical, so the write journal never records a phantom change.
    #[tokio::test]
    async fn unchanged_replace_leaves_the_file_bytes_alone() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store
            .replace_tasks("session_raw_note", "s1", vec![input("t-1", 0, "Task")])
            .await
            .unwrap();
        let path = session_path(&store, &vault, "s1").await.join("tasks.json");
        let before = std::fs::read(&path).unwrap();

        store
            .replace_tasks("session_raw_note", "s1", vec![input("t-1", 0, "Task")])
            .await
            .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// The rewrite is atomic: after a write the session dir holds the real file only (no
    /// tmp sibling left behind) and the file parses whole.
    #[tokio::test]
    async fn rewrite_is_atomic_with_no_tmp_leftovers() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store
            .replace_tasks("session_raw_note", "s1", vec![input("t-1", 0, "Task")])
            .await
            .unwrap();

        let dir = session_path(&store, &vault, "s1").await;
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(names.iter().all(|n| !n.contains("tmp")), "{names:?}");
        let raw = std::fs::read(dir.join("tasks.json")).unwrap();
        let parsed: TasksFile = serde_json::from_slice(&raw).unwrap();
        assert_eq!(parsed.tasks.len(), 1);
    }

    /// Both sources share one `sessions/<id>/tasks.json`, so a replace is a read-modify-write
    /// of the whole file. With the write lock spanning only the write (not the read), two
    /// concurrent replaces each start from the same bytes and the loser's tasks vanish.
    #[tokio::test]
    async fn concurrent_replaces_of_different_sources_do_not_lose_each_others_tasks() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store
            .write_enhanced_doc(&EnhancedDoc {
                id: "doc-1".to_string(),
                session_id: "s1".to_string(),
                kind: "summary".to_string(),
                title: String::new(),
                template_id: String::new(),
                sort_order: 0,
                markdown: "body".to_string(),
            })
            .await
            .unwrap();
        let store = std::sync::Arc::new(store);

        let raw = {
            let store = store.clone();
            async move {
                store
                    .replace_tasks("session_raw_note", "s1", vec![input("t-raw", 0, "Raw")])
                    .await
            }
        };
        let enhanced = {
            let store = store.clone();
            async move {
                store
                    .replace_tasks(
                        "enhanced_note",
                        "doc-1",
                        vec![input("t-enh", 0, "Enhanced")],
                    )
                    .await
            }
        };
        let (a, b) = tokio::join!(raw, enhanced);
        a.unwrap();
        b.unwrap();

        assert_eq!(
            store
                .list_tasks("session_raw_note", "s1")
                .await
                .unwrap()
                .len(),
            1,
            "the raw note's task must survive the concurrent write"
        );
        assert_eq!(
            store
                .list_tasks("enhanced_note", "doc-1")
                .await
                .unwrap()
                .len(),
            1,
            "the enhanced note's task must survive the concurrent write"
        );
    }

    #[tokio::test]
    async fn task_sources_with_unsafe_ids_are_rejected() {
        let (store, vault) = test_store().await;
        for id in ["", "..", "/etc"] {
            assert!(
                store
                    .replace_tasks("session_raw_note", id, vec![input("t-1", 0, "Task")])
                    .await
                    .is_err(),
                "{id:?}"
            );
            assert!(
                store.list_tasks("enhanced_note", id).await.is_err(),
                "{id:?}"
            );
        }
        assert!(!vault.path().join("sessions").exists());
    }

    #[tokio::test]
    async fn tasks_writes_are_journaled_as_own_writes() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store
            .replace_tasks("session_raw_note", "s1", vec![input("t-1", 0, "Task")])
            .await
            .unwrap();

        let rel = store.session_dir("s1").await.unwrap().join("tasks.json");
        let rel = rel.to_str().unwrap();
        assert!(store.journal_matches_current_file(rel));
        std::fs::write(vault.path().join(rel), b"{\"tasks\":[]}").unwrap();
        assert!(!store.journal_matches_current_file(rel));
    }
}

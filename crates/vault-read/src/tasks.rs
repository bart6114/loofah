use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, layout, paths};

/// One action item, file-canonical in `sessions/<session_id>/tasks.json` (or the vault-root
/// `tasks.json` for a source that cannot be tied to a session). Mirrors the live columns of
/// the legacy `action_items` table minus the ownership columns (dropped per plan decision
/// D10) and minus `deleted_at` -- deletion removes the entry and the list is rewritten
/// atomically.
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
pub struct TaskItem {
    pub id: String,
    /// "session_raw_note" / "session_summary" (source_id is the session id), or "enhanced_note" (source_id is
    /// the enhanced doc id). Stored verbatim for any other value.
    pub source_type: String,
    pub source_id: String,
    pub source_order: i32,
    /// "todo" | "in_progress" | "done". Stored verbatim; the frontend validates on read.
    pub status: String,
    /// Plain-text preview of the first paragraph, kept alongside the body like the old
    /// `text` column so the file is greppable.
    pub text: String,
    /// The TipTap `JSONContent[]` body, stored as real JSON (the old `body_json` column
    /// held it stringified).
    pub body: serde_json::Value,
    #[serde(default)]
    pub due_at: String,
    #[serde(default)]
    pub assignee: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct TasksFile {
    #[serde(default)]
    pub tasks: Vec<TaskItem>,
}

/// Read one session's `tasks.json`; a missing file is an empty list. The session id
/// resolves to its physical directory via layout discovery.
pub fn read_session_tasks(vault: &Path, session_id: &str) -> Result<Vec<TaskItem>> {
    read_session_tasks_in(vault, &layout::artifact_dir(vault, session_id)?)
}

/// `read_session_tasks` for an already-resolved session directory (vault-relative).
pub fn read_session_tasks_in(vault: &Path, session_dir: &Path) -> Result<Vec<TaskItem>> {
    let path = vault.join(paths::session_tasks_path_in(session_dir));
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::Io(format!("failed to read tasks file: {e}"))),
    };
    serde_json::from_slice::<TasksFile>(&bytes)
        .map(|file| file.tasks)
        .map_err(|e| Error::Parse(format!("failed to deserialize tasks.json: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_session_tasks_reads_file_and_tolerates_absence() {
        let temp = tempfile::tempdir().unwrap();
        assert!(read_session_tasks(temp.path(), "s1").unwrap().is_empty());

        let dir = temp.path().join("sessions/s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("tasks.json"),
            serde_json::json!({
                "tasks": [{
                    "id": "task-1",
                    "source_type": "session_raw_note",
                    "source_id": "s1",
                    "source_order": 1,
                    "status": "todo",
                    "text": "Prepare launch",
                    "body": [],
                    "created_at": "2026-07-01T00:00:00Z",
                    "updated_at": "2026-07-01T00:00:00Z",
                }],
            })
            .to_string(),
        )
        .unwrap();

        let tasks = read_session_tasks(temp.path(), "s1").unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].text, "Prepare launch");
        assert_eq!(tasks[0].assignee, "");

        std::fs::write(dir.join("tasks.json"), "{ invalid").unwrap();
        assert!(read_session_tasks(temp.path(), "s1").is_err());
    }
}

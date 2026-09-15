use std::path::{Path, PathBuf};

pub fn sessions_root() -> PathBuf {
    PathBuf::from("sessions")
}

// Artifact names are fixed; only the session directory itself varies. These helpers
// build artifact paths from a resolved session directory (vault-relative or absolute),
// never from the logical id — directory basenames are not guaranteed to equal ids.

pub fn meta_path_in(session_dir: &Path) -> PathBuf {
    session_dir.join("_meta.json")
}

pub fn note_path_in(session_dir: &Path) -> PathBuf {
    session_dir.join("notes.md")
}

/// Pre-rename note file name (`_memo.md`). Readers fall back to it when `notes.md`
/// is absent; the store migrates it away on the next note write.
pub fn legacy_note_path_in(session_dir: &Path) -> PathBuf {
    session_dir.join("_memo.md")
}

pub fn enhanced_dir_in(session_dir: &Path) -> PathBuf {
    session_dir.join("enhanced")
}

pub fn enhanced_doc_path_in(session_dir: &Path, doc_id: &str) -> PathBuf {
    enhanced_dir_in(session_dir).join(format!("{}.md", doc_id))
}

pub fn transcript_path_in(session_dir: &Path) -> PathBuf {
    session_dir.join("transcript.json")
}

pub fn session_tasks_path_in(session_dir: &Path) -> PathBuf {
    session_dir.join("tasks.json")
}

pub fn audio_dir_in(session_dir: &Path) -> PathBuf {
    session_dir.join("audio")
}

#[deprecated(
    note = "directory basenames are not guaranteed to equal session ids; resolve the physical directory via `layout` and use the *_in helpers"
)]
pub fn session_dir(id: &str) -> PathBuf {
    sessions_root().join(id)
}

#[deprecated(
    note = "directory basenames are not guaranteed to equal session ids; resolve the physical directory via `layout` and use `meta_path_in`"
)]
pub fn meta_path(id: &str) -> PathBuf {
    meta_path_in(&sessions_root().join(id))
}

#[deprecated(
    note = "directory basenames are not guaranteed to equal session ids; resolve the physical directory via `layout` and use `note_path_in`"
)]
pub fn note_path(id: &str) -> PathBuf {
    note_path_in(&sessions_root().join(id))
}

#[deprecated(
    note = "directory basenames are not guaranteed to equal session ids; resolve the physical directory via `layout` and use `enhanced_dir_in`"
)]
pub fn enhanced_dir(id: &str) -> PathBuf {
    enhanced_dir_in(&sessions_root().join(id))
}

#[deprecated(
    note = "directory basenames are not guaranteed to equal session ids; resolve the physical directory via `layout` and use `enhanced_doc_path_in`"
)]
pub fn enhanced_doc_path(id: &str, doc_id: &str) -> PathBuf {
    enhanced_doc_path_in(&sessions_root().join(id), doc_id)
}

#[deprecated(
    note = "directory basenames are not guaranteed to equal session ids; resolve the physical directory via `layout` and use `transcript_path_in`"
)]
pub fn transcript_path(id: &str) -> PathBuf {
    transcript_path_in(&sessions_root().join(id))
}

#[deprecated(
    note = "directory basenames are not guaranteed to equal session ids; resolve the physical directory via `layout` and use `session_tasks_path_in`"
)]
pub fn session_tasks_path(id: &str) -> PathBuf {
    session_tasks_path_in(&sessions_root().join(id))
}

pub fn vault_tasks_path() -> PathBuf {
    PathBuf::from("tasks.json")
}

pub fn people_path() -> PathBuf {
    PathBuf::from("people.json")
}

pub fn tags_path() -> PathBuf {
    PathBuf::from("tags.json")
}

#[deprecated(
    note = "directory basenames are not guaranteed to equal session ids; resolve the physical directory via `layout` and use `audio_dir_in`"
)]
pub fn audio_dir(id: &str) -> PathBuf {
    audio_dir_in(&sessions_root().join(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(deprecated)]
    fn paths_are_relative_and_correct() {
        assert_eq!(sessions_root(), PathBuf::from("sessions"));
        assert_eq!(session_dir("s1"), PathBuf::from("sessions/s1"));
        assert_eq!(meta_path("s1"), PathBuf::from("sessions/s1/_meta.json"));
        assert_eq!(note_path("s1"), PathBuf::from("sessions/s1/notes.md"));
        assert_eq!(enhanced_dir("s1"), PathBuf::from("sessions/s1/enhanced"));
        assert_eq!(
            enhanced_doc_path("s1", "doc-1"),
            PathBuf::from("sessions/s1/enhanced/doc-1.md")
        );
        assert_eq!(
            transcript_path("s1"),
            PathBuf::from("sessions/s1/transcript.json")
        );
        assert_eq!(
            session_tasks_path("s1"),
            PathBuf::from("sessions/s1/tasks.json")
        );
        assert_eq!(vault_tasks_path(), PathBuf::from("tasks.json"));
        assert_eq!(people_path(), PathBuf::from("people.json"));
        assert_eq!(tags_path(), PathBuf::from("tags.json"));
        assert_eq!(audio_dir("s1"), PathBuf::from("sessions/s1/audio"));
    }

    #[test]
    fn in_helpers_join_fixed_artifact_names_onto_the_session_dir() {
        let dir = Path::new("sessions/Work/2026-03-20 — Planning — 550e84");
        assert_eq!(meta_path_in(dir), dir.join("_meta.json"));
        assert_eq!(note_path_in(dir), dir.join("notes.md"));
        assert_eq!(legacy_note_path_in(dir), dir.join("_memo.md"));
        assert_eq!(enhanced_dir_in(dir), dir.join("enhanced"));
        assert_eq!(
            enhanced_doc_path_in(dir, "doc-1"),
            dir.join("enhanced/doc-1.md")
        );
        assert_eq!(transcript_path_in(dir), dir.join("transcript.json"));
        assert_eq!(session_tasks_path_in(dir), dir.join("tasks.json"));
        assert_eq!(audio_dir_in(dir), dir.join("audio"));
    }
}

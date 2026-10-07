use std::path::{Path, PathBuf};

pub fn sessions_root() -> PathBuf {
    PathBuf::from("sessions")
}

/// Validate a portable filename component without touching the filesystem.
pub fn validate_session_id(id: &str) -> crate::Result<()> {
    let stem = id
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "COM¹" | "COM²" | "COM³" | "LPT¹" | "LPT²" | "LPT³"
    ) || (stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if id.is_empty()
        || id.starts_with('.')
        || id.ends_with(['.', ' '])
        || id
            .chars()
            .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c))
        || std::path::Path::new(id).is_absolute()
        || reserved
        || id.len() > 255
    {
        return Err(crate::Error::Parse(format!("invalid session id: {id:?}")));
    }
    Ok(())
}

pub fn validated_session_dir(id: &str) -> crate::Result<PathBuf> {
    session_dir_under(&sessions_root(), id)
}

pub fn session_dir_under(sessions_root: &Path, id: &str) -> crate::Result<PathBuf> {
    validate_session_id(id)?;
    Ok(sessions_root.join(id))
}

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

pub fn summary_path_in(session_dir: &Path) -> PathBuf {
    session_dir.join("summary.md")
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

pub fn vault_tasks_path() -> PathBuf {
    PathBuf::from("tasks.json")
}

pub fn people_path() -> PathBuf {
    PathBuf::from("people.json")
}

pub fn tags_path() -> PathBuf {
    PathBuf::from("tags.json")
}

#[cfg(test)]
mod tests {
    use super::*;

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

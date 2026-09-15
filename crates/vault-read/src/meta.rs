use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, layout, paths, strip_leading_frontmatter};

#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TagSuggestionStatus {
    Pending,
    Complete,
}

#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
pub struct TagSuggestionItem {
    pub name: String,
    pub confidence: f32,
}

#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
pub struct TagSuggestionState {
    #[serde(alias = "transcript_hash")]
    pub source_hash: String,
    pub algorithm_version: u32,
    pub status: TagSuggestionStatus,
    pub items: Vec<TagSuggestionItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dismissed: Vec<String>,
}

#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub created_at: String,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_suggestions: Option<TagSuggestionState>,
    /// Marker for app-created special sessions (today only the onboarding welcome
    /// note) so they can be found again across restarts. Pre-removal builds carried
    /// this inside the retired calendar-event envelope, which now round-trips
    /// through `extra` -- see `session_find_by_tracking_id`'s legacy fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Who wrote this note. Absent = the human vault owner; present = an
    /// agent/other writer (free-form, e.g. "claude-code").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// The skill the author ran to produce this note, if any (free-form,
    /// e.g. "meeting-summarizer"). Only meaningful alongside `author`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    /// Forward-compat catch-all: fields written by newer app versions must survive a
    /// read-modify-write cycle from this version (vaults are file-synced across machines
    /// running different builds).
    #[serde(flatten)]
    #[specta(skip)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Read only `sessions/<id>/_meta.json`. Missing files return `None`; unreadable,
/// malformed, and mismatched metadata return errors.
pub fn read_session_meta(vault: &Path, id: &str) -> Result<Option<SessionMeta>> {
    match layout::find_session(vault, id) {
        Ok(found) => Ok(found.map(|(_, meta)| meta)),
        Err(layout::SessionLookupError::Corrupt { reason, .. }) => Err(Error::Parse(reason)),
        Err(layout::SessionLookupError::Io(reason)) => Err(Error::Io(reason)),
    }
}

/// Read a session's `_meta.json` from an already-resolved session directory
/// (vault-relative). `Ok(None)` only for a genuinely absent file -- a transiently
/// unreadable or corrupt file is an error, never "no session".
pub fn read_session_meta_in(vault: &Path, session_dir: &Path) -> Result<Option<SessionMeta>> {
    let path = vault.join(paths::meta_path_in(session_dir));
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Io(format!("failed to read meta file: {e}"))),
    };
    let meta: SessionMeta = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Parse(format!("failed to deserialize meta: {e}")))?;
    if paths::validated_session_dir(&meta.id)? != session_dir {
        return Err(Error::Parse(format!(
            "metadata id {:?} does not match directory {}",
            meta.id,
            session_dir.display()
        )));
    }
    Ok(Some(meta))
}

/// List matching metadata in non-hidden, non-symlinked direct session directories.
pub fn list_session_metas(vault: &Path) -> Result<Vec<SessionMeta>> {
    Ok(layout::discover_sessions(vault)?
        .sessions
        .into_iter()
        .map(|(_, meta)| meta)
        .collect())
}

/// Read the user's note (`notes.md`, falling back to the pre-rename `_memo.md`),
/// stripping any legacy exporter frontmatter wrapper.
pub fn read_note(vault: &Path, id: &str) -> Result<Option<String>> {
    read_note_in(vault, &layout::artifact_dir(vault, id)?)
}

/// `read_note` for an already-resolved session directory (vault-relative). `notes.md`
/// wins when both files exist -- the store only ever writes `notes.md` and trashes the
/// legacy file on the next note write, so a lingering `_memo.md` is always the stale copy.
pub fn read_note_in(vault: &Path, session_dir: &Path) -> Result<Option<String>> {
    for path in [
        vault.join(paths::note_path_in(session_dir)),
        vault.join(paths::legacy_note_path_in(session_dir)),
    ] {
        match std::fs::read_to_string(&path) {
            Ok(content) => return Ok(Some(strip_leading_frontmatter(content))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(Error::Io(format!("failed to read note file: {e}"))),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_session(vault: &Path, id: &str, title: &str) {
        let dir = vault.join(format!("sessions/{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("_meta.json"),
            serde_json::json!({
                "id": id,
                "title": title,
                "started_at": null,
                "ended_at": null,
                "created_at": "2026-07-01T00:00:00Z",
                "tags": [],
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn list_session_metas_scans_and_skips_corrupt_entries() {
        let temp = tempfile::tempdir().unwrap();
        assert!(list_session_metas(temp.path()).unwrap().is_empty());

        seed_session(temp.path(), "s1", "Planning");
        seed_session(temp.path(), "s2", "Review");
        let broken = temp.path().join("sessions/broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("_meta.json"), "{ invalid").unwrap();
        let no_meta = temp.path().join("sessions/no-meta");
        std::fs::create_dir_all(&no_meta).unwrap();

        let mut ids: Vec<String> = list_session_metas(temp.path())
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["s1", "s2"]);
    }

    #[test]
    fn meta_roundtrip_preserves_unknown_fields() {
        let raw = serde_json::json!({
            "id": "s1",
            "title": "Planning",
            "started_at": null,
            "ended_at": null,
            "created_at": "2026-07-01T00:00:00Z",
            "tags": [],
            "speakers": { "t1": { "1": "raphael" } },
            "some_future_field": 42,
        });

        let meta: SessionMeta = serde_json::from_value(raw.clone()).unwrap();
        let round_tripped = serde_json::to_value(&meta).unwrap();

        assert_eq!(round_tripped["speakers"], raw["speakers"]);
        assert_eq!(round_tripped["some_future_field"], raw["some_future_field"]);
    }

    #[test]
    fn tag_suggestions_migrate_transcript_hash_to_combined_source_hash() {
        let state: TagSuggestionState = serde_json::from_value(serde_json::json!({
            "transcript_hash": "legacy-hash",
            "algorithm_version": 1,
            "status": "complete",
            "items": [],
        }))
        .unwrap();

        assert_eq!(state.source_hash, "legacy-hash");
        assert!(state.dismissed.is_empty());
        let serialized = serde_json::to_value(state).unwrap();
        assert_eq!(serialized["source_hash"], "legacy-hash");
        assert!(serialized.get("transcript_hash").is_none());
        assert!(serialized.get("dismissed").is_none());
    }

    /// `author` and `skill` are typed fields (absent = the vault owner wrote the
    /// note, no skill involved), not `extra` passengers -- and absence must
    /// serialize as no key, not `null`.
    #[test]
    fn author_and_skill_are_typed_and_absent_when_unset() {
        let raw = serde_json::json!({
            "id": "s1",
            "title": "Agent note",
            "started_at": null,
            "ended_at": null,
            "created_at": "2026-07-01T00:00:00Z",
            "tags": [],
            "author": "claude-code",
            "skill": "meeting-summarizer",
        });

        let meta: SessionMeta = serde_json::from_value(raw).unwrap();
        assert_eq!(meta.author.as_deref(), Some("claude-code"));
        assert_eq!(meta.skill.as_deref(), Some("meeting-summarizer"));
        assert!(!meta.extra.contains_key("author"));
        assert!(!meta.extra.contains_key("skill"));

        let round_tripped = serde_json::to_value(&meta).unwrap();
        assert_eq!(round_tripped["author"], "claude-code");
        assert_eq!(round_tripped["skill"], "meeting-summarizer");

        let mut unset = meta.clone();
        unset.author = None;
        unset.skill = None;
        let serialized = serde_json::to_value(&unset).unwrap();
        assert!(serialized.get("author").is_none());
        assert!(serialized.get("skill").is_none());
    }

    #[test]
    fn read_session_meta_distinguishes_absent_from_corrupt() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(read_session_meta(temp.path(), "ghost").unwrap(), None);

        seed_session(temp.path(), "s1", "Planning");
        assert_eq!(
            read_session_meta(temp.path(), "s1").unwrap().unwrap().title,
            "Planning"
        );

        std::fs::write(temp.path().join("sessions/s1/_meta.json"), "{ invalid").unwrap();
        assert!(read_session_meta(temp.path(), "s1").is_err());
    }

    #[test]
    fn read_note_strips_exporter_frontmatter() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(read_note(temp.path(), "s1").unwrap(), None);

        let dir = temp.path().join("sessions/s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("notes.md"),
            "---\nid: s1:note\nposition: 0\n---\n\nreal content",
        )
        .unwrap();
        assert_eq!(
            read_note(temp.path(), "s1").unwrap().unwrap(),
            "real content"
        );
    }

    #[test]
    fn read_note_falls_back_to_legacy_memo_file() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("sessions/s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("_memo.md"), "legacy note").unwrap();

        assert_eq!(
            read_note(temp.path(), "s1").unwrap().unwrap(),
            "legacy note"
        );
    }

    #[test]
    fn read_note_prefers_notes_md_when_both_files_exist() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("sessions/s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.md"), "current").unwrap();
        std::fs::write(dir.join("_memo.md"), "stale").unwrap();

        assert_eq!(read_note(temp.path(), "s1").unwrap().unwrap(), "current");
    }
}

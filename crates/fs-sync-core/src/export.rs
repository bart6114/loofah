//! Atomic, Drive-friendly file writes + soft-delete (never hard-delete on a
//! synced vault).
//!
//! Backs the desktop `session_store`'s vault writes
//! (`apps/desktop/src-tauri/src/session_store/`) and the
//! `write_document_batch`/`write_json_batch` Tauri commands in
//! `plugins/fs-sync/src/commands.rs`. The DB-to-vault render helpers that
//! used to live here died with the bidirectional sync machinery (Task 13/14).

use std::path::{Path, PathBuf};

/// Computes the sibling temp-file path `write_file_atomic` will stage
/// through before renaming into place. Exposed so callers that need to mark
/// it as an "own write" *before* the write happens (loop prevention) can
/// compute it once and pass the same path into `write_file_atomic`, rather
/// than each side independently generating a (different, nonce-based) tmp
/// path. Starts with `.tmp` to match both the repo's tempfile convention
/// (<https://docs.rs/tempfile/latest/tempfile/struct.Builder.html#method.prefix>)
/// and `plugins/notify`'s `should_skip_path`, which ignores any path whose
/// filename starts with `.tmp`.
pub fn tmp_sibling_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    path.with_file_name(format!(".tmp-{}-{nonce}-{file_name}", std::process::id()))
}

/// Writes `content` to `path` via a temp-file-then-rename (`tmp_path`, see
/// `tmp_sibling_path`) so a reader (or a sync client like Google Drive/iCloud)
/// never observes a partially written file. Creates the parent directory if
/// needed.
///
/// Returns `Ok(false)` without touching the filesystem when `path` already
/// holds byte-identical content, so callers replaying unchanged state never
/// generate spurious filesystem events.
///
/// When `path` exists with **different** content, the existing file is copied
/// to `<vault_base>/.trash/<date>/...` (via `copy_to_trash`) *before* the new
/// content is written — never silently overwritten. Writes are projections
/// of what the caller currently models: a legacy or hand-edited vault file
/// can carry frontmatter keys or JSON fields the caller doesn't know how to
/// reproduce, and those would otherwise be destroyed permanently and
/// irrecoverably on the very first write. Deletions already get this safety
/// (`move_to_trash` below); overwrites deserve the same.
pub fn write_file_atomic(
    vault_base: &Path,
    path: &Path,
    tmp_path: &Path,
    content: &[u8],
) -> crate::Result<bool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            crate::Error::Io(std::io::Error::new(
                error.kind(),
                format!(
                    "failed to create parent directory {} for {}: {error}",
                    parent.display(),
                    path.display()
                ),
            ))
        })?;
    }

    match std::fs::read(path) {
        Ok(existing) if existing == content => return Ok(false),
        Ok(_) => {
            copy_to_trash(vault_base, path)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    if let Some(parent) = tmp_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    hypr_storage::fs::write_staged_file(path, tmp_path, content)?;
    Ok(true)
}

/// Moves `path` (a file or a whole directory) to `<vault_base>/.trash/<UTC
/// date>/<relative path>`, creating parent directories as needed and
/// disambiguating with a numeric suffix if something is already there. Used
/// for every "the session data is gone" case — deletions must never destroy
/// vault content outright (Drive/iCloud-friendly, and it doubles as an undo
/// buffer). No-ops (returns `Ok(None)`) if `path` doesn't exist.
pub fn move_to_trash(vault_base: &Path, path: &Path) -> crate::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }

    let target = trash_destination(vault_base, path)?;
    hypr_storage::fs::rename_with_retry(path, &target)?;
    Ok(Some(target))
}

/// An overwrite backs up the old bytes without creating a gap at the live path.
/// A failed replacement (including a Windows sharing violation) leaves them readable.
pub fn copy_to_trash(vault_base: &Path, path: &Path) -> crate::Result<Option<PathBuf>> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        result => {
            result?;
        }
    }
    let target = trash_destination(vault_base, path)?;
    std::fs::copy(path, &target)?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&target)?
        .sync_all()?;
    Ok(Some(target))
}

fn trash_destination(vault_base: &Path, path: &Path) -> crate::Result<PathBuf> {
    let relative = path.strip_prefix(vault_base).unwrap_or(path);
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let mut target = vault_base.join(".trash").join(date).join(relative);

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    target = unique_path(target);
    Ok(target)
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }

    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("item")
        .to_string();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_string);

    let mut counter = 1;
    loop {
        let candidate_name = match &extension {
            Some(extension) => format!("{stem}-{counter}.{extension}"),
            None => format!("{stem}-{counter}"),
        };
        let candidate = path.with_file_name(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_via_tmp(vault_base: &Path, path: &Path, content: &[u8]) -> crate::Result<bool> {
        let tmp_path = tmp_sibling_path(path);
        write_file_atomic(vault_base, path, &tmp_path, content)
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn an_unreadable_file_is_not_replaced_even_when_its_handle_allows_delete() {
        use std::os::windows::fs::OpenOptionsExt;
        let vault = tempfile::tempdir().unwrap();
        let path = vault.path().join("notes.md");
        std::fs::write(&path, b"original notes").unwrap();
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(2 | 4)
            .open(&path)
            .unwrap();
        assert!(write_via_tmp(vault.path(), &path, b"replacement").is_err());
        drop(locked);
        assert_eq!(std::fs::read(&path).unwrap(), b"original notes");
        assert_eq!(std::fs::read_dir(vault.path()).unwrap().count(), 1);
    }

    #[test]
    fn write_file_atomic_creates_parent_dirs_and_writes_content() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join("dir").join("file.json");

        let wrote = write_via_tmp(temp.path(), &path, b"hello").unwrap();

        assert!(wrote);
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn write_file_atomic_skips_byte_identical_content() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file.json");
        std::fs::write(&path, b"same").unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));
        let wrote = write_via_tmp(temp.path(), &path, b"same").unwrap();

        assert!(!wrote);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
    }

    #[test]
    fn write_file_atomic_overwrites_changed_content_without_leaving_tmp_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file.json");
        std::fs::write(&path, b"old").unwrap();

        let wrote = write_via_tmp(temp.path(), &path, b"new").unwrap();

        assert!(wrote);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let leftovers = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    /// The critical fix from whole-branch review: writes are strict subset
    /// projections, so a byte-different overwrite must never just discard
    /// whatever was there before — it has to land in `.trash/` first.
    #[test]
    fn write_file_atomic_trashes_the_old_bytes_before_overwriting_changed_content() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        std::fs::create_dir_all(vault.join("sessions/abc")).unwrap();
        let path = vault.join("sessions/abc/notes.md");
        std::fs::write(
            &path,
            "---\nid: doc-1\ncustom_legacy_key: keep-me\n---\n\nOld body",
        )
        .unwrap();

        let wrote = write_via_tmp(vault, &path, b"---\nid: doc-1\n---\n\nNew body").unwrap();

        assert!(wrote);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "---\nid: doc-1\n---\n\nNew body"
        );
        let trashed = vault
            .join(".trash")
            .join(chrono::Utc::now().format("%Y-%m-%d").to_string())
            .join("sessions/abc/notes.md");
        assert!(trashed.is_file(), "old bytes should be preserved in .trash");
        let trashed_content = std::fs::read_to_string(&trashed).unwrap();
        assert!(trashed_content.contains("custom_legacy_key: keep-me"));
        assert!(trashed_content.contains("Old body"));
    }

    #[test]
    fn write_file_atomic_error_message_names_parent_and_target() {
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let target = blocker.join("child").join("file.json");

        let error = write_via_tmp(temp.path(), &target, b"x").unwrap_err();

        let message = error.to_string();
        assert!(message.contains("failed to create parent directory"));
        assert!(message.contains(&target.parent().unwrap().display().to_string()));
        assert!(message.contains(&target.display().to_string()));
    }

    #[test]
    fn tmp_sibling_path_starts_with_dot_tmp_matching_notify_skip_convention() {
        let path = Path::new("/vault/sessions/abc/_meta.json");

        let tmp = tmp_sibling_path(path);

        let name = tmp.file_name().and_then(|value| value.to_str()).unwrap();
        assert!(name.starts_with(".tmp"), "got {name}");
        assert_eq!(tmp.parent(), path.parent());
    }

    #[test]
    fn move_to_trash_relocates_under_dated_trash_dir_and_disambiguates() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        std::fs::create_dir_all(vault.join("sessions/abc")).unwrap();
        std::fs::write(vault.join("sessions/abc/_meta.json"), b"{}").unwrap();

        let moved = move_to_trash(vault, &vault.join("sessions/abc/_meta.json"))
            .unwrap()
            .unwrap();

        assert!(!vault.join("sessions/abc/_meta.json").exists());
        assert!(moved.starts_with(vault.join(".trash")));
        assert!(moved.ends_with("sessions/abc/_meta.json"));

        // A second file trashed at the same relative path the same day must
        // not clobber the first.
        std::fs::create_dir_all(vault.join("sessions/abc")).unwrap();
        std::fs::write(vault.join("sessions/abc/_meta.json"), b"{\"again\":true}").unwrap();
        let moved_again = move_to_trash(vault, &vault.join("sessions/abc/_meta.json"))
            .unwrap()
            .unwrap();

        assert_ne!(moved, moved_again);
        assert!(moved.exists());
        assert!(moved_again.exists());
    }

    #[test]
    fn move_to_trash_missing_path_is_a_noop() {
        let temp = tempfile::tempdir().unwrap();

        let result = move_to_trash(temp.path(), &temp.path().join("missing.json")).unwrap();

        assert_eq!(result, None);
    }
}

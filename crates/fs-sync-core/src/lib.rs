#[cfg(test)]
mod test_fixtures;

pub mod audio;
pub mod error;
pub mod export;
pub mod folder;
pub mod frontmatter;
pub mod json;
pub mod path;
pub mod runtime;
pub mod scan;
pub mod session;
pub mod session_content;
pub mod types;

pub use runtime::*;

pub use error::{Error, Result};
pub use path::{build_session_dir, is_uuid, normalize_folder_path, resolve_path_inside_base};
pub use session::find_session_dir;
pub use types::*;

use hypr_vault_read::layout::{eq_nfc, paths_eq_nfc};

use std::path::PathBuf;
use std::sync::Arc;

/// Optional fast-path session-directory resolver: `Ok(Some(abs_dir))` is used
/// verbatim, `Ok(None)` falls back to this crate's own `find_session_dir`
/// discovery (preserving its corrupt/ghost adoption semantics), and an `Err`
/// (e.g. a duplicate-claimed id) propagates. The desktop plugin installs one
/// backed by the store's warm location catalog; standalone users never pay for it.
pub type SessionDirResolver = Arc<dyn Fn(&str) -> Result<Option<PathBuf>> + Send + Sync>;

pub struct FsSyncCore {
    sessions_dir: PathBuf,
    resolver: Option<SessionDirResolver>,
}

impl FsSyncCore {
    pub fn new(base_dir: PathBuf) -> Self {
        let sessions_dir = base_dir.join("sessions");
        Self {
            sessions_dir,
            resolver: None,
        }
    }

    pub fn with_resolver(base_dir: PathBuf, resolver: SessionDirResolver) -> Self {
        let sessions_dir = base_dir.join("sessions");
        Self {
            sessions_dir,
            resolver: Some(resolver),
        }
    }

    pub fn list_folders(&self) -> Result<ListFoldersResult> {
        let mut result = ListFoldersResult {
            folders: std::collections::HashMap::new(),
            session_folder_map: std::collections::HashMap::new(),
        };

        if !self.sessions_dir.exists() {
            return Ok(result);
        }

        folder::scan_directory_recursive(&self.sessions_dir, "", &mut result);

        Ok(result)
    }

    pub fn move_session(
        &self,
        session_id: &str,
        from_folder_path: &str,
        target_folder_path: &str,
    ) -> Result<MoveSessionResult> {
        let from_folder_path = normalize_folder_path(from_folder_path)?;
        let target_folder_path = normalize_folder_path(target_folder_path)?;

        if eq_nfc(&from_folder_path, &target_folder_path) {
            return Err(Error::Path("session_move_noop".into()));
        }

        let source = self.resolve_session_dir(session_id)?;
        if !source.exists() {
            return Err(Error::Path("session_source_missing".into()));
        }

        // Moves keep the current physical basename: readable names survive the
        // move, and the name is never rebuilt from the id.
        let dir_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| Error::Path("session_dir_name_invalid".into()))?
            .to_string();

        let target = build_session_dir(&self.sessions_dir, &target_folder_path, &dir_name)?;
        if paths_eq_nfc(&source, &target) {
            return Err(Error::Path("session_move_noop".into()));
        }
        if target.exists() {
            return Err(Error::Path("session_target_exists".into()));
        }

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&source, &target)?;

        tracing::info!(
            "Moved session {} from {:?} to {:?}",
            session_id,
            source,
            target
        );

        Ok(MoveSessionResult {
            session_id: session_id.to_string(),
            folder_id: target_folder_path,
        })
    }

    pub fn create_folder(&self, folder_path: &str) -> Result<()> {
        let folder_path = normalize_folder_path(folder_path)?;
        let folder = self.sessions_dir.join(folder_path);

        if folder.exists() {
            return Ok(());
        }

        std::fs::create_dir_all(&folder)?;
        tracing::info!("Created folder: {:?}", folder);
        Ok(())
    }

    pub fn rename_folder(&self, old_path: &str, new_path: &str) -> Result<RenameFolderResult> {
        // The source may be a pre-existing hidden folder (rescuing it to a visible
        // name is the one way its sessions become reachable again); the target must
        // never be one.
        let old_path = path::normalize_existing_folder_path(old_path)?;
        let new_path = normalize_folder_path(new_path)?;

        if old_path.is_empty() || new_path.is_empty() {
            return Err(Error::Path("folder_rename_root_not_allowed".into()));
        }
        if eq_nfc(&old_path, &new_path) {
            return Err(Error::Path("folder_rename_noop".into()));
        }

        let source = self.sessions_dir.join(&old_path);
        let target = self.sessions_dir.join(&new_path);

        if !source.exists() {
            return Err(Error::Path("folder_source_missing".into()));
        }

        if target.exists() {
            return Err(Error::Path("folder_target_exists".into()));
        }

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::rename(&source, &target)?;
        tracing::info!("Renamed folder from {:?} to {:?}", source, target);

        let mut updates = Vec::new();
        folder::collect_session_updates(&self.sessions_dir, &new_path, &mut updates);
        updates.sort_by(|a, b| a.session_id.cmp(&b.session_id));

        Ok(RenameFolderResult { updates })
    }

    pub fn delete_folder(&self, folder_path: &str) -> Result<()> {
        // A pre-existing hidden folder stays deletable; the contains-sessions guard
        // below still blocks it while any (visible) session lives inside.
        let folder_path = path::normalize_existing_folder_path(folder_path)?;
        if folder_path.is_empty() {
            return Err(Error::Path("folder_delete_root_not_allowed".into()));
        }
        let folder = self.sessions_dir.join(folder_path);

        if !folder.exists() {
            return Ok(());
        }

        if self.folder_contains_sessions(&folder)? {
            return Err(Error::Path(
                "Cannot delete folder containing sessions. Move or delete sessions first."
                    .to_string(),
            ));
        }

        std::fs::remove_dir_all(&folder)?;
        tracing::info!("Deleted folder: {:?}", folder);
        Ok(())
    }

    /// Guards `delete_folder`: any directory holding `_meta.json` — readable-named,
    /// uuid-named, or with an unreadable meta — counts as a session and blocks the
    /// delete; only meta-less directories are traversed as folders. Hidden
    /// directories are invisible to every layout reader, so their contents never
    /// block an ordinary folder delete.
    fn folder_contains_sessions(&self, folder: &PathBuf) -> Result<bool> {
        let entries = std::fs::read_dir(folder)?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_none_or(|name| name.starts_with('.'))
            {
                continue;
            }

            if hypr_vault_read::has_session_boundary(&path) {
                return Ok(true);
            }
            if self.folder_contains_sessions(&path)? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub fn attachment_save(
        &self,
        session_id: &str,
        data: &[u8],
        filename: &str,
    ) -> Result<AttachmentSaveResult> {
        let session_dir = self.resolve_session_dir(session_id)?;
        let attachments_dir = session_dir.join("attachments");

        std::fs::create_dir_all(&attachments_dir)?;

        let safe_filename = sanitize_filename(filename)?;
        #[cfg(target_os = "windows")]
        let safe_filename = hypr_storage::fs::windows_safe_filename(&safe_filename);
        let (file_path, final_filename) =
            write_unique_file(&attachments_dir, &safe_filename, data)?;

        Ok(AttachmentSaveResult {
            path: file_path.to_string_lossy().to_string(),
            attachment_id: final_filename,
        })
    }

    pub fn attachment_import_path(
        &self,
        session_id: &str,
        source_path: &std::path::Path,
    ) -> Result<AttachmentInfo> {
        if !source_path.is_absolute() {
            return Err(Error::Path("attachment_source_path_not_absolute".into()));
        }

        let metadata = std::fs::metadata(source_path)?;
        if !metadata.is_file() {
            return Err(Error::Path("attachment_source_not_file".into()));
        }

        let filename = source_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| Error::Path("attachment_source_filename_invalid".into()))?;
        let safe_filename = sanitize_filename(filename)?;
        #[cfg(target_os = "windows")]
        let safe_filename = hypr_storage::fs::windows_safe_filename(&safe_filename);
        let session_dir = self.resolve_session_dir(session_id)?;
        let attachments_dir = session_dir.join("attachments");
        std::fs::create_dir_all(&attachments_dir)?;

        let mut source = std::fs::File::open(source_path)?;
        let (file_path, final_filename) =
            write_unique_file_with(&attachments_dir, &safe_filename, |destination| {
                std::io::copy(&mut source, destination).map(|_| ())
            })?;

        attachment_info(file_path, final_filename)
    }

    pub fn attachment_dir(&self, session_id: &str) -> Result<PathBuf> {
        let session_dir = self.resolve_session_dir(session_id)?;
        let attachments_dir = session_dir.join("attachments");
        std::fs::create_dir_all(&attachments_dir)?;
        Ok(attachments_dir)
    }

    pub fn attachment_list(&self, session_id: &str) -> Result<Vec<AttachmentInfo>> {
        let session_dir = self.resolve_session_dir(session_id)?;
        let attachments_dir = session_dir.join("attachments");

        let mut attachments = Vec::new();

        let entries = match std::fs::read_dir(&attachments_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(attachments),
            Err(e) => return Err(e.into()),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let filename = match path.file_name().and_then(|s| s.to_str()) {
                Some(name) if !name.starts_with('.') => name.to_string(),
                None => continue,
                Some(_) => continue,
            };

            let metadata = match entry.metadata() {
                Ok(metadata) if metadata.is_file() => metadata,
                _ => continue,
            };

            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_string();

            let modified_at = metadata
                .modified()
                .map(|t| {
                    chrono::DateTime::<chrono::Utc>::from(t)
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                })
                .unwrap_or_default();

            attachments.push(AttachmentInfo {
                attachment_id: filename,
                path: path.to_string_lossy().to_string(),
                extension,
                size: metadata.len(),
                modified_at,
            });
        }

        Ok(attachments)
    }

    pub fn attachment_read(&self, session_id: &str, attachment_id: &str) -> Result<Vec<u8>> {
        let session_dir = self.resolve_session_dir(session_id)?;
        let attachments_dir = session_dir.join("attachments");
        let safe_attachment_id = sanitize_filename(attachment_id)?;

        Ok(std::fs::read(attachments_dir.join(safe_attachment_id))?)
    }

    pub fn attachment_remove(&self, session_id: &str, attachment_id: &str) -> Result<()> {
        let session_dir = self.resolve_session_dir(session_id)?;
        let attachments_dir = session_dir.join("attachments");

        let entries = match std::fs::read_dir(&attachments_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let filename = match path.file_name().and_then(|s| s.to_str()) {
                Some(name) => name,
                None => continue,
            };

            if eq_nfc(filename, attachment_id) {
                let vault_base = self
                    .sessions_dir
                    .parent()
                    .ok_or_else(|| Error::Path("attachments_vault_base_missing".to_string()))?;
                export::move_to_trash(vault_base, &path)?;
                return Ok(());
            }
        }

        Ok(())
    }

    pub fn resolve_session_dir(&self, session_id: &str) -> Result<PathBuf> {
        // Validation stays in front of the override: a warm store catalog must not
        // make fs-sync accept ids that its own resolution would reject.
        if !is_uuid(session_id) {
            return Err(Error::Path("session_id_invalid".into()));
        }
        if let Some(resolver) = &self.resolver
            && let Some(dir) = resolver(session_id)?
        {
            return Ok(dir);
        }
        find_session_dir(&self.sessions_dir, session_id)
    }
}

fn sanitize_filename(filename: &str) -> Result<String> {
    let path = std::path::Path::new(filename);

    let clean_name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        Error::from(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid filename",
        ))
    })?;

    if clean_name.is_empty() || clean_name.contains(['/', '\\', '\0']) {
        return Err(Error::from(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid filename characters",
        )));
    }

    Ok(clean_name.to_string())
}

fn write_unique_file(
    dir: &std::path::Path,
    filename: &str,
    data: &[u8],
) -> Result<(PathBuf, String)> {
    use std::io::Write;

    write_unique_file_with(dir, filename, |file| file.write_all(data))
}

fn write_unique_file_with(
    dir: &std::path::Path,
    filename: &str,
    mut write_data: impl FnMut(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<(PathBuf, String)> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let (temporary_path, mut temporary_file) = loop {
        let path = dir.join(format!(".attachment-{}.tmp", uuid::Uuid::new_v4()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (path, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };

    if let Err(error) = write_data(&mut temporary_file).and_then(|_| temporary_file.flush()) {
        drop(temporary_file);
        remove_partial_attachment(&temporary_path);
        return Err(error.into());
    }
    drop(temporary_file);

    let path = std::path::Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    let extension = path.extension().and_then(|e| e.to_str());

    let mut counter = 0;
    loop {
        let candidate_filename = if counter == 0 {
            filename.to_string()
        } else {
            match extension {
                Some(ext) => format!("{} {}.{}", stem, counter, ext),
                None => format!("{} {}", stem, counter),
            }
        };

        let candidate_path = dir.join(&candidate_filename);

        match std::fs::hard_link(&temporary_path, &candidate_path) {
            Ok(()) => {
                remove_partial_attachment(&temporary_path);
                return Ok((candidate_path, candidate_filename));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                counter += 1;
                continue;
            }
            Err(e) => {
                remove_partial_attachment(&temporary_path);
                return Err(e.into());
            }
        }
    }
}

fn remove_partial_attachment(path: &std::path::Path) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(error = %error, "Failed to remove temporary attachment");
    }
}

fn attachment_info(path: PathBuf, attachment_id: String) -> Result<AttachmentInfo> {
    let metadata = std::fs::metadata(&path)?;
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_string();
    let modified_at = metadata
        .modified()
        .map(|time| {
            chrono::DateTime::<chrono::Utc>::from(time)
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        })
        .unwrap_or_default();

    Ok(AttachmentInfo {
        attachment_id,
        path: path.to_string_lossy().to_string(),
        extension,
        size: metadata.len(),
        modified_at,
    })
}

#[cfg(test)]
mod tests {
    use assert_fs::TempDir;
    use assert_fs::fixture::PathChild;
    use assert_fs::prelude::*;

    use super::*;
    use crate::test_fixtures::{UUID_1, UUID_2, session_meta_json};

    fn write_session_at(temp: &TempDir, relative_dir: &str, id: &str) {
        let dir = temp.child(relative_dir);
        dir.create_dir_all().unwrap();
        dir.child("_meta.json")
            .write_str(&session_meta_json(id))
            .unwrap();
    }

    #[test]
    fn move_session_to_folder() {
        let temp = TempDir::new().unwrap();
        write_session_at(&temp, &format!("sessions/{UUID_1}"), UUID_1);

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let result = core.move_session(UUID_1, "", "work").unwrap();

        temp.child("sessions")
            .child("work")
            .child(UUID_1)
            .assert(predicates::path::exists());
        assert_eq!(
            result,
            MoveSessionResult {
                session_id: UUID_1.into(),
                folder_id: "work".into(),
            }
        );
    }

    #[test]
    fn move_session_to_root() {
        let temp = TempDir::new().unwrap();
        write_session_at(&temp, &format!("sessions/work/{UUID_1}"), UUID_1);

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let result = core.move_session(UUID_1, "work", "").unwrap();

        temp.child("sessions")
            .child(UUID_1)
            .assert(predicates::path::exists());
        assert_eq!(
            result,
            MoveSessionResult {
                session_id: UUID_1.into(),
                folder_id: "".into(),
            }
        );
    }

    #[test]
    fn move_session_preserves_readable_basename() {
        let temp = TempDir::new().unwrap();
        write_session_at(&temp, "sessions/2026-03-20 — Planning — 550e84", UUID_1);

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let result = core.move_session(UUID_1, "", "work").unwrap();

        temp.child("sessions")
            .child("work")
            .child("2026-03-20 — Planning — 550e84")
            .assert(predicates::path::exists());
        temp.child("sessions")
            .child("work")
            .child(UUID_1)
            .assert(predicates::path::missing());
        assert_eq!(
            result,
            MoveSessionResult {
                session_id: UUID_1.into(),
                folder_id: "work".into(),
            }
        );
    }

    #[test]
    fn move_session_missing_source_errors() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions").create_dir_all().unwrap();

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let result = core.move_session(UUID_1, "", "work");

        assert!(matches!(result, Err(Error::Path(message)) if message == "session_source_missing"));
    }

    #[test]
    fn move_session_existing_target_errors() {
        let temp = TempDir::new().unwrap();
        write_session_at(&temp, "sessions/Planning", UUID_1);
        write_session_at(&temp, "sessions/work/Planning", UUID_2);

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let result = core.move_session(UUID_1, "", "work");

        assert!(matches!(result, Err(Error::Path(message)) if message == "session_target_exists"));
        // Never merge: both directories stay untouched.
        temp.child("sessions")
            .child("Planning")
            .child("_meta.json")
            .assert(predicates::path::exists());
        temp.child("sessions")
            .child("work")
            .child("Planning")
            .child("_meta.json")
            .assert(predicates::path::exists());
    }

    #[test]
    fn rename_folder_target_exists_errors() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions")
            .child("old")
            .create_dir_all()
            .unwrap();
        temp.child("sessions")
            .child("new")
            .create_dir_all()
            .unwrap();

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let result = core.rename_folder("old", "new");

        assert!(matches!(result, Err(Error::Path(message)) if message == "folder_target_exists"));
    }

    #[test]
    fn rename_folder_returns_updated_sessions() {
        let temp = TempDir::new().unwrap();
        write_session_at(&temp, &format!("sessions/old/{UUID_1}"), UUID_1);
        write_session_at(
            &temp,
            "sessions/old/nested/2026-04-01 — Retro — 550e84",
            UUID_2,
        );

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let result = core.rename_folder("old", "new").unwrap();

        assert_eq!(
            result,
            RenameFolderResult {
                updates: vec![
                    FolderSessionUpdate {
                        session_id: UUID_1.into(),
                        folder_id: "new".into(),
                    },
                    FolderSessionUpdate {
                        session_id: UUID_2.into(),
                        folder_id: "new/nested".into(),
                    },
                ],
            }
        );
    }

    #[test]
    fn delete_folder_with_sessions_errors() {
        let temp = TempDir::new().unwrap();
        write_session_at(&temp, &format!("sessions/work/{UUID_1}"), UUID_1);

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let result = core.delete_folder("work");

        assert!(result.is_err());
        temp.child("sessions")
            .child("work")
            .assert(predicates::path::exists());
    }

    #[test]
    fn delete_folder_with_only_readable_named_sessions_errors() {
        let temp = TempDir::new().unwrap();
        write_session_at(
            &temp,
            "sessions/work/2026-03-20 — Planning — 550e84",
            UUID_1,
        );
        write_session_at(
            &temp,
            "sessions/work/nested/2026-04-01 — Retro — 550e84",
            UUID_2,
        );

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let result = core.delete_folder("work");

        assert!(result.is_err());
        temp.child("sessions")
            .child("work")
            .child("2026-03-20 — Planning — 550e84")
            .child("_meta.json")
            .assert(predicates::path::exists());
        temp.child("sessions")
            .child("work")
            .child("nested")
            .child("2026-04-01 — Retro — 550e84")
            .child("_meta.json")
            .assert(predicates::path::exists());
    }

    #[test]
    fn delete_folder_blocked_by_session_with_unreadable_meta() {
        let temp = TempDir::new().unwrap();
        let broken = temp.child("sessions").child("work").child("Broken notes");
        broken.create_dir_all().unwrap();
        broken.child("_meta.json").write_str("{ invalid").unwrap();

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let result = core.delete_folder("work");

        assert!(result.is_err());
        broken.assert(predicates::path::exists());
    }

    #[test]
    fn delete_folder_ignores_sessions_hidden_inside_dot_directories() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions")
            .child("work")
            .create_dir_all()
            .unwrap();
        // A sync-provider sidecar holding session copies must not block the delete:
        // every layout reader treats dot-prefixed directories as invisible.
        write_session_at(
            &temp,
            &format!("sessions/work/.stversions/{UUID_1}"),
            UUID_1,
        );

        let core = FsSyncCore::new(temp.path().to_path_buf());
        core.delete_folder("work").unwrap();

        temp.child("sessions")
            .child("work")
            .assert(predicates::path::missing());
    }

    /// Vaults from before hidden paths were rejected can hold dot-named folders.
    /// The escape hatch: renaming them to a visible name (or deleting them when
    /// empty) must remain possible, while the contains-sessions guard still
    /// blocks deleting one with sessions inside.
    #[test]
    fn preexisting_hidden_folders_can_be_rescued_by_rename_or_deleted() {
        let temp = TempDir::new().unwrap();
        write_session_at(&temp, "sessions/.archive/keep", UUID_1);
        let core = FsSyncCore::new(temp.path().to_path_buf());

        assert!(core.delete_folder(".archive").is_err());

        core.rename_folder(".archive", "archive").unwrap();
        temp.child("sessions")
            .child("archive")
            .child("keep")
            .child("_meta.json")
            .assert(predicates::path::exists());

        temp.child("sessions")
            .child(".empty")
            .create_dir_all()
            .unwrap();
        core.delete_folder(".empty").unwrap();
        temp.child("sessions")
            .child(".empty")
            .assert(predicates::path::missing());
    }

    #[test]
    fn folder_operations_reject_hidden_paths() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions").create_dir_all().unwrap();
        let core = FsSyncCore::new(temp.path().to_path_buf());

        let result = core.create_folder(".trash");
        assert!(
            matches!(result, Err(Error::Path(message)) if message == "folder_path_hidden_not_allowed")
        );
        temp.child("sessions")
            .child(".trash")
            .assert(predicates::path::missing());

        temp.child("sessions")
            .child("work")
            .create_dir_all()
            .unwrap();
        let result = core.rename_folder("work", ".hidden");
        assert!(
            matches!(result, Err(Error::Path(message)) if message == "folder_path_hidden_not_allowed")
        );
    }

    #[test]
    fn create_folder_rejects_traversal() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions").create_dir_all().unwrap();
        let core = FsSyncCore::new(temp.path().to_path_buf());

        let result = core.create_folder("../outside");

        assert!(
            matches!(result, Err(Error::Path(message)) if message == "folder_path_traversal_not_allowed")
        );
        temp.child("outside").assert(predicates::path::missing());
    }

    #[test]
    fn delete_folder_rejects_traversal() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions").create_dir_all().unwrap();
        temp.child("outside").create_dir_all().unwrap();
        let core = FsSyncCore::new(temp.path().to_path_buf());

        let result = core.delete_folder("../outside");

        assert!(
            matches!(result, Err(Error::Path(message)) if message == "folder_path_traversal_not_allowed")
        );
        temp.child("outside").assert(predicates::path::exists());
    }

    #[test]
    fn delete_folder_rejects_root() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions").create_dir_all().unwrap();
        let core = FsSyncCore::new(temp.path().to_path_buf());

        let result = core.delete_folder("");

        assert!(
            matches!(result, Err(Error::Path(message)) if message == "folder_delete_root_not_allowed")
        );
        temp.child("sessions").assert(predicates::path::exists());
    }

    #[test]
    fn attachment_save_dedup_naming() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions")
            .child(UUID_1)
            .create_dir_all()
            .unwrap();

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let first = core.attachment_save(UUID_1, b"hello", "file.txt").unwrap();
        let second = core.attachment_save(UUID_1, b"world", "file.txt").unwrap();

        assert_eq!(first.attachment_id, "file.txt");
        assert_eq!(second.attachment_id, "file 1.txt");
        temp.child("sessions")
            .child(UUID_1)
            .child("attachments")
            .child("file.txt")
            .assert(predicates::path::exists());
        temp.child("sessions")
            .child(UUID_1)
            .child("attachments")
            .child("file 1.txt")
            .assert(predicates::path::exists());
    }

    #[test]
    fn attachment_save_removes_a_partial_candidate_after_write_failure() {
        let temp = TempDir::new().unwrap();
        let attachments_dir = temp.path().join("attachments");
        std::fs::create_dir_all(&attachments_dir).unwrap();

        let result = write_unique_file_with(&attachments_dir, "file.txt", |file| {
            use std::io::Write;

            file.write_all(b"par")?;
            Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "disk full",
            ))
        });

        assert!(matches!(
            result,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::StorageFull
        ));
        assert!(!attachments_dir.join("file.txt").exists());
        assert!(
            std::fs::read_dir(&attachments_dir)
                .unwrap()
                .next()
                .is_none()
        );
        let saved = write_unique_file(&attachments_dir, "file.txt", b"complete").unwrap();
        assert_eq!(saved.1, "file.txt");
    }

    #[test]
    fn attachment_import_path_validates_and_copies_regular_files() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions")
            .child(UUID_1)
            .create_dir_all()
            .unwrap();
        let source = temp.child("Résumé 📎.txt");
        source.write_str("native path").unwrap();
        let core = FsSyncCore::new(temp.path().to_path_buf());

        let imported = core.attachment_import_path(UUID_1, source.path()).unwrap();

        assert_eq!(imported.attachment_id, "Résumé 📎.txt");
        assert_eq!(imported.extension, "txt");
        assert_eq!(imported.size, 11);
        assert!(!imported.modified_at.is_empty());
        assert_eq!(std::fs::read(imported.path).unwrap(), b"native path");
    }

    #[test]
    fn attachment_import_path_rejects_relative_paths_and_directories() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions")
            .child(UUID_1)
            .create_dir_all()
            .unwrap();
        let core = FsSyncCore::new(temp.path().to_path_buf());

        assert!(matches!(
            core.attachment_import_path(UUID_1, std::path::Path::new("file.txt")),
            Err(Error::Path(message)) if message == "attachment_source_path_not_absolute"
        ));
        assert!(matches!(
            core.attachment_import_path(UUID_1, temp.path()),
            Err(Error::Path(message)) if message == "attachment_source_not_file"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn attachment_import_path_follows_symlinks_and_keeps_the_link_name() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions")
            .child(UUID_1)
            .create_dir_all()
            .unwrap();
        let source = temp.child("source.txt");
        source.write_str("linked").unwrap();
        let link = temp.path().join("alias.txt");
        std::os::unix::fs::symlink(source.path(), &link).unwrap();
        let core = FsSyncCore::new(temp.path().to_path_buf());

        let imported = core.attachment_import_path(UUID_1, &link).unwrap();

        assert_eq!(imported.attachment_id, "alias.txt");
        assert_eq!(std::fs::read(imported.path).unwrap(), b"linked");
    }

    #[test]
    fn attachment_import_path_deduplicates_names() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions")
            .child(UUID_1)
            .create_dir_all()
            .unwrap();
        let source = temp.child("file.txt");
        source.write_str("content").unwrap();
        let core = FsSyncCore::new(temp.path().to_path_buf());

        let first = core.attachment_import_path(UUID_1, source.path()).unwrap();
        let second = core.attachment_import_path(UUID_1, source.path()).unwrap();

        assert_eq!(first.attachment_id, "file.txt");
        assert_eq!(second.attachment_id, "file 1.txt");
    }

    #[test]
    fn atomic_attachment_writer_hides_partial_content() {
        let temp = TempDir::new().unwrap();
        let attachments_dir = temp.path().join("attachments");
        std::fs::create_dir_all(&attachments_dir).unwrap();

        write_unique_file_with(&attachments_dir, "file.txt", |file| {
            use std::io::Write;
            file.write_all(b"partial")?;
            assert!(!attachments_dir.join("file.txt").exists());
            Ok(())
        })
        .unwrap();

        assert_eq!(
            std::fs::read(attachments_dir.join("file.txt")).unwrap(),
            b"partial"
        );
        assert!(std::fs::read_dir(&attachments_dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.')
        }));
    }

    #[test]
    fn attachment_read_returns_saved_bytes() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions")
            .child(UUID_1)
            .create_dir_all()
            .unwrap();

        let core = FsSyncCore::new(temp.path().to_path_buf());
        core.attachment_save(UUID_1, b"hello", "image.png").unwrap();

        let bytes = core.attachment_read(UUID_1, "image.png").unwrap();

        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn attachment_dir_creates_and_returns_the_session_attachment_directory() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions")
            .child(UUID_1)
            .create_dir_all()
            .unwrap();

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let path = core.attachment_dir(UUID_1).unwrap();

        assert_eq!(
            path,
            temp.path()
                .join("sessions")
                .join(UUID_1)
                .join("attachments")
        );
        assert!(path.is_dir());
    }

    #[test]
    fn attachment_list_reports_size_and_skips_hidden_files_and_directories() {
        let temp = TempDir::new().unwrap();
        let attachments = temp.child("sessions").child(UUID_1).child("attachments");
        attachments.create_dir_all().unwrap();
        attachments
            .child("brief.txt")
            .write_binary(b"hello")
            .unwrap();
        attachments
            .child("no-extension")
            .write_binary(b"1234567")
            .unwrap();
        attachments
            .child(".DS_Store")
            .write_binary(b"hidden")
            .unwrap();
        attachments.child("nested").create_dir_all().unwrap();
        attachments
            .child("nested")
            .child("inside.pdf")
            .write_binary(b"nested")
            .unwrap();

        let core = FsSyncCore::new(temp.path().to_path_buf());
        let mut listed = core.attachment_list(UUID_1).unwrap();
        listed.sort_by(|left, right| left.attachment_id.cmp(&right.attachment_id));

        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].attachment_id, "brief.txt");
        assert_eq!(listed[0].extension, "txt");
        assert_eq!(listed[0].size, 5);
        assert_eq!(listed[1].attachment_id, "no-extension");
        assert_eq!(listed[1].extension, "");
        assert_eq!(listed[1].size, 7);
    }

    #[test]
    fn attachment_remove_moves_the_file_to_vault_trash() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions")
            .child(UUID_1)
            .create_dir_all()
            .unwrap();

        let core = FsSyncCore::new(temp.path().to_path_buf());
        core.attachment_save(UUID_1, b"recoverable", "brief.txt")
            .unwrap();

        core.attachment_remove(UUID_1, "brief.txt").unwrap();

        temp.child("sessions")
            .child(UUID_1)
            .child("attachments")
            .child("brief.txt")
            .assert(predicates::path::missing());
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        temp.child(".trash")
            .child(date)
            .child("sessions")
            .child(UUID_1)
            .child("attachments")
            .child("brief.txt")
            .assert(predicates::path::exists());
    }

    #[test]
    fn attachment_remove_missing_noop() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions")
            .child(UUID_1)
            .create_dir_all()
            .unwrap();

        let core = FsSyncCore::new(temp.path().to_path_buf());
        core.attachment_remove(UUID_1, "missing.txt").unwrap();
    }

    #[test]
    fn attachment_save_rejects_invalid_session_id() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions").create_dir_all().unwrap();
        let core = FsSyncCore::new(temp.path().to_path_buf());

        let result = core.attachment_save("../outside", b"hello", "file.txt");

        assert!(matches!(result, Err(Error::Path(message)) if message == "session_id_invalid"));
        temp.child("outside").assert(predicates::path::missing());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn attachment_save_uses_safe_windows_names_and_returns_the_actual_name() {
        let temp = TempDir::new().unwrap();
        temp.child("sessions")
            .child(UUID_1)
            .create_dir_all()
            .unwrap();
        let core = FsSyncCore::new(temp.path().to_path_buf());
        let attachment = core.attachment_save(UUID_1, b"notes", "NUL.txt").unwrap();
        assert_eq!(attachment.attachment_id, "_NUL.txt");
        temp.child("sessions")
            .child(UUID_1)
            .child("attachments/_NUL.txt")
            .assert("notes");
    }

    #[test]
    fn resolver_hit_is_used_verbatim_for_attachments_and_moves() {
        let temp = TempDir::new().unwrap();
        // The physical home has a name the fallback could never derive from the id;
        // only the injected resolver knows it.
        write_session_at(&temp, "sessions/2026-03-20 — Planning — 550e84", UUID_1);
        let hit = temp.path().join("sessions/2026-03-20 — Planning — 550e84");

        let resolver_hit = hit.clone();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let resolver_calls = calls.clone();
        let core = FsSyncCore::with_resolver(
            temp.path().to_path_buf(),
            Arc::new(move |id: &str| {
                resolver_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert_eq!(id, UUID_1);
                Ok(Some(resolver_hit.clone()))
            }),
        );

        core.attachment_save(UUID_1, b"hello", "file.txt").unwrap();
        assert!(hit.join("attachments/file.txt").is_file());

        core.move_session(UUID_1, "", "work").unwrap();
        temp.child("sessions")
            .child("work")
            .child("2026-03-20 — Planning — 550e84")
            .child("attachments")
            .child("file.txt")
            .assert(predicates::path::exists());
        assert!(calls.load(std::sync::atomic::Ordering::SeqCst) >= 2);
    }

    #[test]
    fn resolver_none_preserves_nested_unclaimed_dir_adoption() {
        // A nested uuid-named directory with a corrupt meta: the resolver misses,
        // and the fallback must still adopt it instead of inventing a root path.
        let temp = TempDir::new().unwrap();
        let corrupt = temp.path().join("sessions/work").join(UUID_1);
        std::fs::create_dir_all(&corrupt).unwrap();
        std::fs::write(corrupt.join("_meta.json"), "{ invalid").unwrap();

        let core =
            FsSyncCore::with_resolver(temp.path().to_path_buf(), Arc::new(|_id: &str| Ok(None)));

        assert_eq!(core.resolve_session_dir(UUID_1).unwrap(), corrupt);
    }

    #[test]
    fn resolver_errors_propagate_instead_of_falling_back() {
        let temp = TempDir::new().unwrap();
        write_session_at(&temp, &format!("sessions/{UUID_1}"), UUID_1);

        let core = FsSyncCore::with_resolver(
            temp.path().to_path_buf(),
            Arc::new(|_id: &str| Err(Error::Path("duplicate id".into()))),
        );

        assert!(matches!(
            core.resolve_session_dir(UUID_1),
            Err(Error::Path(message)) if message == "duplicate id"
        ));
    }

    #[test]
    fn resolver_is_never_consulted_for_an_invalid_session_id() {
        let temp = TempDir::new().unwrap();
        let core = FsSyncCore::with_resolver(
            temp.path().to_path_buf(),
            Arc::new(|_id: &str| panic!("a non-uuid id must be rejected before the resolver")),
        );

        assert!(matches!(
            core.resolve_session_dir("../outside"),
            Err(Error::Path(message)) if message == "session_id_invalid"
        ));
    }

    #[test]
    fn sanitize_filename_rejects_empty() {
        assert!(sanitize_filename("").is_err());
    }

    #[test]
    fn sanitize_filename_strips_directories() {
        let result = sanitize_filename("nested/path/file.txt").unwrap();
        assert_eq!(result, "file.txt");
    }
}

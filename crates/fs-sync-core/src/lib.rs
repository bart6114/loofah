#[cfg(test)]
mod test_fixtures;

pub mod audio;
pub mod error;
pub mod export;
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
pub use path::{is_uuid, resolve_path_inside_base};
pub use session::find_session_dir;
pub use types::*;

use hypr_vault_read::layout::eq_nfc;
use std::path::PathBuf;

pub struct FsSyncCore {
    sessions_dir: PathBuf,
}

impl FsSyncCore {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            sessions_dir: base_dir.join("sessions"),
        }
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
    use crate::test_fixtures::UUID_1;

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
    fn sanitize_filename_rejects_empty() {
        assert!(sanitize_filename("").is_err());
    }

    #[test]
    fn sanitize_filename_strips_directories() {
        let result = sanitize_filename("nested/path/file.txt").unwrap();
        assert_eq!(result, "file.txt");
    }
}

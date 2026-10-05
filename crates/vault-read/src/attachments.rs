use std::path::Path;

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    pub attachment_id: String,
    pub path: String,
    pub extension: String,
    pub size: u64,
    pub modified_at: String,
}

pub fn read_session_attachments(
    vault: &Path,
    session_id: &str,
) -> crate::Result<Vec<AttachmentInfo>> {
    let directory = vault.join(crate::paths::validated_session_dir(session_id)?);
    read_attachments_in(&directory).map_err(|error| crate::Error::Io(error.to_string()))
}

pub fn read_attachments_in(session_dir: &Path) -> std::io::Result<Vec<AttachmentInfo>> {
    let attachments_dir = session_dir.join("attachments");
    for directory in [
        session_dir.parent(),
        Some(session_dir),
        Some(attachments_dir.as_path()),
    ]
    .into_iter()
    .flatten()
    {
        match std::fs::symlink_metadata(directory) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Ok(Vec::new()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        }
    }
    let mut attachments = Vec::new();

    let entries = match std::fs::read_dir(&attachments_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(attachments),
        Err(e) => return Err(e),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let filename = match path.file_name().and_then(|s| s.to_str()) {
            Some(name) if !name.starts_with('.') && !name.contains('\\') => name.to_string(),
            None => continue,
            Some(_) => continue,
        };

        let metadata = match entry.metadata() {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_immediate_visible_files_in_canonical_attachments_directory() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sessions/s1");
        let attachments = directory.join("attachments");
        std::fs::create_dir_all(attachments.join("nested")).unwrap();
        std::fs::write(directory.join("photo.png"), b"loose user attachment").unwrap();
        std::fs::write(directory.join("notes.md"), b"user note").unwrap();
        std::fs::write(attachments.join("document.pdf"), b"document").unwrap();
        std::fs::write(attachments.join("image.png"), b"image").unwrap();
        std::fs::write(attachments.join(".hidden.png"), b"hidden").unwrap();
        std::fs::write(attachments.join("nested/deep.txt"), b"nested").unwrap();
        std::fs::write(attachments.join("unsafe\\name.txt"), b"ambiguous path").unwrap();
        let mut listed = read_session_attachments(root.path(), "s1").unwrap();
        listed.sort_by(|a, b| a.attachment_id.cmp(&b.attachment_id));
        assert_eq!(
            listed
                .iter()
                .map(|a| a.attachment_id.as_str())
                .collect::<Vec<_>>(),
            ["document.pdf", "image.png"]
        );
        assert_eq!(listed[0].size, 8);
        assert_eq!(listed[0].extension, "pdf");
        assert!(!listed[0].modified_at.is_empty());
        assert_eq!(Path::new(&listed[0].path), attachments.join("document.pdf"));
        assert!(
            read_session_attachments(root.path(), "missing")
                .unwrap()
                .is_empty()
        );
        assert!(read_session_attachments(root.path(), "../s1").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlink_files_and_symlinked_inventory_directories() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(external.path().join("private.txt"), b"private").unwrap();
        let directory = root.path().join("sessions/s1");
        let attachments = directory.join("attachments");
        std::fs::create_dir_all(&attachments).unwrap();
        symlink(
            external.path().join("private.txt"),
            attachments.join("link.txt"),
        )
        .unwrap();
        assert!(
            read_session_attachments(root.path(), "s1")
                .unwrap()
                .is_empty()
        );
        std::fs::remove_dir_all(&attachments).unwrap();
        symlink(external.path(), &attachments).unwrap();
        assert!(
            read_session_attachments(root.path(), "s1")
                .unwrap()
                .is_empty()
        );
        symlink(external.path(), root.path().join("sessions/s2")).unwrap();
        assert!(
            read_session_attachments(root.path(), "s2")
                .unwrap()
                .is_empty()
        );
        std::fs::remove_dir_all(root.path().join("sessions")).unwrap();
        symlink(external.path(), root.path().join("sessions")).unwrap();
        assert!(
            read_session_attachments(root.path(), "s1")
                .unwrap()
                .is_empty()
        );
    }
}

use std::{
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

pub struct StagedAttachment {
    path: Option<PathBuf>,
    directory: PathBuf,
}

impl StagedAttachment {
    pub fn prepare(
        directory: &Path,
        write: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> io::Result<Self> {
        ensure_directory(directory)?;
        let (path, file) = loop {
            let path = directory.join(format!(".attachment-{}.tmp", uuid::Uuid::new_v4()));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => break (path, file),
                Err(cause) if cause.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(cause) => return Err(cause),
            }
        };
        let staged = Self {
            path: Some(path),
            directory: directory.into(),
        };
        let mut file = file;
        let result = write(&mut file).and_then(|_| file.sync_all());
        // Close before cleanup/publication, including on Windows sharing violations.
        drop(file);
        result?;
        Ok(staged)
    }

    pub fn path(&self) -> &Path {
        self.path
            .as_deref()
            .expect("attachment has not been published")
    }

    pub fn publish_as(&mut self, filename: &str) -> io::Result<PathBuf> {
        if filename.is_empty()
            || filename == "."
            || filename == ".."
            || filename.contains(['/', '\\', '\0'])
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid attachment basename",
            ));
        }
        ensure_existing_directory(&self.directory)?;
        let source = self.path.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Attachment already published")
        })?;
        let destination = self.directory.join(filename);
        hypr_storage::fs::rename_no_replace(source, &destination)?;
        // An error flushing the directory must not remove an already published original.
        self.path = None;
        sync_directory(&self.directory)?;
        if let Some(parent) = self.directory.parent() {
            sync_directory(parent)?;
        }
        Ok(destination)
    }

    pub fn publish_unique(mut self, filename: &str) -> io::Result<(PathBuf, String)> {
        let filename = sanitize_filename(filename)?;
        for counter in 0..u64::MAX {
            let candidate = candidate_filename(&filename, counter);
            match self.publish_as(&candidate) {
                Ok(path) => return Ok((path, candidate)),
                Err(cause) if cause.kind() == io::ErrorKind::AlreadyExists => {}
                Err(cause) => return Err(cause),
            }
        }
        Err(io::Error::other("Attachment name space exhausted"))
    }
}

impl Drop for StagedAttachment {
    fn drop(&mut self) {
        if let Some(path) = &self.path
            && let Err(cause) = std::fs::remove_file(path)
            && cause.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(%cause, "Cannot remove temporary attachment");
        }
    }
}

pub fn sanitize_filename(filename: &str) -> io::Result<String> {
    let clean = basename(filename)?;
    #[cfg(windows)]
    return Ok(hypr_storage::fs::windows_safe_filename(clean));
    #[cfg(not(windows))]
    Ok(clean.into())
}

pub fn basename(filename: &str) -> io::Result<&str> {
    let clean = Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid attachment filename")
        })?;
    if clean.is_empty() || clean.contains(['/', '\\', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid attachment filename",
        ));
    }
    Ok(clean)
}

pub fn candidate_filename(filename: &str, counter: u64) -> String {
    if counter == 0 {
        return filename.into();
    }
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(filename);
    match path.extension().and_then(|value| value.to_str()) {
        Some(extension) => format!("{stem} {counter}.{extension}"),
        None => format!("{stem} {counter}"),
    }
}

fn ensure_directory(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => std::fs::create_dir_all(path)?,
        Err(cause) => return Err(cause),
        Ok(_) => {}
    }
    ensure_existing_directory(path)
}
fn ensure_existing_directory(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Attachment directory is not regular",
        ));
    }
    Ok(())
}
fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn staged_files_are_hidden_until_a_complete_exclusive_publish_and_collisions_keep_both_originals()
     {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("attachments");
        let mut staged = StagedAttachment::prepare(&directory, |file| {
            file.write_all(b"part")?;
            assert!(!directory.join("image 73.png").exists());
            file.write_all(b" complete")
        })
        .unwrap();
        assert!(
            staged
                .path()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with('.')
        );
        std::fs::write(directory.join("image 73.png"), b"User original").unwrap();
        assert_eq!(
            staged.publish_as("image 73.png").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(staged.path()).unwrap(), b"part complete");
        let (path, name) = staged.publish_unique("image 73.png").unwrap();
        assert_eq!(name, "image 73 1.png");
        assert_eq!(std::fs::read(path).unwrap(), b"part complete");
        assert_eq!(
            std::fs::read(directory.join("image 73.png")).unwrap(),
            b"User original"
        );
        assert_eq!(std::fs::read_dir(directory).unwrap().count(), 2);
    }

    #[test]
    fn failed_writes_and_abandoned_stages_remove_only_their_private_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let keep = root.path().join(".user-private");
        std::fs::write(&keep, b"Preserve").unwrap();
        let cause = StagedAttachment::prepare(root.path(), |file| {
            file.write_all(b"partial")?;
            Err(io::Error::new(io::ErrorKind::StorageFull, "Full device"))
        })
        .err()
        .unwrap();
        assert_eq!(cause.kind(), io::ErrorKind::StorageFull);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        let staged =
            StagedAttachment::prepare(root.path(), |file| file.write_all(b"Abandoned")).unwrap();
        let path = staged.path().to_owned();
        drop(staged);
        assert!(!path.exists());
        assert_eq!(std::fs::read(keep).unwrap(), b"Preserve");
    }

    #[test]
    fn unwinding_closes_the_writer_before_removing_its_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let result = std::panic::catch_unwind(|| {
            let _ = StagedAttachment::prepare(root.path(), |file| {
                file.write_all(b"Partial before panic")?;
                panic!("injected writer panic")
            });
        });
        assert!(result.is_err());
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[test]
    fn publication_never_interprets_a_basename_as_a_path() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("attachments");
        let mut staged =
            StagedAttachment::prepare(&directory, |file| file.write_all(b"Owned")).unwrap();
        for name in ["", ".", "..", "../notes.md", "nested/file", "a\\b", "a\0b"] {
            assert_eq!(
                staged.publish_as(name).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
            assert!(staged.path().exists());
        }
        let path = staged.publish_as("présentation été.pdf").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"Owned");
        assert!(!root.path().join("notes.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn linked_attachment_directories_are_refused_before_any_bytes_are_written() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = root.path().join("attachments");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        assert!(
            StagedAttachment::prepare(&link, |_| panic!("must not follow linked directory"))
                .is_err()
        );
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }
}

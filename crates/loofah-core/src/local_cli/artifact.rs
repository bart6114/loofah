use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

pub struct LocalCliArtifact {
    file: File,
    pub size_bytes: u64,
    pub sha256: String,
    pub trusted_path: Option<PathBuf>,
    owner: Box<dyn Send + 'static>,
}
impl LocalCliArtifact {
    pub fn new(
        file: File,
        size_bytes: u64,
        sha256: String,
        trusted_path: Option<PathBuf>,
        owner: Box<dyn Send + 'static>,
    ) -> Self {
        Self {
            file,
            size_bytes,
            sha256,
            trusted_path,
            owner,
        }
    }
    pub fn into_parts(self) -> (File, Box<dyn Send + 'static>) {
        (self.file, self.owner)
    }
}
impl Read for LocalCliArtifact {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(output)
    }
}
impl Seek for LocalCliArtifact {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(position)
    }
}

/// Keeps both the immutable copy and the original descriptor/admission alive
/// until the actual decoder/model worker releases its input.
pub struct MaterializedLocalArtifact {
    directory: tempfile::TempDir,
    path: PathBuf,
    _source: LocalCliArtifact,
}
impl MaterializedLocalArtifact {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    pub fn directory(&self) -> &std::path::Path {
        self.directory.path()
    }
}
impl LocalCliArtifact {
    pub fn materialize(
        mut self,
        filename: &str,
        temporary: PathBuf,
    ) -> Result<MaterializedLocalArtifact, hypr_loofah_protocol::ApplicationError> {
        use hypr_loofah_protocol::ErrorCode;
        let failure = |cause: &dyn std::fmt::Display| {
            crate::vault::error(
                ErrorCode::Unavailable,
                &format!("Local input could not be retained: {cause}"),
            )
        };
        if std::path::Path::new(filename)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(filename)
        {
            return Err(failure(&"Invalid input filename"));
        }
        for path in temporary.ancestors() {
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                    return Err(failure(&"Unsafe local media temporary directory"));
                }
                Ok(_) => {}
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                Err(cause) => return Err(failure(&cause)),
            }
        }
        std::fs::create_dir_all(&temporary).map_err(|cause| failure(&cause))?;
        let directory = tempfile::Builder::new()
            .prefix("local-media-")
            .tempdir_in(temporary)
            .map_err(|cause| failure(&cause))?;
        let path = directory.path().join(filename);
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|cause| failure(&cause))?;
        let size = self.size_bytes;
        self.rewind().map_err(|cause| failure(&cause))?;
        std::io::copy(&mut (&mut self).take(size.saturating_add(1)), &mut output)
            .map_err(|cause| failure(&cause))?;
        output.flush().map_err(|cause| failure(&cause))?;
        drop(output);
        let copied = hypr_fs_sync_core::media::metadata_from_reader(
            filename,
            "application/octet-stream",
            File::open(&path).map_err(|cause| failure(&cause))?,
        )
        .map_err(|cause| failure(&cause))?;
        if copied.sha256 != self.sha256 || copied.size_bytes != self.size_bytes {
            return Err(failure(&"Recording input changed before processing"));
        }
        Ok(MaterializedLocalArtifact {
            directory,
            path,
            _source: self,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Owner(Arc<AtomicBool>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    #[test]
    fn materialization_retains_original_authority_until_actual_input_owner_retires() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("input.wav");
        std::fs::write(&source, b"original source").unwrap();
        let file = File::open(&source).unwrap();
        let metadata =
            hypr_fs_sync_core::media::metadata_from_reader("input.wav", "audio/wav", &file)
                .unwrap();
        let retired = Arc::new(AtomicBool::new(false));
        let artifact = LocalCliArtifact::new(
            file,
            metadata.size_bytes,
            metadata.sha256,
            None,
            Box::new(Owner(retired.clone())),
        );
        let prepared = artifact
            .materialize("input.wav", root.path().join("private"))
            .unwrap();
        assert_eq!(std::fs::read(prepared.path()).unwrap(), b"original source");
        assert!(!retired.load(Ordering::Acquire));
        let path = prepared.path().to_owned();
        drop(prepared);
        assert!(retired.load(Ordering::Acquire));
        assert!(!path.exists());
    }
    #[test]
    fn materialization_rejects_changed_source_and_unsafe_destination_names() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("input.wav");
        std::fs::write(&source, b"original source").unwrap();
        let file = File::open(&source).unwrap();
        let metadata =
            hypr_fs_sync_core::media::metadata_from_reader("input.wav", "audio/wav", &file)
                .unwrap();
        let artifact = LocalCliArtifact::new(
            file,
            metadata.size_bytes,
            metadata.sha256,
            None,
            Box::new(()),
        );
        std::fs::write(&source, b"modified source").unwrap();
        assert!(
            artifact
                .materialize("input.wav", root.path().join("private"))
                .is_err()
        );
        let artifact = LocalCliArtifact::new(
            File::open(&source).unwrap(),
            0,
            "0".repeat(64),
            None,
            Box::new(()),
        );
        assert!(
            artifact
                .materialize("../escaped.wav", root.path().join("private"))
                .is_err()
        );
        assert!(!root.path().join("escaped.wav").exists());
    }
}

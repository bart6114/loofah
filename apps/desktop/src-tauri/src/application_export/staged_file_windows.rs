use std::{fs::File, io, path::Path};

pub(super) struct StagedFile {
    file: Option<tempfile::NamedTempFile>,
    original: same_file::Handle,
}
pub(super) struct PersistError {
    pub(super) file: StagedFile,
    pub(super) error: io::Error,
}
impl StagedFile {
    pub(super) fn create(root: &Path, prefix: &str) -> io::Result<Self> {
        let file = tempfile::Builder::new().prefix(prefix).tempfile_in(root)?;
        let original = same_file::Handle::from_file(file.as_file().try_clone()?)?;
        Ok(Self {
            file: Some(file),
            original,
        })
    }
    fn temporary(&self) -> &tempfile::NamedTempFile {
        self.file
            .as_ref()
            .expect("staged export has not been consumed")
    }
    pub(super) fn as_file(&self) -> &File {
        self.temporary().as_file()
    }
    pub(super) fn reopen(&self) -> io::Result<File> {
        // tempfile uses ReOpenFile on Windows, retaining the original file even
        // when the pathname has changed. Publication still checks its name.
        self.temporary().reopen()
    }
    fn owns_name(&self) -> io::Result<bool> {
        let path = self.temporary().path();
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Ok(false);
        }
        Ok(same_file::Handle::from_path(path)? == self.original)
    }
    fn check_name(&self) -> io::Result<()> {
        if self.owns_name()? {
            Ok(())
        } else {
            Err(io::Error::other("Export staging file changed"))
        }
    }
    pub(super) fn mark_download(&self) -> io::Result<()> {
        self.check_name()?;
        crate::remote::attachments::mark_download(self.temporary().path())
    }
    pub(super) fn persist_noclobber(mut self, path: &Path) -> Result<File, PersistError> {
        if let Err(error) = self.check_name() {
            return Err(PersistError { file: self, error });
        }
        match self
            .file
            .take()
            .expect("staged export has not been consumed")
            .persist_noclobber(path)
        {
            Ok(file) => Ok(file),
            Err(error) => {
                self.file = Some(error.file);
                Err(PersistError {
                    file: self,
                    error: error.error,
                })
            }
        }
    }
}
impl Drop for StagedFile {
    fn drop(&mut self) {
        // Preserve the original directory pin and same-owner/name check. This
        // refuses an already-replaced entry; it is not a same-UID race boundary.
        if self.file.is_some() && !self.owns_name().unwrap_or(false) {
            self.file.as_mut().unwrap().disable_cleanup(true);
        }
    }
}

use same_file::Handle;
use std::{
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
};

use crate::StoreError;

pub(crate) struct FileSnapshot {
    handle: Handle,
    metadata: Metadata,
    change: (i64, i64),
}

impl FileSnapshot {
    pub(crate) fn new(file: File) -> Result<Self, StoreError> {
        let metadata = file.metadata().map_err(io_error)?;
        let change = change_time(&file)?;
        Ok(Self {
            handle: Handle::from_file(file).map_err(io_error)?,
            metadata,
            change,
        })
    }
    pub(crate) fn file(&self) -> &File {
        self.handle.as_file()
    }
    pub(crate) fn hash(&self) -> Result<String, StoreError> {
        let mut file = self.file();
        file.seek(SeekFrom::Start(0)).map_err(io_error)?;
        hypr_fs_sync_core::media::metadata_from_reader(
            "attachment",
            "application/octet-stream",
            file.take(self.metadata.len().saturating_add(1)),
        )
        .map(|info| info.sha256)
        .map_err(io_error)
    }
    pub(crate) fn unchanged(&self, current: &Self) -> Result<bool, StoreError> {
        let hashed = self.file().metadata().map_err(io_error)?;
        let named = current.file().metadata().map_err(io_error)?;
        Ok(self.handle == current.handle
            && self.change == change_time(self.file())?
            && self.change == change_time(current.file())?
            && same_metadata(&self.metadata, &hashed)
            && same_metadata(&hashed, &named))
    }
    pub(crate) fn lock_for_recheck(&self) -> Result<bool, StoreError> {
        #[cfg(windows)]
        match self.file().try_lock_shared() {
            Ok(()) => Ok(true),
            Err(std::fs::TryLockError::WouldBlock) => Ok(false),
            Err(std::fs::TryLockError::Error(cause)) => Err(io_error(cause)),
        }
        #[cfg(not(windows))]
        Ok(true)
    }
}

fn same_metadata(a: &Metadata, b: &Metadata) -> bool {
    a.len() == b.len()
        && a.modified().ok() == b.modified().ok()
        && a.created().ok() == b.created().ok()
}
#[cfg(unix)]
fn change_time(file: &File) -> Result<(i64, i64), StoreError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().map_err(io_error)?;
    Ok((metadata.ctime(), metadata.ctime_nsec()))
}
#[cfg(not(unix))]
fn change_time(_: &File) -> Result<(i64, i64), StoreError> {
    Ok((0, 0))
}
fn io_error(cause: std::io::Error) -> StoreError {
    StoreError::Io(cause.to_string())
}

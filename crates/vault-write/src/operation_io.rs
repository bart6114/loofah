use std::{io::Read, path::Path};

use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{StoreError, media::open_regular_file};

pub(crate) fn operation_key(namespace: &str, request_id: uuid::Uuid) -> String {
    Sha256::digest(serde_json::to_vec(&(namespace, request_id)).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn read_receipt<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, StoreError> {
    let Some(file) = open_regular_file(path)? else {
        return Ok(None);
    };
    if file.metadata().map_err(io_error)?.len() > 64 * 1024 {
        return Err(StoreError::Io("Invalid operation receipt".into()));
    }
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > 64 * 1024 {
        return Err(StoreError::Io("Invalid operation receipt".into()));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| StoreError::Io("Invalid operation receipt".into()))
}

pub(crate) fn persist(path: &Path, receipt: &impl Serialize) -> Result<(), StoreError> {
    ensure_directory(
        path.parent()
            .ok_or_else(|| StoreError::Io("Missing receipt directory".into()))?,
    )?;
    hypr_storage::fs::atomic_write(
        path,
        &serde_json::to_string(receipt)
            .map_err(|cause| StoreError::Serialize(cause.to_string()))?,
    )
    .map_err(io_error)?;
    sync_parent(path)
}

pub(crate) fn ensure_directory(path: &Path) -> Result<(), StoreError> {
    if path.is_dir() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Io("Invalid receipt directory".into()))?;
    ensure_directory(parent)?;
    match std::fs::create_dir(path) {
        Ok(()) => sync_parent(path),
        Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        Err(cause) => Err(io_error(cause)),
    }
}

pub(crate) fn sync_parent(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    hypr_storage::fs::sync_directory_with_anchor(
        path.parent()
            .ok_or_else(|| StoreError::Io("Missing operation parent".into()))?,
        path,
    )
    .map_err(io_error)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn io_error(cause: std::io::Error) -> StoreError {
    StoreError::Io(cause.to_string())
}

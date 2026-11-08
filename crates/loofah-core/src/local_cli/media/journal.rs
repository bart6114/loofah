use crate::{VaultContext, vault::error};
use hypr_loofah_protocol::{ApplicationError, ErrorCode, LocalCliReply};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    fingerprint: String,
    result: Option<Result<LocalCliReply, ApplicationError>>,
}

pub(super) struct Claim {
    context: Arc<VaultContext>,
    path: PathBuf,
    record: Record,
}
impl Claim {
    pub async fn open(
        context: Arc<VaultContext>,
        root: PathBuf,
        request: &str,
        fingerprint: String,
    ) -> Result<Self, ApplicationError> {
        let request = request.to_owned();
        tokio::task::spawn_blocking(move || {
            let path = root.join(format!("{request}.json"));
            check_parents(&path)?;
            std::fs::create_dir_all(&root).map_err(io_error)?;
            let wanted = Record { version: 1, fingerprint, result: None };
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)] {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let record = match options.open(&path) {
                Ok(mut file) => {
                    file.write_all(&serde_json::to_vec(&wanted).map_err(io_error)?).map_err(io_error)?;
                    file.sync_all().map_err(io_error)?;
                    sync_parent(&path)?;
                    wanted
                }
                Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata = std::fs::symlink_metadata(&path).map_err(io_error)?;
                    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16 * 1024 * 1024 { return Err(super::unknown("Local media receipt requires review")); }
                    let mut bytes = Vec::new();
                    File::open(&path).map_err(io_error)?.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(io_error)?;
                    let current: Record = serde_json::from_slice(&bytes).map_err(|_| super::unknown("Local media receipt requires review"))?;
                    if current.version != 1 || current.fingerprint != wanted.fingerprint { return Err(error(ErrorCode::Conflict, "Local media request changed")); }
                    if current.result.is_none() { return Err(super::unknown("Local media work was admitted previously; inspect retained results before an explicit retry")); }
                    current
                }
                Err(cause) => return Err(io_error(cause)),
            };
            Ok(Self { context, path, record })
        }).await.map_err(io_error)?
    }
    pub fn result(&self) -> Option<&Result<LocalCliReply, ApplicationError>> {
        self.record.result.as_ref()
    }
    pub async fn finish(
        mut self,
        result: Result<LocalCliReply, ApplicationError>,
    ) -> Result<(), ApplicationError> {
        self.record.result = Some(result);
        tokio::task::spawn_blocking(move || {
            let _context = self.context;
            check_parents(&self.path)?;
            let bytes = serde_json::to_string(&self.record).map_err(io_error)?;
            hypr_storage::fs::atomic_write(&self.path, &bytes).map_err(|_| super::unknown("Local media work finished but its receipt could not be saved; inspect canonical content before retrying"))?;
            sync_parent(&self.path)
        }).await.map_err(io_error)?
    }
}
fn check_parents(path: &Path) -> Result<(), ApplicationError> {
    for parent in path.ancestors() {
        match std::fs::symlink_metadata(parent) {
            Ok(meta) if meta.file_type().is_symlink() || (parent != path && !meta.is_dir()) => {
                return Err(error(
                    ErrorCode::Unavailable,
                    "Local media operation storage is unsafe",
                ));
            }
            Ok(_) => {}
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(io_error(cause)),
        }
    }
    Ok(())
}
fn sync_parent(path: &Path) -> Result<(), ApplicationError> {
    #[cfg(unix)]
    {
        File::open(path.parent().unwrap())
            .and_then(|file| file.sync_all())
            .map_err(io_error)?;
    }
    Ok(())
}
fn io_error(cause: impl std::fmt::Display) -> ApplicationError {
    error(
        ErrorCode::Internal,
        &format!("Local media state could not be retained: {cause}"),
    )
}

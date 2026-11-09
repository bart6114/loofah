#[cfg(unix)]
use super::staged_file::StagedFile;
use super::*;
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path};
use tokio::io::AsyncWriteExt;
#[cfg(not(unix))]
type StagedFile = tempfile::NamedTempFile;

pub(super) fn image_references(
    input: &ExportInput,
) -> Result<Vec<(String, String)>, ApplicationError> {
    let mut images = Vec::new();
    for src in hypr_export_core::referenced_images(input) {
        let Some(encoded) = src.strip_prefix("attachments/") else {
            continue;
        };
        let name = percent_encoding::percent_decode_str(encoded)
            .decode_utf8()
            .map_err(|_| error(ErrorCode::Validation, "Invalid export image reference"))?
            .into_owned();
        hypr_loofah_protocol::validate_attachment_basename(&name)
            .map_err(|_| error(ErrorCode::Validation, "Invalid export image reference"))?;
        let extension = Path::new(&name)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !["png", "jpg", "jpeg", "gif", "svg", "webp"].contains(&extension.as_str()) {
            return Err(error(
                ErrorCode::Validation,
                "An embedded image format cannot be exported",
            ));
        }
        images.push((src, name));
        if images.len() > 32 {
            return Err(budget());
        }
    }
    Ok(images)
}
pub(super) async fn local_attachment(
    leases: &MediaLeases,
    context: Arc<VaultContext>,
    session: &str,
    name: &str,
    root: &Path,
    limit: u64,
    cancel: &CancellationToken,
) -> Result<(PathBuf, u64), ApplicationError> {
    let id = uuid::Uuid::new_v4().to_string();
    let descriptor = leases
        .open_attachment(context, "session-export", &id, session, name)
        .await?
        .ok_or_else(|| error(ErrorCode::NotFound, "An embedded image is missing"))?;
    let result = async {
        if descriptor.metadata.size_bytes > limit {
            return Err(budget());
        }
        let reader = leases.attachment_reader("session-export", &descriptor.vault_id, &id)?;
        let extension = Path::new(name)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("image");
        let path = root.join(format!("{id}.{extension}"));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .map_err(io_error)?;
        let copied = async {
            let mut hash = Sha256::new();
            let mut offset = 0;
            while offset < descriptor.metadata.size_bytes {
                if cancel.is_cancelled() {
                    return Err(error(ErrorCode::Cancelled, "Export cancelled"));
                }
                let chunk = reader
                    .read_chunk(
                        offset,
                        ((descriptor.metadata.size_bytes - offset).min(64 * 1024)) as usize,
                    )
                    .await?;
                if chunk.is_empty() {
                    return Err(unexpected());
                }
                hash.update(&chunk);
                file.write_all(&chunk).await.map_err(io_error)?;
                offset += chunk.len() as u64;
            }
            if hash
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
                != descriptor.metadata.sha256
            {
                return Err(error(
                    ErrorCode::Conflict,
                    "Embedded image bytes changed during export",
                ));
            }
            Ok::<_, ApplicationError>(())
        }
        .await;
        // Tokio writes retain a blocking operation after write_all returns.
        // Flush even failed/cancelled copies before releasing the private owner.
        let flushed = file.flush().await.map_err(io_error);
        copied?;
        flushed?;
        file.sync_all().await.map_err(io_error)?;
        drop(file);
        Ok((path, descriptor.metadata.size_bytes))
    }
    .await;
    let closed = leases.close_attachment("session-export", &descriptor.vault_id, &id);
    match result {
        Ok(value) => {
            closed?;
            Ok(value)
        }
        Err(error) => Err(error),
    }
}

pub(crate) struct StagedExport {
    file: StagedFile,
    root: PathBuf,
    identity: DirectoryIdentity,
    name: String,
    extension: String,
}
#[derive(PartialEq, Eq)]
struct DirectoryIdentity {
    canonical: PathBuf,
    handle: same_file::Handle,
}
fn directory(path: &Path) -> Result<DirectoryIdentity, ApplicationError> {
    if !path.is_absolute() {
        return Err(error(
            ErrorCode::Validation,
            "An absolute export directory is required",
        ));
    }
    for ancestor in path.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor).map_err(io_error)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(error(
                ErrorCode::Conflict,
                "Export directory is not a private directory",
            ));
        }
    }
    Ok(DirectoryIdentity {
        canonical: std::fs::canonicalize(path).map_err(io_error)?,
        handle: same_file::Handle::from_path(path).map_err(io_error)?,
    })
}
pub(super) fn stage(
    root: PathBuf,
    title: &str,
    extension: &str,
    bytes: Vec<u8>,
) -> Result<StagedExport, ApplicationError> {
    if bytes.len() > 128 * 1024 * 1024 {
        return Err(budget());
    }
    let parent = root.parent().ok_or_else(unexpected)?;
    directory(parent)?;
    match std::fs::create_dir(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error(error)),
    };
    let identity = directory(&root)?;
    let file = staging_file(&root, &identity, ".loofah-export-")?;
    file.as_file().write_all(&bytes).map_err(io_error)?;
    file.as_file().sync_all().map_err(io_error)?;
    let name = hypr_storage::fs::windows_safe_filename(title);
    let name = if name.trim().is_empty() {
        "Session".into()
    } else {
        name.chars()
            .scan(0usize, |bytes, character| {
                *bytes += character.len_utf8();
                (*bytes <= 120).then_some(character)
            })
            .collect()
    };
    Ok(StagedExport {
        file,
        root,
        identity,
        name,
        extension: extension.into(),
    })
}
pub(crate) fn stage_download(
    root: PathBuf,
    filename: &str,
) -> Result<(StagedExport, std::fs::File), ApplicationError> {
    hypr_loofah_protocol::validate_attachment_basename(filename)
        .map_err(|_| error(ErrorCode::Validation, "Invalid original media filename"))?;
    let parent = root.parent().ok_or_else(unexpected)?;
    directory(parent)?;
    match std::fs::create_dir(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error(error)),
    }
    let identity = directory(&root)?;
    let file = staging_file(&root, &identity, ".loofah-source-")?;
    let output = file.reopen().map_err(io_error)?;
    let filename = hypr_storage::fs::windows_safe_filename(filename);
    let path = Path::new(&filename);
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(unexpected)?
        .into();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .into();
    Ok((
        StagedExport {
            file,
            root,
            identity,
            name,
            extension,
        },
        output,
    ))
}
impl StagedExport {
    pub(crate) fn mark_download(&self) {
        #[cfg(unix)]
        let marked = self.file.mark_download();
        #[cfg(not(unix))]
        let marked = crate::remote::attachments::mark_download(self.file.path());
        if let Err(cause) = marked {
            tracing::warn!(%cause,"Could not apply original media download metadata");
        }
    }
}
pub(crate) fn publish(mut staged: StagedExport) -> Result<PathBuf, ApplicationError> {
    if directory(&staged.root)? != staged.identity {
        return Err(error(
            ErrorCode::Conflict,
            "Export directory changed before publication",
        ));
    }
    for suffix in 0..10000 {
        let stem = if suffix == 0 {
            staged.name.clone()
        } else {
            format!("{} ({suffix})", staged.name)
        };
        let name = if staged.extension.is_empty() {
            stem
        } else {
            format!("{stem}.{}", staged.extension)
        };
        let path = staged.root.join(&name);
        #[cfg(unix)]
        {
            match staged.file.publish(&name) {
                Ok(()) => {
                    staged.file.sync().map_err(io_error)?;
                    if directory(&staged.root)? != staged.identity {
                        return Err(error(
                            ErrorCode::Conflict,
                            "Export directory changed during publication",
                        ));
                    }
                    staged.file.keep();
                    return Ok(path);
                }
                Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(cause) => return Err(io_error(cause)),
            }
        }
        #[cfg(not(unix))]
        match staged.file.persist_noclobber(&path) {
            Ok(file) => {
                file.sync_all().map_err(io_error)?;
                #[cfg(unix)]
                std::fs::File::open(&staged.root)
                    .and_then(|directory| directory.sync_all())
                    .map_err(io_error)?;
                return Ok(path);
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                staged.file = error.file
            }
            Err(error) => return Err(io_error(error.error)),
        }
    }
    Err(error(
        ErrorCode::ResourceLimit,
        "Export filename attempts exhausted",
    ))
}

fn staging_file(
    root: &Path,
    identity: &DirectoryIdentity,
    prefix: &str,
) -> Result<StagedFile, ApplicationError> {
    #[cfg(unix)]
    let file = StagedFile::create(identity.handle.as_file(), prefix).map_err(io_error)?;
    #[cfg(not(unix))]
    let file = tempfile::Builder::new()
        .prefix(prefix)
        .tempfile_in(root)
        .map_err(io_error)?;
    if directory(root)? != *identity {
        return Err(error(
            ErrorCode::Conflict,
            "Export directory changed during staging",
        ));
    }
    Ok(file)
}

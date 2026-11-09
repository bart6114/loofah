#[cfg(unix)]
use std::fs::File;
use std::{io::Write, path::PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{WavePrefix, header, rejected};
use crate::{Result, capture_spool::CaptureWriter};
use hypr_loofah_protocol::ErrorCode;

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct WaveExportReceipt {
    pub frames: u64,
    pub pcm_bytes: u64,
    pub file_bytes: u64,
    pub sha256: String,
    pub sealed: bool,
}

pub async fn export_wave(writer: CaptureWriter, destination: PathBuf) -> Result<WaveExportReceipt> {
    run(
        writer,
        destination,
        #[cfg(test)]
        None,
    )
    .await
}

#[cfg(test)]
struct ExportPause {
    entered: tokio::sync::oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

async fn run(
    writer: CaptureWriter,
    destination: PathBuf,
    #[cfg(test)] pause: Option<ExportPause>,
) -> Result<WaveExportReceipt> {
    tokio::spawn(async move {
        if !destination.is_absolute() || destination.file_name().is_none() {
            return Err(rejected(
                ErrorCode::Validation,
                "Choose an absolute audio export destination",
            ));
        }
        let snapshot = writer.snapshot().await?;
        if snapshot.frames == 0 || snapshot.chunk_count == 0 {
            return Err(rejected(
                ErrorCode::Conflict,
                "This capture has no saved audio to export",
            ));
        }
        let header = header(&snapshot)?;
        let header_bytes = header.len() as u64;
        let parent = destination.parent().unwrap().to_path_buf();
        let target = destination.clone();
        let mut output = tokio::task::spawn_blocking(move || {
            match target.symlink_metadata() {
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "Choose a new filename",
                    ));
                }
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                Err(cause) => return Err(cause),
            }
            tempfile::Builder::new()
                .prefix(".loofah-audio-")
                .tempfile_in(parent)
        })
        .await
        .map_err(io_error)?
        .map_err(io_error)?;
        let mut encoded = Sha256::new();
        encoded.update(&header);
        output = write(output, header).await?;
        let mut prefix = WavePrefix::new(&snapshot);
        let mut source = Sha256::new();
        for sequence in 0..snapshot.chunk_count {
            let chunk = writer.read_chunk(sequence).await?;
            prefix.admit(&chunk, &snapshot)?;
            source.update(&chunk.bytes);
            encoded.update(&chunk.bytes);
            output = write(output, chunk.bytes).await?;
        }
        prefix.finish_snapshot(&snapshot)?;
        if let Some(seal) = &snapshot.seal {
            prefix.finish(seal)?;
            if hex(source) != seal.source_sha256 {
                return Err(rejected(
                    ErrorCode::Conflict,
                    "Capture source checksum changed",
                ));
            }
        }
        let receipt = WaveExportReceipt {
            frames: snapshot.frames,
            pcm_bytes: snapshot.pcm_bytes,
            file_bytes: snapshot.pcm_bytes + header_bytes,
            sha256: hex(encoded),
            sealed: snapshot.seal.is_some(),
        };
        tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            if let Some(pause) = pause {
                let _ = pause.entered.send(());
                let _ = pause.release.recv();
            }
            output.as_file().sync_all().map_err(io_error)?;
            let parent = destination.parent().unwrap();
            // Never overwrite a user file, even if it appeared while audio was verified.
            output
                .persist_noclobber(&destination)
                .map_err(|cause| io_error(cause.error))?;
            #[cfg(unix)]
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(io_error)?;
            #[cfg(not(unix))]
            let _ = parent;
            Ok(receipt)
        })
        .await
        .map_err(io_error)?
    })
    .await
    .map_err(io_error)?
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

async fn write(
    mut output: tempfile::NamedTempFile,
    bytes: Vec<u8>,
) -> Result<tempfile::NamedTempFile> {
    tokio::task::spawn_blocking(move || {
        output.write_all(&bytes).map_err(io_error)?;
        Ok(output)
    })
    .await
    .map_err(io_error)?
}
fn hex(hash: Sha256) -> String {
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn io_error(_: impl std::fmt::Display) -> hypr_loofah_protocol::ApplicationError {
    crate::failure(
        ErrorCode::Unavailable,
        "Audio could not be exported. The original device recording was retained. Choose a new filename if the destination already exists.",
    )
}

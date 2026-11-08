use super::{LocalCliArtifact, LocalMediaFuture};
use hypr_loofah_protocol::{ApplicationError, ErrorCode, LocalCliProgress};
use std::{io::Seek, path::PathBuf, sync::Arc};

/// Retains the original reader and private working directory until the decoder finishes.
pub fn normalize_media_artifact(
    source: LocalCliArtifact,
    filename: String,
    temporary: PathBuf,
    progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
) -> LocalMediaFuture<LocalCliArtifact> {
    Box::pin(async move {
        tokio::task::spawn_blocking(move || {
            let input = source.materialize(&filename, temporary)?;
            report(&progress, "importing audio", 0);
            let output = input.directory().join("audio.mp3");
            hypr_fs_sync_core::audio::import_audio(
                input.path(),
                &input.directory().join("encode.tmp"),
                &output,
            )
            .map_err(failure)?;
            let mut file = std::fs::File::open(output).map_err(failure)?;
            let metadata =
                hypr_fs_sync_core::media::metadata_from_reader("audio.mp3", "audio/mpeg", &file)
                    .map_err(failure)?;
            file.rewind().map_err(failure)?;
            report(&progress, "imported audio", 100);
            Ok(LocalCliArtifact::new(
                file,
                metadata.size_bytes,
                metadata.sha256,
                None,
                Box::new(input),
            ))
        })
        .await
        .map_err(failure)?
    })
}

fn report(progress: &Arc<dyn Fn(LocalCliProgress) + Send + Sync>, phase: &str, processed: usize) {
    progress(LocalCliProgress {
        schema_version: "2".into(),
        phase: phase.into(),
        processed,
        total: Some(100),
        added: 0,
        updated: 0,
        deleted: 0,
        elapsed_ms: 0,
    });
}
fn failure(_: impl std::fmt::Display) -> ApplicationError {
    crate::vault::error(ErrorCode::Validation, "Audio normalization failed")
}

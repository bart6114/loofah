use super::*;
use crate::attachment_upload::{DeviceAttachmentSource, UploadReader};
use std::io::{Read, Seek, Write};

pub(super) async fn retain(
    context: Arc<VaultContext>,
    mut source: UploadReader,
    input: UploadAttachmentRequest,
    temporary: PathBuf,
    device: DeviceAttachmentSource,
    browser_date: Option<String>,
) -> Result<(LocalCliArtifact, Option<String>), ApplicationError> {
    tokio::task::spawn_blocking(move || {
        for parent in temporary.ancestors() {
            match std::fs::symlink_metadata(parent) {
                Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                    return Err(invalid("Unsafe media staging directory"));
                }
                Ok(_) => {}
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(unavailable()),
            }
        }
        std::fs::create_dir_all(&temporary).map_err(|_| unavailable())?;
        let directory = tempfile::Builder::new()
            .prefix("media-")
            .tempdir_in(temporary)
            .map_err(|_| unavailable())?;
        let mut file = tempfile::tempfile_in(directory.path()).map_err(|_| unavailable())?;
        let copied = std::io::copy(
            &mut (&mut source).take(input.size_bytes.saturating_add(1)),
            &mut file,
        )
        .map_err(|_| invalid("Original media source changed"))?;
        file.flush().map_err(|_| unavailable())?;
        file.rewind().map_err(|_| unavailable())?;
        let actual = hypr_fs_sync_core::media::metadata_from_reader(
            &input.filename,
            "application/octet-stream",
            &mut file,
        )
        .map_err(|_| invalid("Original media source is unavailable"))?;
        if copied != input.size_bytes
            || actual.size_bytes != input.size_bytes
            || actual.sha256 != input.sha256
        {
            return Err(invalid("Original media source changed"));
        }
        file.rewind().map_err(|_| unavailable())?;
        let date = match device {
            DeviceAttachmentSource::Path { path } => {
                hypr_fs_sync_core::audio::source_metadata(std::path::Path::new(&path))
                    .ok()
                    .and_then(estimate)
            }
            DeviceAttachmentSource::Staged { .. } => browser_date,
        };
        if date
            .as_ref()
            .is_some_and(|value| chrono::DateTime::parse_from_rfc3339(value).is_err())
        {
            return Err(invalid("Invalid original audio date"));
        }
        Ok((
            LocalCliArtifact::new(
                file,
                input.size_bytes,
                input.sha256,
                None,
                Box::new((directory, source, context)),
            ),
            date,
        ))
    })
    .await
    .map_err(|_| unknown("Original media staging worker stopped"))?
}
fn estimate(metadata: hypr_fs_sync_core::audio::AudioSourceMetadata) -> Option<String> {
    let timestamp = metadata.created_at.or(metadata.modified_at)?;
    let timestamp = chrono::DateTime::parse_from_rfc3339(&timestamp)
        .ok()?
        .timestamp_millis();
    let duration = i64::try_from(metadata.duration_ms.unwrap_or(0)).ok()?;
    chrono::DateTime::from_timestamp_millis(timestamp.saturating_sub(duration).max(0))
        .map(|date| date.to_rfc3339())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imported_audio_date_prefers_creation_and_subtracts_duration_without_underflow() {
        assert_eq!(
            estimate(hypr_fs_sync_core::audio::AudioSourceMetadata {
                created_at: Some("2026-11-08T10:00:00Z".into()),
                modified_at: Some("2026-11-09T10:00:00Z".into()),
                duration_ms: Some(60_000)
            })
            .unwrap(),
            "2026-11-08T09:59:00+00:00"
        );
        assert_eq!(
            estimate(hypr_fs_sync_core::audio::AudioSourceMetadata {
                created_at: None,
                modified_at: Some("1970-01-01T00:00:01Z".into()),
                duration_ms: Some(60_000)
            })
            .unwrap(),
            "1970-01-01T00:00:00+00:00"
        );
    }
}

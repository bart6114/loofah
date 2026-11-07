use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, UploadAttachmentRequest, validate_upload_attachment_request,
};
use tauri::Manager;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) mod staging;

#[derive(Clone, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeviceAttachmentSource {
    Path { path: String },
    Staged { id: String },
}

pub(crate) enum UploadReader {
    Path(PathSource),
    Staged(staging::StageSource),
}
impl Read for UploadReader {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Path(source) => source.read(target),
            Self::Staged(source) => source.read(target),
        }
    }
}
impl UploadReader {
    pub(crate) fn admission(&self) -> Arc<OwnedSemaphorePermit> {
        match self {
            Self::Path(source) => source.admission(),
            Self::Staged(source) => source.admission(),
        }
    }
}

pub struct DeviceUploadIo {
    slots: Arc<Semaphore>,
    stages: staging::StagingRegistry,
}
impl Default for DeviceUploadIo {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(2)),
            stages: staging::StagingRegistry::default(),
        }
    }
}
pub(crate) struct PathSource {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    permit: Arc<OwnedSemaphorePermit>,
}
impl PathSource {
    pub(crate) fn admission(&self) -> Arc<OwnedSemaphorePermit> {
        self.permit.clone()
    }
}
impl Read for PathSource {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        if target.is_empty() {
            return Ok(0);
        }
        if self.file.is_none() {
            let file = open(&self.path)?;
            if file.metadata()?.len() != self.size {
                return Err(io::Error::other("Attachment source changed"));
            }
            self.file = Some(file);
        }
        self.file.as_mut().unwrap().read(target)
    }
}
impl DeviceUploadIo {
    pub(crate) fn reader(
        &self,
        owner: &str,
        source: DeviceAttachmentSource,
        input: &UploadAttachmentRequest,
    ) -> Result<UploadReader, ApplicationError> {
        match source {
            DeviceAttachmentSource::Path { path } => {
                self.source(path, input).map(UploadReader::Path)
            }
            DeviceAttachmentSource::Staged { id } => self
                .stages
                .reader(owner, &id, input, self.reserve()?)
                .map(UploadReader::Staged),
        }
    }

    pub(crate) fn release_window(&self, owner: &str) {
        self.stages.release_window(owner);
    }
    pub(crate) fn shutdown(&self) {
        self.stages.shutdown();
        self.slots.close();
    }
    fn reserve(&self) -> Result<Arc<OwnedSemaphorePermit>, ApplicationError> {
        self.slots
            .clone()
            .try_acquire_owned()
            .map(Arc::new)
            .map_err(|_| {
                let mut cause = hypr_loofah_client::failure(
                    ErrorCode::Unavailable,
                    "Device attachment transfers are busy; retry shortly",
                );
                cause.retryable = true;
                cause
            })
    }
    pub(crate) fn source(
        &self,
        source_path: String,
        input: &UploadAttachmentRequest,
    ) -> Result<PathSource, ApplicationError> {
        validate_upload_attachment_request(input).map_err(invalid)?;
        let path = path(source_path)?;
        if filename(&path)? != input.filename {
            return Err(invalid("Attachment source filename changed"));
        }
        Ok(PathSource {
            path,
            file: None,
            size: input.size_bytes,
            permit: self.reserve()?,
        })
    }
    async fn prepare(
        &self,
        source_path: String,
    ) -> Result<UploadAttachmentRequest, ApplicationError> {
        let path = path(source_path)?;
        let filename = filename(&path)?.to_owned();
        let permit = self.reserve()?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let file = open(&path).map_err(|_| source_error())?;
            let before = file.metadata().map_err(|_| source_error())?;
            let info = tauri_plugin_fs_sync::media::metadata_from_reader(
                &filename,
                "application/octet-stream",
                (&file).take(before.len().saturating_add(1)),
            )
            .map_err(|_| source_error())?;
            let after = file.metadata().map_err(|_| source_error())?;
            if info.size_bytes != before.len()
                || before.len() != after.len()
                || before.modified().ok() != after.modified().ok()
            {
                return Err(hypr_loofah_client::failure(
                    ErrorCode::Conflict,
                    "Attachment source changed while being reviewed",
                ));
            }
            let input = UploadAttachmentRequest {
                filename,
                size_bytes: info.size_bytes,
                sha256: info.sha256,
            };
            validate_upload_attachment_request(&input).map_err(invalid)?;
            Ok(input)
        })
        .await
        .map_err(|_| source_error())?
    }
}

#[tauri::command]
#[specta::specta]
pub async fn device_prepare_attachment_upload<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    source_path: String,
) -> Result<UploadAttachmentRequest, ApplicationError> {
    app.state::<DeviceUploadIo>().prepare(source_path).await
}

fn path(source_path: String) -> Result<PathBuf, ApplicationError> {
    let path = PathBuf::from(source_path);
    if !path.is_absolute() {
        return Err(invalid("Select an absolute device file path"));
    }
    Ok(path)
}
fn filename(path: &Path) -> Result<&str, ApplicationError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("Invalid attachment source filename"))
}
fn open(path: &Path) -> io::Result<File> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(io::Error::other("Attachment source is not a regular file"));
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Attachment source is not a regular file"));
    }
    Ok(file)
}
fn invalid(message: &str) -> ApplicationError {
    hypr_loofah_client::failure(ErrorCode::Validation, message)
}
fn source_error() -> ApplicationError {
    hypr_loofah_client::failure(
        ErrorCode::Unavailable,
        "Cannot read the selected device attachment",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_ipc_uploads_and_reconciles_without_login_or_reopening_a_completed_source() {
        use hypr_loofah_core::{
            VaultContext, search::SearchService, settings::ConfigState, startup::StartupState,
        };
        let vault = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let device = tempfile::tempdir().unwrap();
        let source = device.path().join("chart ü.pdf");
        std::fs::write(&source, b"payload").unwrap();
        let store = Arc::new(hypr_vault_write::SessionStore::new(vault.path().into()));
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Upload","created_at":"2026-11-07T10:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "Canonical note")
            .await
            .unwrap();
        let context = Arc::new(
            VaultContext::new(
                "vault".into(),
                store,
                StartupState::new(Some(vault.path())),
                Arc::new(ConfigState::load_or_default(vault.path())),
                Arc::new(SearchService::default()),
            )
            .with_operation_storage(runtime.path().into()),
        );
        let app = tauri::test::mock_builder()
            .manage(DeviceUploadIo::default())
            .manage(context)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let window = tauri::WebviewWindowBuilder::new(&app, "main", tauri::WebviewUrl::default())
            .build()
            .unwrap();
        let window = window.as_ref().window();
        let input =
            device_prepare_attachment_upload(app.handle().clone(), source.to_str().unwrap().into())
                .await
                .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let saved = crate::application::application_upload_attachment_source(
            app.handle().clone(),
            window.clone(),
            id.clone(),
            "note".into(),
            input.clone(),
            DeviceAttachmentSource::Path {
                path: source.to_str().unwrap().into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(saved.filename, input.filename);
        std::fs::remove_file(&source).unwrap();
        let replay = crate::application::application_upload_attachment_source(
            app.handle().clone(),
            window.clone(),
            id.clone(),
            "note".into(),
            input.clone(),
            DeviceAttachmentSource::Path {
                path: source.to_str().unwrap().into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(replay, saved);
        assert_eq!(
            crate::application::application_attachment_upload_status(
                app.handle().clone(),
                id,
                "note".into(),
                input
            )
            .await
            .unwrap(),
            hypr_loofah_protocol::AttachmentUploadStatus::Committed(saved)
        );
        let session = vault.path().join("sessions/note");
        assert_eq!(
            std::fs::read_dir(session.join("attachments"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            std::fs::read(session.join("notes.md")).unwrap(),
            b"Canonical note"
        );
        assert_eq!(app.state::<DeviceUploadIo>().slots.available_permits(), 2);
        let io = app.state::<DeviceUploadIo>();
        let source_id = uuid::Uuid::new_v4().to_string();
        let incoming = device.path().join("incoming");
        io.stages
            .begin(
                incoming.clone(),
                window.label().into(),
                source_id.clone(),
                "clipboard.png".into(),
                15,
                io.reserve().unwrap(),
            )
            .await
            .unwrap();
        io.stages
            .append(
                window.label(),
                &source_id,
                0,
                b"Clipboard bytes".to_vec(),
                io.reserve().unwrap(),
            )
            .await
            .unwrap();
        let input = io
            .stages
            .finish(window.label(), &source_id, io.reserve().unwrap())
            .await
            .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let metadata = crate::application::application_upload_attachment_source(
            app.handle().clone(),
            window.clone(),
            id.clone(),
            "note".into(),
            input.clone(),
            DeviceAttachmentSource::Staged {
                id: source_id.clone(),
            },
        )
        .await
        .unwrap();
        io.stages.close(window.label(), &source_id).await;
        assert_eq!(std::fs::read_dir(incoming).unwrap().count(), 0);
        assert_eq!(
            std::fs::read(session.join("attachments").join(&metadata.filename)).unwrap(),
            b"Clipboard bytes"
        );
        assert_eq!(
            crate::application::application_upload_attachment_source(
                app.handle().clone(),
                window,
                id,
                "note".into(),
                input,
                DeviceAttachmentSource::Staged { id: source_id }
            )
            .await
            .unwrap(),
            metadata
        );
        assert_eq!(
            std::fs::read_dir(session.join("attachments"))
                .unwrap()
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn device_review_and_lazy_reads_need_no_vault_or_login_and_retain_original_declarations()
    {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("chart ü.pdf");
        std::fs::write(&path, b"payload").unwrap();
        let io = DeviceUploadIo::default();
        let input = io.prepare(path.to_str().unwrap().into()).await.unwrap();
        assert_eq!(input.filename, "chart ü.pdf");
        assert_eq!(input.size_bytes, 7);
        let mut source = io.source(path.to_str().unwrap().into(), &input).unwrap();
        let mut bytes = Vec::new();
        source.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"payload");
        drop(source);
        let mut source = io.source(path.to_str().unwrap().into(), &input).unwrap();
        std::fs::write(&path, b"changed length").unwrap();
        assert!(source.read_to_end(&mut Vec::new()).is_err());
        drop(source);
        std::fs::remove_file(&path).unwrap();
        let source = io.source(path.to_str().unwrap().into(), &input).unwrap();
        drop(source);
        assert_eq!(io.slots.available_permits(), 2);
    }

    #[tokio::test]
    async fn source_admission_is_bounded_and_held_by_actual_readers_after_host_ownership_drops() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source.bin");
        std::fs::write(&path, b"").unwrap();
        let io = DeviceUploadIo::default();
        let input = io.prepare(path.to_str().unwrap().into()).await.unwrap();
        let first = io.source(path.to_str().unwrap().into(), &input).unwrap();
        let host = first.admission();
        let second = io.source(path.to_str().unwrap().into(), &input).unwrap();
        assert!(io.source(path.to_str().unwrap().into(), &input).is_err());
        drop(host);
        assert_eq!(io.slots.available_permits(), 0);
        drop(first);
        assert_eq!(io.slots.available_permits(), 1);
        drop(second);
        assert_eq!(io.slots.available_permits(), 2);
        assert!(io.prepare("relative.bin".into()).await.is_err());
        assert!(
            io.prepare(root.path().to_str().unwrap().into())
                .await
                .is_err()
        );
        let mut renamed = input;
        renamed.filename = "other.bin".into();
        assert!(io.source(path.to_str().unwrap().into(), &renamed).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn selected_source_symlinks_keep_the_selected_name_and_do_not_change_the_original_file() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original.bin");
        let selected = root.path().join("selected.pdf");
        std::fs::write(&original, b"original").unwrap();
        std::os::unix::fs::symlink(&original, &selected).unwrap();
        let io = DeviceUploadIo::default();
        let input = io.prepare(selected.to_str().unwrap().into()).await.unwrap();
        assert_eq!(input.filename, "selected.pdf");
        let mut source = io
            .source(selected.to_str().unwrap().into(), &input)
            .unwrap();
        let mut bytes = Vec::new();
        source.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"original");
        assert_eq!(std::fs::read(original).unwrap(), b"original");
    }
}

use super::*;
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{Seek, SeekFrom, Write},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tauri_plugin_fs_sync::attachment::StagedAttachment;
use tokio::sync::OwnedMutexGuard;
use tokio_util::sync::CancellationToken;

const CHUNK: usize = 64 * 1024;
const MAX_SOURCE_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_STAGED_BYTES: u64 = 2 * MAX_SOURCE_BYTES;

struct Budget {
    used: Arc<AtomicU64>,
    bytes: u64,
}
impl Drop for Budget {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
struct Row {
    file: File,
    _stage: StagedAttachment,
    received: u64,
    input: Option<UploadAttachmentRequest>,
}
struct Entry {
    owner: String,
    filename: String,
    size: u64,
    cancel: CancellationToken,
    row: Arc<tokio::sync::Mutex<Row>>,
    _slot: OwnedSemaphorePermit,
    _window_slot: OwnedSemaphorePermit,
    _budget: Budget,
}
type Entries = Arc<Mutex<HashMap<String, Arc<Entry>>>>;
pub(super) struct StagingRegistry {
    entries: Entries,
    windows: Mutex<HashMap<String, (Arc<Semaphore>, CancellationToken)>>,
    slots: Arc<Semaphore>,
    used: Arc<AtomicU64>,
    closed: AtomicBool,
}
impl Default for StagingRegistry {
    fn default() -> Self {
        Self {
            entries: Default::default(),
            windows: Default::default(),
            slots: Arc::new(Semaphore::new(32)),
            used: Arc::new(AtomicU64::new(0)),
            closed: AtomicBool::new(false),
        }
    }
}
impl Drop for StagingRegistry {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl StagingRegistry {
    fn entry(&self, owner: &str, id: &str) -> Result<Arc<Entry>, ApplicationError> {
        self.entries
            .lock()
            .unwrap()
            .get(id)
            .filter(|entry| entry.owner == owner && !entry.cancel.is_cancelled())
            .cloned()
            .ok_or_else(missing)
    }
    pub(super) async fn begin(
        &self,
        root: PathBuf,
        owner: String,
        id: String,
        filename: String,
        size: u64,
        permit: Arc<OwnedSemaphorePermit>,
    ) -> Result<(), ApplicationError> {
        uuid::Uuid::parse_str(&id).map_err(|_| invalid("A UUID device source ID is required"))?;
        hypr_loofah_protocol::validate_attachment_basename(&filename)
            .map_err(|_| invalid("Invalid attachment filename"))?;
        if size > MAX_SOURCE_BYTES {
            return Err(capacity());
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(missing());
        }
        if let Some(existing) = self.entries.lock().unwrap().get(&id) {
            return if existing.owner == owner
                && existing.filename == filename
                && existing.size == size
                && !existing.cancel.is_cancelled()
            {
                Ok(())
            } else {
                Err(conflict())
            };
        }
        let (window, lifetime) = {
            let mut windows = self.windows.lock().unwrap();
            windows
                .entry(owner.clone())
                .or_insert_with(|| (Arc::new(Semaphore::new(8)), CancellationToken::new()))
                .clone()
        };
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| capacity())?;
        let window_slot = window.try_acquire_owned().map_err(|_| capacity())?;
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(size)
                    .filter(|next| *next <= MAX_STAGED_BYTES)
            })
            .map_err(|_| capacity())?;
        let budget = Budget {
            used: self.used.clone(),
            bytes: size,
        };
        let entry = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let stage = StagedAttachment::prepare(&root, |_| Ok(())).map_err(|_| source_error())?;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(stage.path())
                .map_err(|_| source_error())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| source_error())?;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .map_err(|_| source_error())?;
            }
            Ok::<_, ApplicationError>(Arc::new(Entry {
                owner,
                filename,
                size,
                cancel: lifetime.child_token(),
                row: Arc::new(tokio::sync::Mutex::new(Row {
                    file,
                    _stage: stage,
                    received: 0,
                    input: None,
                })),
                _slot: slot,
                _window_slot: window_slot,
                _budget: budget,
            }))
        })
        .await
        .map_err(|_| source_error())??;
        if entry.cancel.is_cancelled() || self.closed.load(Ordering::Acquire) {
            return Err(missing());
        }
        let previous = {
            let mut entries = self.entries.lock().unwrap();
            if entry.cancel.is_cancelled() || self.closed.load(Ordering::Acquire) {
                return Err(missing());
            }
            if let Some(previous) = entries.get(&id) {
                Some(previous.clone())
            } else {
                entries.insert(id, entry.clone());
                None
            }
        };
        if let Some(previous) = previous
            && (previous.owner != entry.owner
                || previous.filename != entry.filename
                || previous.size != entry.size
                || previous.cancel.is_cancelled())
        {
            return Err(conflict());
        }
        Ok(())
    }
    pub(super) async fn append(
        &self,
        owner: &str,
        id: &str,
        offset: u64,
        data: Vec<u8>,
        permit: Arc<OwnedSemaphorePermit>,
    ) -> Result<u64, ApplicationError> {
        if data.is_empty() || data.len() > CHUNK {
            return Err(invalid(
                "Device source chunks must contain 1 to 65536 bytes",
            ));
        }
        let entry = self.entry(owner, id)?;
        let mut row = entry.row.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if entry.cancel.is_cancelled() {
                return Err(missing());
            }
            if row.input.is_some() {
                return Err(conflict());
            }
            let end = offset
                .checked_add(data.len() as u64)
                .filter(|end| *end <= entry.size)
                .ok_or_else(conflict)?;
            if offset < row.received && end <= row.received {
                let mut original = vec![0_u8; data.len()];
                row.file
                    .seek(SeekFrom::Start(offset))
                    .map_err(|_| source_error())?;
                row.file
                    .read_exact(&mut original)
                    .map_err(|_| source_error())?;
                return if original == data {
                    Ok(row.received)
                } else {
                    Err(conflict())
                };
            }
            if offset != row.received
                || row.file.metadata().map_err(|_| source_error())?.len() != offset
            {
                return Err(conflict());
            }
            row.file
                .seek(SeekFrom::Start(offset))
                .map_err(|_| source_error())?;
            if row.file.write_all(&data).is_err() {
                entry.cancel.cancel();
                return Err(source_error());
            }
            row.received = end;
            if entry.cancel.is_cancelled() {
                return Err(missing());
            }
            Ok(end)
        })
        .await
        .map_err(|_| source_error())?
    }
    pub(super) async fn finish(
        &self,
        owner: &str,
        id: &str,
        permit: Arc<OwnedSemaphorePermit>,
    ) -> Result<UploadAttachmentRequest, ApplicationError> {
        let entry = self.entry(owner, id)?;
        let mut row = entry.row.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if entry.cancel.is_cancelled() {
                return Err(missing());
            }
            if let Some(input) = &row.input {
                return Ok(input.clone());
            }
            if row.received != entry.size {
                return Err(conflict());
            }
            row.file.sync_all().map_err(|_| source_error())?;
            row.file
                .seek(SeekFrom::Start(0))
                .map_err(|_| source_error())?;
            let info = tauri_plugin_fs_sync::media::metadata_from_reader(
                &entry.filename,
                "application/octet-stream",
                (&row.file).take(entry.size.saturating_add(1)),
            )
            .map_err(|_| source_error())?;
            if info.size_bytes != entry.size || entry.cancel.is_cancelled() {
                return Err(conflict());
            }
            let input = UploadAttachmentRequest {
                filename: entry.filename.clone(),
                size_bytes: info.size_bytes,
                sha256: info.sha256,
            };
            row.input = Some(input.clone());
            Ok(input)
        })
        .await
        .map_err(|_| source_error())?
    }
    pub(super) fn reader(
        &self,
        owner: &str,
        id: &str,
        input: &UploadAttachmentRequest,
        permit: Arc<OwnedSemaphorePermit>,
    ) -> Result<StageSource, ApplicationError> {
        uuid::Uuid::parse_str(id).map_err(|_| invalid("A UUID device source ID is required"))?;
        validate_upload_attachment_request(input).map_err(invalid)?;
        Ok(StageSource {
            entries: self.entries.clone(),
            owner: owner.into(),
            id: id.into(),
            input: input.clone(),
            permit,
            opened: None,
        })
    }
    pub(super) async fn close(&self, owner: &str, id: &str) {
        let entry = {
            let mut entries = self.entries.lock().unwrap();
            if entries.get(id).is_some_and(|entry| entry.owner == owner) {
                entries.remove(id)
            } else {
                None
            }
        };
        if let Some(entry) = entry {
            entry.cancel.cancel();
            let _ = tauri::async_runtime::spawn_blocking(move || drop(entry)).await;
        }
    }
    pub(super) fn release_window(&self, owner: &str) {
        if let Some((_, lifetime)) = self.windows.lock().unwrap().remove(owner) {
            lifetime.cancel();
        }
        let removed = {
            let mut entries = self.entries.lock().unwrap();
            let ids: Vec<_> = entries
                .iter()
                .filter(|(_, entry)| entry.owner == owner)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| entries.remove(&id))
                .collect::<Vec<_>>()
        };
        for entry in &removed {
            entry.cancel.cancel();
        }
        cleanup(removed);
    }
    pub(super) fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        for (_, (_, lifetime)) in self.windows.lock().unwrap().drain() {
            lifetime.cancel();
        }
        let removed: Vec<_> = self
            .entries
            .lock()
            .unwrap()
            .drain()
            .map(|(_, entry)| entry)
            .collect();
        for entry in &removed {
            entry.cancel.cancel();
        }
        cleanup(removed);
    }
}

pub(crate) struct StageSource {
    entries: Entries,
    owner: String,
    id: String,
    input: UploadAttachmentRequest,
    permit: Arc<OwnedSemaphorePermit>,
    opened: Option<(OwnedMutexGuard<Row>, Arc<Entry>)>,
}
impl StageSource {
    pub(super) fn admission(&self) -> Arc<OwnedSemaphorePermit> {
        self.permit.clone()
    }
}
impl Read for StageSource {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        if target.is_empty() {
            return Ok(0);
        }
        if self.opened.is_none() {
            let entry = self
                .entries
                .lock()
                .unwrap()
                .get(&self.id)
                .filter(|entry| entry.owner == self.owner && !entry.cancel.is_cancelled())
                .cloned()
                .ok_or_else(|| io::Error::other("Device source is no longer available"))?;
            let mut row =
                entry.row.clone().try_lock_owned().map_err(|_| {
                    io::Error::new(io::ErrorKind::WouldBlock, "Device source is busy")
                })?;
            if row.input.as_ref() != Some(&self.input) {
                return Err(io::Error::other("Device source declaration changed"));
            }
            row.file.seek(SeekFrom::Start(0))?;
            self.opened = Some((row, entry));
        }
        let (row, entry) = self.opened.as_mut().unwrap();
        if entry.cancel.is_cancelled() {
            return Err(io::Error::other("Device source was closed"));
        }
        let read = row.file.read(target)?;
        if entry.cancel.is_cancelled() {
            return Err(io::Error::other("Device source was closed"));
        }
        Ok(read)
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn device_attachment_source_begin<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    source_id: String,
    filename: String,
    size_bytes: u64,
) -> Result<(), ApplicationError> {
    let root = app
        .path()
        .app_cache_dir()
        .map_err(|_| source_error())?
        .join("attachment-sources");
    let state = app.state::<DeviceUploadIo>();
    state
        .stages
        .begin(
            root,
            window.label().into(),
            source_id.clone(),
            filename,
            size_bytes,
            state.reserve()?,
        )
        .await?;
    if app.get_webview_window(window.label()).is_none() {
        state.stages.close(window.label(), &source_id).await;
        return Err(missing());
    }
    Ok(())
}
#[tauri::command]
#[specta::specta]
pub(crate) async fn device_attachment_source_append<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    source_id: String,
    offset: u64,
    data: Vec<u8>,
) -> Result<u64, ApplicationError> {
    let state = app.state::<DeviceUploadIo>();
    state
        .stages
        .append(window.label(), &source_id, offset, data, state.reserve()?)
        .await
}
#[tauri::command]
#[specta::specta]
pub(crate) async fn device_attachment_source_finish<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    source_id: String,
) -> Result<UploadAttachmentRequest, ApplicationError> {
    let state = app.state::<DeviceUploadIo>();
    state
        .stages
        .finish(window.label(), &source_id, state.reserve()?)
        .await
}
#[tauri::command]
#[specta::specta]
pub(crate) async fn device_attachment_source_close<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    source_id: String,
) {
    app.state::<DeviceUploadIo>()
        .stages
        .close(window.label(), &source_id)
        .await;
}
fn cleanup(entries: Vec<Arc<Entry>>) {
    if !entries.is_empty() {
        tauri::async_runtime::spawn_blocking(move || drop(entries));
    }
}
fn missing() -> ApplicationError {
    hypr_loofah_client::failure(ErrorCode::Cancelled, "Device source is no longer available")
}
fn conflict() -> ApplicationError {
    hypr_loofah_client::failure(
        ErrorCode::Conflict,
        "Device source bytes or declaration changed",
    )
}
fn capacity() -> ApplicationError {
    hypr_loofah_client::failure(
        ErrorCode::ResourceLimit,
        "Device attachment staging capacity reached",
    )
}

#[cfg(test)]
mod tests;

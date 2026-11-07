use super::*;
use hypr_loofah_protocol::{
    FileMetadata, RECORDING_CHUNK_LIMIT, RecordingLease, RecordingWaveform,
};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::http::{Method, Request, Response, StatusCode, header};
use tokio::sync::{OnceCell, OwnedSemaphorePermit, Semaphore};

#[cfg(test)]
mod tests;

#[derive(Clone, serde::Serialize, specta::Type)]
pub struct NativeRecording {
    pub id: String,
    pub metadata: FileMetadata,
}

pub(super) struct MediaRegistry {
    reservations: Mutex<()>,
    entries: Mutex<HashMap<String, Arc<MediaEntry>>>,
    slots: Arc<Semaphore>,
    reads: Arc<Semaphore>,
}
impl Default for MediaRegistry {
    fn default() -> Self {
        Self {
            reservations: Default::default(),
            entries: Default::default(),
            slots: Arc::new(Semaphore::new(64)),
            reads: Arc::new(Semaphore::new(8)),
        }
    }
}

struct MediaEntry {
    id: String,
    connection: String,
    owner: String,
    vault: String,
    session: String,
    client: Arc<RemoteClient>,
    cancel: CancellationToken,
    opened: AtomicBool,
    lease: OnceCell<Option<RecordingLease>>,
    reopen: tokio::sync::Mutex<()>,
    _request: PendingRequest,
    _slot: OwnedSemaphorePermit,
}
impl Drop for MediaEntry {
    fn drop(&mut self) {
        self.cancel.cancel();
        if self.opened.load(Ordering::Acquire) && !self.client.is_closed() {
            let client = self.client.clone();
            let vault = self.vault.clone();
            let id = self.id.clone();
            tauri::async_runtime::spawn(async move {
                let _ = client
                    .close_recording(&vault, &id, CancellationToken::new())
                    .await;
            });
        }
    }
}

impl MediaRegistry {
    pub(super) fn prune(&self) {
        let removed = {
            let mut entries = self.entries.lock().unwrap();
            let ids: Vec<_> = entries
                .iter()
                .filter(|(_, entry)| entry.cancel.is_cancelled())
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| entries.remove(&id))
                .collect::<Vec<_>>()
        };
        drop(removed);
    }
    pub(super) fn clear(&self) {
        let removed = std::mem::take(&mut *self.entries.lock().unwrap());
        drop(removed);
    }
    fn get(&self, owner: &str, id: &str) -> Result<Arc<MediaEntry>, ApplicationError> {
        self.entries
            .lock()
            .unwrap()
            .get(id)
            .filter(|entry| entry.owner == owner && !entry.cancel.is_cancelled())
            .cloned()
            .ok_or_else(unavailable_media)
    }
    fn remove(&self, owner: &str, connection: &str, id: &str) {
        let removed = {
            let mut entries = self.entries.lock().unwrap();
            if entries
                .get(id)
                .is_some_and(|entry| entry.owner == owner && entry.connection == connection)
            {
                entries.remove(id)
            } else {
                None
            }
        };
        if let Some(entry) = &removed {
            entry.cancel.cancel();
        }
        drop(removed);
    }
    fn reserve(
        &self,
        state: &RemoteState,
        connection: &str,
        owner: &str,
        id: String,
        vault: String,
        session: String,
    ) -> Result<Arc<MediaEntry>, ApplicationError> {
        let _reservation = self.reservations.lock().unwrap();
        self.prune();
        state.client(connection, owner)?;
        if !state
            .registry
            .lock()
            .unwrap()
            .get(connection)
            .is_some_and(|connection| connection.authenticated)
        {
            return Err(failure(
                ErrorCode::Unauthenticated,
                "Sign in before opening a recording",
            ));
        }
        if let Some(entry) = self.entries.lock().unwrap().get(&id).cloned() {
            if entry.owner != owner || entry.connection != connection {
                return Err(unavailable_media());
            }
            if entry.vault != vault || entry.session != session {
                return Err(failure(
                    ErrorCode::Conflict,
                    "Recording request identifies another resource",
                ));
            }
            return Ok(entry);
        }
        let (client, cancel, request) = state.admit(connection, owner, &id)?;
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| capacity())?;
        let entry = Arc::new(MediaEntry {
            id: id.clone(),
            connection: connection.into(),
            owner: owner.into(),
            vault,
            session,
            client,
            cancel,
            opened: AtomicBool::new(false),
            lease: OnceCell::new(),
            reopen: Default::default(),
            _request: request,
            _slot: slot,
        });
        {
            let mut entries = self.entries.lock().unwrap();
            if entries.contains_key(&id) {
                return Err(failure(
                    ErrorCode::Conflict,
                    "Recording request is already running",
                ));
            }
            if entries
                .values()
                .filter(|entry| entry.owner == owner)
                .count()
                >= 8
            {
                return Err(capacity());
            }
            entries.insert(id, entry.clone());
        }
        Ok(entry)
    }
}

impl MediaEntry {
    async fn open(&self) -> Result<Option<NativeRecording>, ApplicationError> {
        check_open(&self.cancel)?;
        let lease = self
            .lease
            .get_or_try_init(|| async {
                self.opened.store(true, Ordering::Release);
                self.client
                    .open_recording(&self.vault, &self.session, &self.id, self.cancel.clone())
                    .await
            })
            .await?;
        check_open(&self.cancel)?;
        Ok(lease.as_ref().map(|lease| NativeRecording {
            id: self.id.clone(),
            metadata: lease.metadata.clone(),
        }))
    }
    fn descriptor(&self) -> Result<&RecordingLease, ApplicationError> {
        check_open(&self.cancel)?;
        self.lease
            .get()
            .and_then(Option::as_ref)
            .ok_or_else(unavailable_media)
    }
    async fn reopen(&self) -> Result<(), ApplicationError> {
        let _guard = self.reopen.lock().await;
        let expected = self.descriptor()?;
        let current = self
            .client
            .open_recording(&self.vault, &self.session, &self.id, self.cancel.clone())
            .await?
            .ok_or_else(unavailable_media)?;
        if current.metadata != expected.metadata {
            return Err(failure(
                ErrorCode::Conflict,
                "Recording changed; reload playback before continuing",
            ));
        }
        Ok(())
    }
    async fn read(&self, offset: u64, count: usize) -> Result<Vec<u8>, ApplicationError> {
        let lease = self.descriptor()?;
        let result = self
            .client
            .read_recording_chunk(lease, offset, count, self.cancel.clone())
            .await;
        let bytes = match result {
            Err(error) if error.code == ErrorCode::NotFound => {
                self.reopen().await?;
                self.client
                    .read_recording_chunk(lease, offset, count, self.cancel.clone())
                    .await?
            }
            other => other?,
        };
        check_open(&self.cancel)?;
        Ok(bytes)
    }
    async fn head(&self) -> Result<(), ApplicationError> {
        let lease = self.descriptor()?;
        match self.client.head_recording(lease, self.cancel.clone()).await {
            Err(error) if error.code == ErrorCode::NotFound => {
                self.reopen().await?;
                self.client.head_recording(lease, self.cancel.clone()).await
            }
            other => other,
        }
    }
    async fn waveform(
        &self,
        cancel: CancellationToken,
    ) -> Result<RecordingWaveform, ApplicationError> {
        let lease = self.descriptor()?;
        let operation = async {
            match self.client.recording_waveform(lease, cancel.clone()).await {
                Err(error) if error.code == ErrorCode::NotFound => {
                    self.reopen().await?;
                    self.client.recording_waveform(lease, cancel).await
                }
                other => other,
            }
        };
        tokio::select! {
            _ = self.cancel.cancelled() => Err(unavailable_media()),
            result = operation => result,
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn remote_audio_open<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    resource_id: String,
    vault_id: String,
    session_id: String,
) -> Result<Option<NativeRecording>, ApplicationError> {
    let id = uuid::Uuid::parse_str(&resource_id)
        .map_err(|_| {
            failure(
                ErrorCode::Validation,
                "A UUID recording resource ID is required",
            )
        })?
        .to_string();
    let state = app.state::<RemoteState>();
    let entry = state.media.reserve(
        &state,
        &connection_id,
        window.label(),
        id.clone(),
        vault_id,
        session_id,
    )?;
    let result = entry.open().await;
    if !matches!(&result, Ok(Some(_))) {
        state.media.remove(window.label(), &connection_id, &id);
    }
    result
}

#[tauri::command]
#[specta::specta]
pub async fn remote_audio_peaks<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    resource_id: String,
    request_id: String,
) -> Result<RecordingWaveform, ApplicationError> {
    let resource_id = uuid::Uuid::parse_str(&resource_id)
        .map_err(|_| unavailable_media())?
        .to_string();
    if uuid::Uuid::parse_str(&request_id).is_err() {
        return Err(failure(
            ErrorCode::Validation,
            "A UUID request ID is required",
        ));
    }
    let state = app.state::<RemoteState>();
    let (_, cancel, _request) = state.admit(&connection_id, window.label(), &request_id)?;
    let entry = state.media.get(window.label(), &resource_id)?;
    if entry.connection != connection_id {
        return Err(unavailable_media());
    }
    entry.waveform(cancel).await
}

#[tauri::command]
#[specta::specta]
pub fn remote_audio_close<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    resource_id: String,
) {
    let Ok(resource_id) = uuid::Uuid::parse_str(&resource_id).map(|id| id.to_string()) else {
        return;
    };
    super::remote_cancel_request(
        app.clone(),
        window.clone(),
        connection_id.clone(),
        resource_id.clone(),
    );
    app.state::<RemoteState>()
        .media
        .remove(window.label(), &connection_id, &resource_id);
}

fn check_open(cancel: &CancellationToken) -> Result<(), ApplicationError> {
    if cancel.is_cancelled() {
        return Err(unavailable_media());
    }
    Ok(())
}
fn unavailable_media() -> ApplicationError {
    failure(ErrorCode::NotFound, "This recording is not available")
}

fn byte_range(value: Option<&str>, size: u64) -> Result<(u64, usize, bool), ()> {
    if size == 0 {
        return Err(());
    }
    let (start, end, requested) = if let Some(value) = value {
        if value.len() > 128 || value.contains(',') {
            return Err(());
        }
        let (start, end) = value
            .strip_prefix("bytes=")
            .and_then(|value| value.split_once('-'))
            .ok_or(())?;
        if start.is_empty() {
            let suffix: u64 = end.parse().map_err(|_| ())?;
            if suffix == 0 {
                return Err(());
            }
            (size.saturating_sub(suffix), size - 1, true)
        } else {
            let start: u64 = start.parse().map_err(|_| ())?;
            let end = if end.is_empty() {
                size - 1
            } else {
                end.parse::<u64>().map_err(|_| ())?.min(size - 1)
            };
            if start >= size || end < start {
                return Err(());
            }
            (start, end, true)
        }
    } else {
        (0, size - 1, false)
    };
    let count = (end - start + 1).min(RECORDING_CHUNK_LIMIT as u64) as usize;
    Ok((start, count, requested || count as u64 != size))
}

pub(super) async fn response(
    state: &RemoteState,
    owner: &str,
    origin: &str,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    if !request.body().is_empty()
        || request.headers().get(header::ORIGIN).is_some_and(|value| {
            value.to_str().ok().is_none_or(|value| {
                value != origin && !(origin == "tauri://localhost" && value == "null")
            })
        })
    {
        return Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Vec::new())
            .unwrap();
    }
    let result = media_response(state, owner, request).await;
    let mut response = match result {
        Ok(value) => value,
        Err(error) => {
            let status = match error.code {
                ErrorCode::NotFound => StatusCode::NOT_FOUND,
                ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
                ErrorCode::Forbidden => StatusCode::FORBIDDEN,
                ErrorCode::Conflict => StatusCode::CONFLICT,
                ErrorCode::ResourceLimit => StatusCode::TOO_MANY_REQUESTS,
                ErrorCode::Cancelled => StatusCode::GONE,
                _ => StatusCode::SERVICE_UNAVAILABLE,
            };
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_vec(&error).unwrap_or_default())
                .unwrap()
        }
    };
    response
        .headers_mut()
        .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.parse().unwrap());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    response
}

async fn media_response(
    state: &RemoteState,
    owner: &str,
    request: Request<Vec<u8>>,
) -> Result<Response<Vec<u8>>, ApplicationError> {
    let path = request
        .uri()
        .path()
        .strip_prefix('/')
        .ok_or_else(unavailable_media)?;
    if path.starts_with("attachments/")
        || path.starts_with("attachments%2F")
        || path.starts_with("attachments%2f")
    {
        return super::attachments::preview_response(state, owner, request).await;
    }
    if path.len() != 36 || request.uri().query().is_some() || uuid::Uuid::parse_str(path).is_err() {
        return Err(unavailable_media());
    }
    let entry = state.media.get(owner, path)?;
    let metadata = &entry.descriptor()?.metadata;
    let etag = format!("\"sha256:{}\"", metadata.sha256);
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, &metadata.content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, &etag);
    if request.method() != Method::GET
        && request.method() != Method::HEAD
        && request.method() != Method::OPTIONS
    {
        return Ok(response
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .header(header::ALLOW, "GET, HEAD, OPTIONS")
            .body(Vec::new())
            .unwrap());
    }
    if request.method() == Method::OPTIONS {
        return Ok(response
            .status(StatusCode::NO_CONTENT)
            .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS")
            .header(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                "Range, If-Match, If-Range",
            )
            .body(Vec::new())
            .unwrap());
    }
    if request
        .headers()
        .get(header::IF_MATCH)
        .is_some_and(|value| value.as_bytes() != etag.as_bytes())
    {
        return Ok(response
            .status(StatusCode::PRECONDITION_FAILED)
            .body(Vec::new())
            .unwrap());
    }
    let _permit = state
        .media
        .reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| capacity())?;
    if request.method() == Method::HEAD {
        entry.head().await?;
        check_open(&entry.cancel)?;
        return Ok(response
            .header(header::CONTENT_LENGTH, metadata.size_bytes)
            .body(Vec::new())
            .unwrap());
    }
    let range = request
        .headers()
        .get(header::RANGE)
        .map(|value| value.to_str().map_err(|_| ()))
        .transpose();
    let range = range.and_then(|range| byte_range(range, metadata.size_bytes));
    let (offset, length, partial) = match range {
        Ok(value) => value,
        Err(()) => {
            return Ok(response
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes */{}", metadata.size_bytes),
                )
                .body(Vec::new())
                .unwrap());
        }
    };
    let bytes = entry.read(offset, length).await?;
    if partial {
        response = response.status(StatusCode::PARTIAL_CONTENT).header(
            header::CONTENT_RANGE,
            format!(
                "bytes {offset}-{}/{}",
                offset + bytes.len() as u64 - 1,
                metadata.size_bytes
            ),
        );
    }
    Ok(response
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(bytes)
        .unwrap())
}

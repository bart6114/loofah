use super::*;
use hypr_loofah_protocol::{AttachmentLease, FileMetadata, validate_attachment_basename};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tauri::http::{Method, Request, Response, StatusCode, header};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const PREVIEW_LIMIT: u64 = 32 * 1024 * 1024;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct AttachmentTarget {
    pub vault_id: String,
    pub session_id: String,
    pub attachment_id: String,
}

#[derive(Clone, serde::Serialize, specta::Type)]
pub struct NativeAttachment {
    pub id: String,
    pub preview_limit_bytes: u64,
}

#[derive(Clone, serde::Serialize, specta::Type)]
pub struct NativeAttachmentCopy {
    pub copied_filename: String,
    pub metadata: FileMetadata,
}

#[derive(Clone, Copy, serde::Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentCopyAction {
    Save,
    Open,
    Reveal,
}

pub(super) struct AttachmentRegistry {
    entries: Arc<Mutex<HashMap<String, Arc<Entry>>>>,
    slots: Arc<Semaphore>,
    transfers: Arc<Semaphore>,
    copies: Arc<Semaphore>,
}

impl Default for AttachmentRegistry {
    fn default() -> Self {
        Self {
            entries: Default::default(),
            slots: Arc::new(Semaphore::new(512)),
            transfers: Arc::new(Semaphore::new(2)),
            // Leave a stream available for previews while an original copy downloads.
            copies: Arc::new(Semaphore::new(1)),
        }
    }
}

struct Entry {
    id: String,
    connection: String,
    owner: String,
    target: AttachmentTarget,
    client: Arc<RemoteClient>,
    cancel: CancellationToken,
    copy: tokio::sync::Mutex<Option<LocalCopy>>,
    leases: Arc<Mutex<HashSet<String>>>,
    _slot: OwnedSemaphorePermit,
}

#[derive(Clone)]
struct LocalCopy {
    path: PathBuf,
    metadata: FileMetadata,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl AttachmentRegistry {
    pub(super) fn clear(&self) {
        for (_, entry) in self.entries.lock().unwrap().drain() {
            entry.cancel.cancel();
        }
    }

    pub(super) fn prune(&self, state: &RemoteState) {
        let connections = state.registry.lock().unwrap();
        self.entries.lock().unwrap().retain(|_, entry| {
            let keep = !entry.cancel.is_cancelled()
                && !entry.client.is_closed()
                && connections
                    .get(&entry.connection)
                    .is_some_and(|connection| {
                        connection.authenticated && connection.leases.contains_key(&entry.owner)
                    });
            if !keep {
                entry.cancel.cancel();
            }
            keep
        });
    }

    pub(super) async fn cancel_connection(&self, connection: &str) {
        let removed = {
            let mut entries = self.entries.lock().unwrap();
            let ids: Vec<_> = entries
                .iter()
                .filter(|(_, entry)| entry.connection == connection)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| entries.remove(&id))
                .collect::<Vec<_>>()
        };
        let mut cleanup = tokio::task::JoinSet::new();
        for entry in removed {
            entry.cancel.cancel();
            let leases: Vec<_> = entry.leases.lock().unwrap().iter().cloned().collect();
            for id in leases {
                let client = entry.client.clone();
                let vault = entry.target.vault_id.clone();
                cleanup.spawn(async move {
                    let _ = client
                        .close_attachment(&vault, &id, CancellationToken::new())
                        .await;
                });
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while cleanup.join_next().await.is_some() {}
        })
        .await;
    }

    fn reserve(
        &self,
        state: &RemoteState,
        connection: &str,
        owner: &str,
        id: &str,
        target: AttachmentTarget,
    ) -> Result<Arc<Entry>, ApplicationError> {
        uuid::Uuid::parse_str(id)
            .map_err(|_| invalid("A UUID attachment resource ID is required"))?;
        validate_attachment_basename(&target.attachment_id)
            .map_err(|_| invalid("Invalid attachment ID"))?;
        for id in [&target.vault_id, &target.session_id] {
            if id.is_empty()
                || id.len() > 256
                || id == "."
                || id == ".."
                || id.contains(['/', '\\', '\0'])
            {
                return Err(invalid("Invalid attachment scope"));
            }
        }
        self.prune(state);
        let cancel = state.window_lifetime(owner).child_token();
        let mut connections = state.registry.lock().unwrap();
        let connected = connections.get_mut(connection).ok_or_else(missing)?;
        if !connected.leases.contains_key(owner) || connected.client.is_closed() {
            return Err(missing());
        }
        if !connected.authenticated {
            return Err(failure(
                ErrorCode::Unauthenticated,
                "Sign in before opening an attachment",
            ));
        }
        if !connected
            .client
            .capabilities()
            .features
            .iter()
            .any(|value| value == "attachment_leases")
        {
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support attachments. Upgrade the server, then reconnect.",
            ));
        }
        connected
            .cancelled
            .retain(|_, at| at.elapsed() < Duration::from_secs(60));
        if connected
            .cancelled
            .remove(&(owner.into(), id.into()))
            .is_some()
        {
            return Err(closed());
        }
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.get(id) {
            if entry.connection != connection || entry.owner != owner || entry.target != target {
                return Err(failure(
                    ErrorCode::Conflict,
                    "Attachment resource identifies another scope",
                ));
            }
            return Ok(entry.clone());
        }
        if entries
            .values()
            .filter(|entry| entry.owner == owner)
            .count()
            >= 128
        {
            return Err(busy());
        }
        let slot = self.slots.clone().try_acquire_owned().map_err(|_| busy())?;
        let entry = Arc::new(Entry {
            id: id.into(),
            connection: connection.into(),
            owner: owner.into(),
            target,
            client: connected.client.clone(),
            cancel,
            copy: Default::default(),
            leases: Default::default(),
            _slot: slot,
        });
        entries.insert(id.into(), entry.clone());
        Ok(entry)
    }

    fn get(
        &self,
        state: &RemoteState,
        owner: &str,
        id: &str,
    ) -> Result<Arc<Entry>, ApplicationError> {
        let connections = state.registry.lock().unwrap();
        let entries = self.entries.lock().unwrap();
        let entry = entries
            .get(id)
            .filter(|entry| entry.owner == owner)
            .ok_or_else(closed)?;
        if !connections
            .get(&entry.connection)
            .is_some_and(|connection| {
                connection.authenticated && connection.leases.contains_key(owner)
            })
        {
            return Err(closed());
        }
        entry.check(None)?;
        Ok(entry.clone())
    }

    fn close(&self, state: &RemoteState, owner: &str, connection: &str, id: &str) {
        if uuid::Uuid::parse_str(id).is_err() {
            return;
        }
        let mut connections = state.registry.lock().unwrap();
        let Some(connected) = connections
            .get_mut(connection)
            .filter(|entry| entry.leases.contains_key(owner))
        else {
            return;
        };
        let mut entries = self.entries.lock().unwrap();
        if entries
            .get(id)
            .is_some_and(|entry| entry.owner == owner && entry.connection == connection)
        {
            if let Some(entry) = entries.remove(id) {
                entry.cancel.cancel();
            }
        }
        if connected.cancelled.len() < 256 {
            connected
                .cancelled
                .insert((owner.into(), id.into()), Instant::now());
        }
    }
}

struct ServerLease {
    client: Arc<RemoteClient>,
    vault: String,
    id: String,
    closed: bool,
    leases: Arc<Mutex<HashSet<String>>>,
}

impl ServerLease {
    fn new(entry: &Entry) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        entry.leases.lock().unwrap().insert(id.clone());
        Self {
            client: entry.client.clone(),
            vault: entry.target.vault_id.clone(),
            id,
            closed: false,
            leases: entry.leases.clone(),
        }
    }

    async fn open(
        &self,
        entry: &Entry,
        cancel: CancellationToken,
    ) -> Result<AttachmentLease, ApplicationError> {
        self.client
            .open_attachment(
                &self.vault,
                &entry.target.session_id,
                &entry.target.attachment_id,
                &self.id,
                cancel,
            )
            .await?
            .ok_or_else(|| failure(ErrorCode::NotFound, "Attachment is no longer available"))
    }

    async fn close(&mut self) {
        if !self.client.is_closed() {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                self.client
                    .close_attachment(&self.vault, &self.id, CancellationToken::new()),
            )
            .await;
        }
        self.closed = true;
        self.leases.lock().unwrap().remove(&self.id);
    }
}

impl Drop for ServerLease {
    fn drop(&mut self) {
        if self.closed || self.client.is_closed() {
            return;
        }
        let client = self.client.clone();
        let vault = self.vault.clone();
        let id = self.id.clone();
        let leases = self.leases.clone();
        tauri::async_runtime::spawn(async move {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                client.close_attachment(&vault, &id, CancellationToken::new()),
            )
            .await;
            leases.lock().unwrap().remove(&id);
        });
    }
}

impl Entry {
    fn check(&self, request: Option<&CancellationToken>) -> Result<(), ApplicationError> {
        if self.cancel.is_cancelled()
            || self.client.is_closed()
            || request.is_some_and(CancellationToken::is_cancelled)
        {
            Err(closed())
        } else {
            Ok(())
        }
    }

    async fn acquire(
        &self,
        transfers: Arc<Semaphore>,
        request: Option<&CancellationToken>,
    ) -> Result<OwnedSemaphorePermit, ApplicationError> {
        self.check(request)?;
        tokio::select! {
            _ = self.cancel.cancelled() => Err(closed()),
            _ = async { match request { Some(request) => request.cancelled().await, None => std::future::pending().await } } => Err(closed()),
            result = transfers.acquire_owned() => result.map_err(|_| closed()),
        }
    }

    async fn local_copy(
        self: &Arc<Self>,
        root: PathBuf,
        transfers: Arc<Semaphore>,
        copies: Arc<Semaphore>,
        request: CancellationToken,
        fresh: bool,
    ) -> Result<LocalCopy, ApplicationError> {
        self.local_copy_bounded(root, transfers, copies, request, fresh, None)
            .await
    }

    async fn local_copy_bounded(
        self: &Arc<Self>,
        root: PathBuf,
        transfers: Arc<Semaphore>,
        copies: Arc<Semaphore>,
        request: CancellationToken,
        fresh: bool,
        limit: Option<u64>,
    ) -> Result<LocalCopy, ApplicationError> {
        let mut copy = tokio::select! {
            _ = self.cancel.cancelled() => return Err(closed()),
            _ = request.cancelled() => return Err(closed()),
            copy = self.copy.lock() => copy,
        };
        self.check(Some(&request))?;
        if let Some(existing) = &*copy
            && !fresh
        {
            if tokio::fs::symlink_metadata(&existing.path)
                .await
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
            {
                self.check(Some(&request))?;
                return Ok(existing.clone());
            }
        }
        let copy_permit = self.acquire(copies, Some(&request)).await?;
        let permit = self.acquire(transfers, Some(&request)).await?;
        let mut lease = ServerLease::new(self);
        let operation = async {
            let descriptor = tokio::select! {
                _ = self.cancel.cancelled() => return Err(closed()),
                result = lease.open(self, request.clone()) => result?,
            };
            let metadata = descriptor.metadata.clone();
            if limit.is_some_and(|limit| metadata.size_bytes > limit) {
                return Err(failure(
                    ErrorCode::ResourceLimit,
                    "Export image byte budget exceeded",
                ));
            }
            let stage_root = root.clone();
            let (staging, file) = tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&stage_root).map_err(io_error)?;
                let staging = tempfile::Builder::new()
                    .prefix(".loofah-download-")
                    .tempfile_in(stage_root)
                    .map_err(io_error)?;
                let file = staging.reopen().map_err(io_error)?;
                Ok::<_, ApplicationError>((staging, file))
            })
            .await
            .map_err(worker_error)??;
            let file = tokio::fs::File::from_std(file);
            let download = self.client.download_attachment_with_guard(
                descriptor,
                file,
                (staging, (permit, copy_permit), self.clone()),
                request.clone(),
            );
            let completed = if limit.is_some() {
                // Export shutdown owns the whole copy: cancellation still joins
                // SDK writes before its private directory and host admission drop.
                tokio::pin!(download);
                tokio::select! {
                    result = &mut download => result,
                    _ = self.cancel.cancelled() => { request.cancel(); download.await },
                }
            } else {
                tokio::select! {
                    _ = self.cancel.cancelled() => return Err(closed()),
                    result = download => result,
                }
            };
            let (file, (staging, _permit, _entry)) = completed?;
            let synced = file.sync_all().await.map_err(io_error);
            drop(file);
            synced?;
            self.check(Some(&request))?;
            let name = metadata.filename.clone();
            let path = tokio::task::spawn_blocking(move || publish_copy(staging, &root, &name))
                .await
                .map_err(worker_error)??;
            self.check(Some(&request))?;
            Ok(LocalCopy { path, metadata })
        }
        .await;
        lease.close().await;
        let result = operation?;
        *copy = Some(result.clone());
        Ok(result)
    }
}

fn publish_copy(
    mut staging: tempfile::NamedTempFile,
    root: &Path,
    name: &str,
) -> Result<PathBuf, ApplicationError> {
    if let Err(cause) = mark_download(staging.path()) {
        tracing::warn!(%cause, "could not apply operating-system download metadata");
    }
    let name = hypr_storage::fs::windows_safe_filename(name);
    let path = Path::new(&name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment");
    let extension = path.extension().and_then(|value| value.to_str());
    for number in 0..10_000 {
        let filename = if number == 0 {
            name.clone()
        } else {
            match extension {
                Some(ext) => format!("{stem} ({number}).{ext}"),
                None => format!("{stem} ({number})"),
            }
        };
        let path = root.join(filename);
        match staging.persist_noclobber(&path) {
            Ok(file) => {
                file.sync_all().map_err(io_error)?;
                #[cfg(unix)]
                std::fs::File::open(root)
                    .and_then(|directory| directory.sync_all())
                    .map_err(io_error)?;
                return Ok(path);
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                staging = error.file
            }
            Err(error) => return Err(io_error(error.error)),
        }
    }
    Err(failure(
        ErrorCode::ResourceLimit,
        "Too many copies with this attachment name",
    ))
}

#[cfg(windows)]
pub(crate) fn mark_download(path: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":Zone.Identifier");
    let mut file = std::fs::File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .open(stream)?;
    file.write_all(b"[ZoneTransfer]\r\nZoneId=3\r\n")?;
    file.sync_all()
}

#[cfg(target_os = "macos")]
pub(crate) fn mark_download(path: &Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let value = format!("0081;{seconds:x};Loofah;{}", uuid::Uuid::new_v4());
    let mut child = Command::new("/usr/bin/xattr")
        .args(["-w", "com.apple.quarantine", &value])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(std::io::Error::other(
                        "Download metadata could not be applied",
                    ))
                };
            }
            Ok(None) if started.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            outcome => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(outcome.err().unwrap_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "Download metadata timed out")
                }));
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn mark_download(_: &Path) -> std::io::Result<()> {
    Ok(())
}

pub(super) async fn preview_response(
    state: &RemoteState,
    owner: &str,
    request: Request<Vec<u8>>,
) -> Result<Response<Vec<u8>>, ApplicationError> {
    let path = request.uri().path();
    // Tauri's convertFileSrc encodes the complete opaque path, including its slash.
    let id = path
        .strip_prefix("/attachments/")
        .or_else(|| path.strip_prefix("/attachments%2F"))
        .or_else(|| path.strip_prefix("/attachments%2f"))
        .ok_or_else(closed)?;
    if id.len() != 36 || request.uri().query().is_some() || uuid::Uuid::parse_str(id).is_err() {
        return Err(closed());
    }
    let entry = state.attachments.get(state, owner, id)?;
    if request.method() == Method::OPTIONS {
        return Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS")
            .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "If-Match")
            .body(Vec::new())
            .unwrap());
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .header(header::ALLOW, "GET, HEAD, OPTIONS")
            .body(Vec::new())
            .unwrap());
    }
    if request.headers().contains_key(header::RANGE) {
        return Ok(Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .body(Vec::new())
            .unwrap());
    }
    let transfers = state.attachments.transfers.clone();
    let method = request.method().clone();
    let if_match = request.headers().get(header::IF_MATCH).cloned();
    let cancel = entry.cancel.child_token();
    let _on_drop = cancel.clone().drop_guard();
    tokio::spawn(async move {
        let permit = entry.acquire(transfers, Some(&cancel)).await?;
        let mut lease = ServerLease::new(&entry);
        let operation = async {
            let descriptor = lease.open(&entry, cancel.clone()).await?;
            let metadata = &descriptor.metadata;
            if metadata.size_bytes > PREVIEW_LIMIT {
                return Ok(Response::builder()
                    .status(StatusCode::PAYLOAD_TOO_LARGE)
                    .body(Vec::new())
                    .unwrap());
            }
            if !metadata.content_type.starts_with("image/") {
                return Ok(Response::builder()
                    .status(StatusCode::UNSUPPORTED_MEDIA_TYPE)
                    .body(Vec::new())
                    .unwrap());
            }
            let etag = format!("\"sha256:{}\"", metadata.sha256);
            let response = Response::builder()
                .header(header::CONTENT_TYPE, &metadata.content_type)
                .header(header::ETAG, &etag)
                .header(
                    header::CONTENT_SECURITY_POLICY,
                    "sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'",
                );
            if if_match.is_some_and(|value| value.as_bytes() != etag.as_bytes()) {
                return Ok(response
                    .status(StatusCode::PRECONDITION_FAILED)
                    .body(Vec::new())
                    .unwrap());
            }
            let size = metadata.size_bytes;
            let body = if method == Method::HEAD {
                Vec::new()
            } else {
                let (bytes, _guard) = entry
                    .client
                    .download_attachment_with_guard(
                        descriptor,
                        Vec::with_capacity(size as usize),
                        (permit, entry.clone()),
                        cancel.clone(),
                    )
                    .await?;
                bytes
            };
            entry.check(Some(&cancel))?;
            Ok(response
                .header(header::CONTENT_LENGTH, size)
                .body(body)
                .unwrap())
        }
        .await;
        lease.close().await;
        entry.check(Some(&cancel))?;
        operation
    })
    .await
    .map_err(worker_error)?
}

#[tauri::command]
#[specta::specta]
pub fn remote_attachment_open<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    resource_id: String,
    target: AttachmentTarget,
) -> Result<NativeAttachment, ApplicationError> {
    let state = app.state::<RemoteState>();
    let entry =
        state
            .attachments
            .reserve(&state, &connection_id, window.label(), &resource_id, target)?;
    Ok(NativeAttachment {
        id: entry.id.clone(),
        preview_limit_bytes: PREVIEW_LIMIT,
    })
}

#[tauri::command]
#[specta::specta]
pub fn remote_attachment_close<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    resource_id: String,
) {
    let state = app.state::<RemoteState>();
    state
        .attachments
        .close(&state, window.label(), &connection_id, &resource_id);
}

#[tauri::command]
#[specta::specta]
pub async fn remote_attachment_copy<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    resource_id: String,
    request_id: String,
    action: AttachmentCopyAction,
) -> Result<NativeAttachmentCopy, ApplicationError> {
    uuid::Uuid::parse_str(&request_id).map_err(|_| invalid("A UUID request ID is required"))?;
    let state = app.state::<RemoteState>();
    let entry = state
        .attachments
        .get(&state, window.label(), &resource_id)?;
    if entry.connection != connection_id {
        return Err(closed());
    }
    let (_, cancel, pending) = state.admit(&connection_id, window.label(), &request_id)?;
    let _on_drop = cancel.clone().drop_guard();
    let root = download_root(&app)?;
    let transfers = state.attachments.transfers.clone();
    let copies = state.attachments.copies.clone();
    tokio::spawn(async move {
        let _pending = pending;
        let copy = entry
            .local_copy(
                root,
                transfers,
                copies,
                cancel.clone(),
                matches!(action, AttachmentCopyAction::Save),
            )
            .await?;
        let info = NativeAttachmentCopy {
            copied_filename: copy.path.file_name().unwrap().to_string_lossy().into(),
            metadata: copy.metadata,
        };
        tokio::task::spawn_blocking(move || {
            entry.check(Some(&cancel))?;
            match action {
                AttachmentCopyAction::Save => Ok(()),
                AttachmentCopyAction::Open => app
                    .opener()
                    .open_path(copy.path.to_string_lossy().into_owned(), None::<&str>)
                    .map_err(|_| {
                        failure(ErrorCode::Internal, "Could not open the downloaded copy")
                    }),
                AttachmentCopyAction::Reveal => {
                    app.opener().reveal_item_in_dir(&copy.path).map_err(|_| {
                        failure(ErrorCode::Internal, "Could not show the downloaded copy")
                    })
                }
            }?;
            Ok(info)
        })
        .await
        .map_err(worker_error)?
    })
    .await
    .map_err(worker_error)?
}

pub(crate) fn download_root<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<PathBuf, ApplicationError> {
    if app.config().identifier == "io.loofah.separation-qa"
        || (cfg!(feature = "dev") && app.config().identifier == "io.loofah.remote-qa")
    {
        return std::env::var_os("LOOFAH_QA_DOWNLOADS_ROOT")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| invalid("An absolute QA downloads directory is required"));
    }
    app.path()
        .download_dir()
        .map(|path| path.join("Loofah"))
        .map_err(|_| failure(ErrorCode::Unavailable, "Downloads folder is unavailable"))
}

fn invalid(message: &str) -> ApplicationError {
    failure(ErrorCode::Validation, message)
}
fn closed() -> ApplicationError {
    failure(ErrorCode::Cancelled, "This attachment resource is closed")
}
fn busy() -> ApplicationError {
    let mut error = failure(
        ErrorCode::ResourceLimit,
        "Attachment resources are busy; close unused views and retry",
    );
    error.retryable = true;
    error
}
fn io_error(cause: std::io::Error) -> ApplicationError {
    tracing::error!(%cause, "attachment copy failed");
    failure(ErrorCode::Internal, "Could not save the attachment copy")
}
fn worker_error(cause: tokio::task::JoinError) -> ApplicationError {
    tracing::error!(%cause, "native attachment worker stopped");
    failure(ErrorCode::Internal, "Attachment operation stopped")
}

pub(crate) struct ExportAttachmentOwner {
    pub path: PathBuf,
    pub metadata: FileMetadata,
    entry: Arc<Entry>,
    entries: Arc<Mutex<HashMap<String, Arc<Entry>>>>,
}
impl ExportAttachmentOwner {
    pub fn check_current(&self, cancel: &CancellationToken) -> Result<(), ApplicationError> {
        self.entry.check(Some(cancel))
    }
}
impl Drop for ExportAttachmentOwner {
    fn drop(&mut self) {
        self.entries.lock().unwrap().remove(&self.entry.id);
        self.entry.cancel.cancel();
    }
}

pub(crate) async fn export_attachment(
    state: &RemoteState,
    connection: &str,
    owner: &str,
    target: AttachmentTarget,
    root: PathBuf,
    cancel: CancellationToken,
    limit: u64,
) -> Result<ExportAttachmentOwner, ApplicationError> {
    let entry = state.attachments.reserve(
        state,
        connection,
        owner,
        &uuid::Uuid::new_v4().to_string(),
        target,
    )?;
    let mut retained = ExportAttachmentOwner {
        path: PathBuf::new(),
        metadata: FileMetadata {
            filename: String::new(),
            content_type: String::new(),
            size_bytes: 0,
            sha256: String::new(),
        },
        entry: entry.clone(),
        entries: state.attachments.entries.clone(),
    };
    let copied = entry
        .local_copy_bounded(
            root,
            state.attachments.transfers.clone(),
            state.attachments.copies.clone(),
            cancel,
            true,
            Some(limit),
        )
        .await?;
    retained.path = copied.path;
    retained.metadata = copied.metadata;
    Ok(retained)
}

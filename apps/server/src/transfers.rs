use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use hypr_vault_access::{
    Error, Upload, User, Versions, digest,
    filesystem::{Change, VaultStorage, atomic_json, file_version, safe_path},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::{Server, auth, http::ApiError};

#[derive(Clone, Serialize, Deserialize)]
struct UploadState {
    user_id: String,
    upload: Upload,
    key: String,
    versions: Versions,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct Begin {
    session_id: String,
    filename: String,
    size: u64,
    sha256: String,
    expected: Option<String>,
    #[serde(default)]
    expected_versions: Versions,
    #[serde(default)]
    audio: bool,
}

fn filename(name: &str) -> hypr_vault_access::Result<()> {
    if name.is_empty()
        || name.len() > 255
        || name.starts_with('.')
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(Error::new("invalid_request", "invalid artifact filename"));
    }
    Ok(())
}

fn upload_path(server: &Server, vault: &str, id: &str) -> hypr_vault_access::Result<PathBuf> {
    hypr_vault_access::validate_id(vault)?;
    hypr_vault_access::validate_id(id)?;
    Ok(server
        .config
        .data_dir
        .join("service/vaults")
        .join(vault)
        .join("uploads")
        .join(id))
}

fn load(
    server: &Server,
    vault: &str,
    id: &str,
    user: &User,
) -> hypr_vault_access::Result<UploadState> {
    let bytes =
        std::fs::read(upload_path(server, vault, id)?.join("state.json")).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::new("not_found", "upload not found")
            } else {
                error.into()
            }
        })?;
    let state: UploadState = serde_json::from_slice(&bytes)?;
    if state.user_id != user.id {
        return Err(Error::new("forbidden", "upload belongs to another user"));
    }
    Ok(state)
}

#[utoipa::path(post,path="/v1/vaults/{vault}/uploads",request_body=Begin,params(("vault"=String,Path)),responses((status=200,body=Upload)))]
pub async fn begin(
    State(server): State<Arc<Server>>,
    Path(vault): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Begin>,
) -> std::result::Result<Json<Upload>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    server.authorize(&user, &vault, true, false).await?;
    hypr_vault_access::validate_id(&request.session_id)?;
    filename(&request.filename)?;
    if request.size == 0 || request.size > server.config.max_upload_bytes {
        return Err(Error::new("too_large", "artifact exceeds the configured upload limit").into());
    }
    if request.sha256.len() != 64 || !request.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::new("invalid_request", "SHA-256 checksum is required").into());
    }
    if request.audio
        && !["audio.mp3", "audio.wav", "audio.ogg", "audio.peaks.json"]
            .contains(&request.filename.as_str())
    {
        return Err(Error::new("invalid_request", "unsupported recording filename").into());
    }
    let runtime = server.runtime(&vault).await?;
    let _gate = runtime.gate.write().await;
    server.authorize(&user, &vault, true, false).await?;
    if runtime.store.session_get(&request.session_id).is_none()
        || runtime.storage.deleted(&request.session_id)?
    {
        return Err(Error::new("not_found", "session not found").into());
    }
    let key = if request.audio {
        format!("sessions/{}/{}", request.session_id, request.filename)
    } else {
        format!(
            "sessions/{}/attachments/{}",
            request.session_id, request.filename
        )
    };
    let current = file_version(&safe_path(&runtime.storage.content, &key)?)?;
    if current != request.expected {
        return Err(Error::new("conflict", "artifact changed since it was selected").into());
    }
    let mut versions = BTreeMap::from([(key.clone(), request.expected.clone())]);
    if request.audio {
        for name in ["audio.mp3", "audio.wav", "audio.ogg"] {
            let key = format!("sessions/{}/{name}", request.session_id);
            let current = file_version(&safe_path(&runtime.storage.content, &key)?)?;
            if request
                .expected_versions
                .get(&key)
                .is_some_and(|expected| expected != &current)
            {
                return Err(Error::new("conflict", "recording changed since capture began").into());
            }
            versions.insert(key, current);
        }
    }
    let upload = Upload {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: request.session_id,
        filename: request.filename,
        size: request.size,
        offset: 0,
        sha256: request.sha256.to_ascii_lowercase(),
        expected: request.expected,
        finished: false,
    };
    let path = upload_path(&server, &vault, &upload.id)?;
    hypr_vault_access::filesystem::create_dir_all(&path)?;
    hypr_vault_access::filesystem::atomic_write(&path.join("data"), &[])?;
    atomic_json(
        &path.join("state.json"),
        &UploadState {
            user_id: user.id,
            upload: upload.clone(),
            key,
            versions,
        },
    )?;
    Ok(Json(upload))
}

#[utoipa::path(get,path="/v1/vaults/{vault}/uploads/{upload}",params(("vault"=String,Path),("upload"=String,Path)),responses((status=200,body=Upload)))]
pub async fn status(
    State(server): State<Arc<Server>>,
    Path((vault, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<Json<Upload>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    server.authorize(&user, &vault, false, false).await?;
    let runtime = server.runtime(&vault).await?;
    let _gate = runtime.gate.read().await;
    server.authorize(&user, &vault, false, false).await?;
    Ok(Json(load(&server, &vault, &id, &user)?.upload))
}

#[utoipa::path(put,path="/v1/vaults/{vault}/uploads/{upload}",request_body(content=String,content_type="application/octet-stream"),params(("vault"=String,Path),("upload"=String,Path),("Upload-Offset"=u64,Header)),responses((status=200,body=Upload)))]
pub async fn chunk(
    State(server): State<Arc<Server>>,
    Path((vault, id)): Path<(String, String)>,
    headers: HeaderMap,
    bytes: Bytes,
) -> std::result::Result<Json<Upload>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    server.authorize(&user, &vault, true, false).await?;
    let offset: u64 = headers
        .get("upload-offset")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| Error::new("invalid_request", "Upload-Offset is required"))?;
    if bytes.is_empty() || bytes.len() > 8 * 1024 * 1024 {
        return Err(Error::new("too_large", "chunks must contain 1 byte to 8 MiB").into());
    }
    let runtime = server.runtime(&vault).await?;
    load(&server, &vault, &id, &user)?;
    let upload_lock = runtime
        .transfer_locks
        .lock()
        .await
        .entry(id.clone())
        .or_default()
        .clone();
    let _upload_gate = upload_lock.lock().await;
    let _gate = runtime.gate.read().await;
    server.authorize(&user, &vault, true, false).await?;
    let mut state = load(&server, &vault, &id, &user)?;
    if state.upload.finished
        || offset > state.upload.offset
        || offset.saturating_add(bytes.len() as u64) > state.upload.size
    {
        return Err(Error::new("conflict", "upload offset does not match").into());
    }
    let path = upload_path(&server, &vault, &id)?;
    let data_path = path.join("data");
    let next = tokio::task::spawn_blocking(move || -> hypr_vault_access::Result<UploadState> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(data_path)?;
        if offset < state.upload.offset {
            if offset + bytes.len() as u64 > state.upload.offset {
                return Err(Error::new("conflict", "chunk overlaps a committed chunk"));
            }
            file.seek(SeekFrom::Start(offset))?;
            let mut previous = vec![0; bytes.len()];
            file.read_exact(&mut previous)?;
            if previous != bytes {
                return Err(Error::new(
                    "conflict",
                    "retried chunk has different content",
                ));
            }
            return Ok(state);
        }
        file.set_len(state.upload.offset)?;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        state.upload.offset += bytes.len() as u64;
        atomic_json(&path.join("state.json"), &state)?;
        Ok(state)
    })
    .await
    .map_err(|error| Error::new("storage", error.to_string()))??;
    Ok(Json(next.upload))
}

#[utoipa::path(post,path="/v1/vaults/{vault}/uploads/{upload}/finish",params(("vault"=String,Path),("upload"=String,Path)),responses((status=200,body=Upload)))]
pub async fn finish(
    State(server): State<Arc<Server>>,
    Path((vault, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<Json<Upload>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    server.authorize(&user, &vault, true, false).await?;
    let runtime = server.runtime(&vault).await?;
    load(&server, &vault, &id, &user)?;
    let upload_lock = runtime
        .transfer_locks
        .lock()
        .await
        .entry(id.clone())
        .or_default()
        .clone();
    let _upload_gate = upload_lock.lock().await;
    let mut state = load(&server, &vault, &id, &user)?;
    if state.upload.finished {
        let _ = std::fs::remove_file(upload_path(&server, &vault, &id)?.join("data"));
        return Ok(Json(state.upload));
    }
    let data = upload_path(&server, &vault, &id)?.join("data");
    let checksum_path = data.clone();
    let actual = tokio::task::spawn_blocking(move || file_version(&checksum_path))
        .await
        .map_err(|error| Error::new("storage", error.to_string()))??;
    if actual.as_deref() != Some(state.upload.sha256.as_str()) {
        return Err(Error::new("invalid_data", "upload checksum does not match").into());
    }
    let _gate = runtime.gate.write().await;
    server.authorize(&user, &vault, true, false).await?;
    crate::content::recover(&runtime).await?;
    if runtime.storage.deleted(&state.upload.session_id)?
        || runtime
            .store
            .session_get(&state.upload.session_id)
            .is_none()
    {
        return Err(Error::new("conflict", "session was deleted").into());
    }
    if state.upload.offset != state.upload.size {
        return Err(Error::new("conflict", "upload is incomplete").into());
    }
    let path = upload_path(&server, &vault, &id)?;
    let storage = runtime.storage.clone();
    let expected = state.versions.clone();
    let key = state.key.clone();
    let checksum = state.upload.sha256.clone();
    let session_id = state.upload.session_id.clone();
    let audio = !key.contains("/attachments/") && !key.ends_with("audio.peaks.json");
    let operation = format!("{}_upload_{id}", user.id);
    runtime
        .failed
        .store(true, std::sync::atomic::Ordering::Release);
    tokio::task::spawn_blocking(move || -> hypr_vault_access::Result<()> {
        let mut changes = vec![Change::Publish(key.clone(), data, checksum.clone())];
        if audio {
            for name in ["audio.mp3", "audio.wav", "audio.ogg", "audio.peaks.json"] {
                let other = format!("sessions/{session_id}/{name}");
                if other != key {
                    changes.push(Change::Remove(other));
                }
            }
        }
        storage.commit(&operation, &checksum, &expected, changes, Value::Null)?;
        Ok(())
    })
    .await
    .map_err(|error| Error::new("storage", error.to_string()))??;
    state.upload.finished = true;
    atomic_json(&path.join("state.json"), &state)?;
    std::fs::remove_file(path.join("data")).map_err(Error::from)?;
    runtime
        .store
        .refresh_session(&state.upload.session_id)
        .await
        .map_err(crate::content::store_error)?;
    runtime
        .failed
        .store(false, std::sync::atomic::Ordering::Release);
    server.notify(
        Some(vault),
        "artifacts",
        vec![state.upload.session_id.clone()],
    );
    Ok(Json(state.upload))
}

#[utoipa::path(delete,path="/v1/vaults/{vault}/uploads/{upload}",params(("vault"=String,Path),("upload"=String,Path)),responses((status=200,body=Value)))]
pub async fn cancel(
    State(server): State<Arc<Server>>,
    Path((vault, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    server.authorize(&user, &vault, true, false).await?;
    let runtime = server.runtime(&vault).await?;
    load(&server, &vault, &id, &user)?;
    let upload_lock = runtime
        .transfer_locks
        .lock()
        .await
        .entry(id.clone())
        .or_default()
        .clone();
    let _upload_gate = upload_lock.lock().await;
    let _gate = runtime.gate.write().await;
    load(&server, &vault, &id, &user)?;
    std::fs::remove_dir_all(upload_path(&server, &vault, &id)?).map_err(Error::from)?;
    Ok(Json(Value::Null))
}

#[utoipa::path(get,path="/v1/vaults/{vault}/sessions/{session}/artifacts",params(("vault"=String,Path),("session"=String,Path)),responses((status=200,body=Value)))]
pub async fn list(
    State(server): State<Arc<Server>>,
    Path((vault, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    server.authorize(&user, &vault, false, false).await?;
    hypr_vault_access::validate_id(&session)?;
    let runtime = server.runtime(&vault).await?;
    let _gate = runtime.gate.read().await;
    server.authorize(&user, &vault, false, false).await?;
    if runtime.failed.load(std::sync::atomic::Ordering::Acquire) {
        return Err(Error::new("commit_pending", "vault recovery is required").into());
    }
    let mut keys = ["audio.mp3", "audio.wav", "audio.ogg", "audio.peaks.json"]
        .map(|name| format!("sessions/{session}/{name}"))
        .to_vec();
    let prefix = format!("sessions/{session}/attachments");
    let path = safe_path(&runtime.storage.content, &prefix)?;
    if path.is_dir() {
        for entry in std::fs::read_dir(path).map_err(Error::from)? {
            let entry = entry.map_err(Error::from)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') && entry.file_type().map_err(Error::from)?.is_file() {
                keys.push(format!("{prefix}/{name}"));
            }
        }
    }
    let root = runtime.storage.content.clone();
    let artifacts_runtime = runtime.clone();
    let artifacts = tokio::task::spawn_blocking(move || -> hypr_vault_access::Result<Vec<Value>> {
        let mut artifacts = Vec::new();
        for key in keys {
            let path = safe_path(&root, &key)?;
            if !path.is_file() { continue; }
            use std::os::unix::fs::MetadataExt;
            let metadata=path.metadata()?;
            let modified=metadata.modified()?;
            let cached=artifacts_runtime.artifact_versions.lock().unwrap().get(&key).filter(|(size,inode,at,_)|*size==metadata.len() && *inode==metadata.ino() && *at==modified).map(|(_,_,_,version)|version.clone());
            let version=match cached {Some(version)=>version,None=>{let version=file_version(&path)?.ok_or_else(||Error::new("not_found","artifact disappeared"))?;artifacts_runtime.artifact_versions.lock().unwrap().insert(key.clone(),(metadata.len(),metadata.ino(),modified,version.clone()));version}};
            artifacts.push(json!({"key":key,"filename":path.file_name().unwrap().to_string_lossy(),"size":metadata.len(),"version":version,"content_type":content_type(&key)}));
        }
        Ok(artifacts)
    }).await.map_err(|error| Error::new("storage", error.to_string()))??;
    Ok(Json(json!(artifacts)))
}

#[derive(Serialize, Deserialize)]
struct Ticket {
    user_id: String,
    vault_id: String,
    key: String,
    expires: i64,
    access_hash: String,
    inode: u64,
    size: u64,
    modified: std::time::SystemTime,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct TicketRequest {
    key: String,
}

#[utoipa::path(post,path="/v1/vaults/{vault}/sessions/{session}/media",request_body=TicketRequest,params(("vault"=String,Path),("session"=String,Path)),responses((status=200,body=Value)))]
pub async fn ticket(
    State(server): State<Arc<Server>>,
    Path((vault, session)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<TicketRequest>,
) -> std::result::Result<Json<Value>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    server.authorize(&user, &vault, false, false).await?;
    hypr_vault_access::validate_id(&session)?;
    let prefix = format!("sessions/{session}/");
    let suffix = request.key.strip_prefix(&prefix).ok_or_else(|| {
        Error::new(
            "invalid_request",
            "artifact must belong to the selected session",
        )
    })?;
    if let Some(name) = suffix.strip_prefix("attachments/") {
        filename(name)?;
    } else if !["audio.mp3", "audio.wav", "audio.ogg", "audio.peaks.json"].contains(&suffix) {
        return Err(Error::new("invalid_request", "unsupported artifact").into());
    }
    let runtime = server.runtime(&vault).await?;
    let artifact = safe_path(&runtime.storage.content, &request.key)?;
    if !artifact.is_file() {
        return Err(Error::new("not_found", "artifact not found").into());
    }
    use std::os::unix::fs::MetadataExt;
    let metadata = artifact.metadata().map_err(Error::from)?;
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let access_hash = digest(
        headers["authorization"]
            .to_str()
            .unwrap()
            .trim_start_matches("Bearer ")
            .as_bytes(),
    );
    atomic_json(
        &server
            .config
            .data_dir
            .join("service/media")
            .join(digest(token.as_bytes())),
        &Ticket {
            user_id: user.id,
            vault_id: vault,
            key: request.key,
            expires: chrono::Utc::now().timestamp() + 300,
            access_hash,
            inode: metadata.ino(),
            size: metadata.len(),
            modified: metadata.modified().map_err(Error::from)?,
        },
    )?;
    Ok(Json(
        json!({"url":format!("{}/v1/media/{token}",server.config.public_url.trim_end_matches('/')),"expires_in":300}),
    ))
}

#[utoipa::path(get,path="/v1/media/{ticket}",params(("ticket"=String,Path),("Range"=Option<String>,Header)),responses((status=200)))]
pub async fn media(
    State(server): State<Arc<Server>>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> std::result::Result<Response, ApiError> {
    if token.len() != 64 {
        return Err(Error::new("unauthenticated", "invalid media ticket").into());
    }
    let path = server
        .config
        .data_dir
        .join("service/media")
        .join(digest(token.as_bytes()));
    let ticket: Ticket = serde_json::from_slice(
        &std::fs::read(path).map_err(|_| Error::new("unauthenticated", "media ticket expired"))?,
    )
    .map_err(Error::from)?;
    let now = chrono::Utc::now().timestamp();
    let registry = server.registry.read().await;
    if ticket.expires <= now
        || !registry.sessions.values().any(|session| {
            session.access_hash == ticket.access_hash
                && session.user_id == ticket.user_id
                && session.access_expires > now
                && session.expires > now
        })
    {
        return Err(Error::new("unauthenticated", "media ticket expired").into());
    }
    let user = registry
        .users
        .get(&ticket.user_id)
        .cloned()
        .ok_or_else(|| Error::new("unauthenticated", "user is unavailable"))?;
    drop(registry);
    server
        .authorize(&user, &ticket.vault_id, false, false)
        .await?;
    let runtime = server.runtime(&ticket.vault_id).await?;
    let _gate = runtime.gate.read().await;
    server
        .authorize(&user, &ticket.vault_id, false, false)
        .await?;
    if runtime.failed.load(std::sync::atomic::Ordering::Acquire) {
        return Err(Error::new("commit_pending", "vault recovery is required").into());
    }
    let mut file = tokio::fs::File::open(safe_path(&runtime.storage.content, &ticket.key)?)
        .await
        .map_err(|_| Error::new("not_found", "artifact not found"))?;
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().await.map_err(Error::from)?;
    if metadata.ino() != ticket.inode
        || metadata.len() != ticket.size
        || metadata.modified().map_err(Error::from)? != ticket.modified
    {
        return Err(Error::new("conflict", "artifact changed; request a new media link").into());
    }
    let size = metadata.len();
    let (start, end, partial) = range(
        headers
            .get(header::RANGE)
            .and_then(|range| range.to_str().ok()),
        size,
    )?;
    tokio::io::AsyncSeekExt::seek(&mut file, SeekFrom::Start(start))
        .await
        .map_err(Error::from)?;
    let length = if size == 0 { 0 } else { end - start + 1 };
    let stream = tokio_util::io::ReaderStream::new(file.take(length));
    let mut response = (
        if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        },
        Body::from_stream(stream),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, length.to_string().parse().unwrap());
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        content_type(&ticket.key).parse().unwrap(),
    );
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".parse().unwrap());
    response.headers_mut().insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        "Content-Length, Content-Range, Accept-Ranges"
            .parse()
            .unwrap(),
    );
    if partial {
        response.headers_mut().insert(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{size}").parse().unwrap(),
        );
    }
    Ok(response)
}

pub fn range(value: Option<&str>, size: u64) -> hypr_vault_access::Result<(u64, u64, bool)> {
    let Some(value) = value else {
        return Ok((0, size.saturating_sub(1), false));
    };
    let value = value
        .strip_prefix("bytes=")
        .ok_or_else(|| Error::new("range", "unsupported range"))?;
    let (start, end) = value
        .split_once('-')
        .filter(|_| !value.contains(','))
        .ok_or_else(|| Error::new("range", "invalid range"))?;
    let parse = |value: &str| {
        value
            .parse::<u64>()
            .map_err(|_| Error::new("range", "invalid byte position"))
    };
    let (start, end) = if start.is_empty() {
        let suffix = parse(end)?;
        if suffix == 0 {
            return Err(Error::new("range", "empty suffix"));
        }
        (size.saturating_sub(suffix), size.saturating_sub(1))
    } else {
        (
            parse(start)?,
            if end.is_empty() {
                size.saturating_sub(1)
            } else {
                parse(end)?.min(size.saturating_sub(1))
            },
        )
    };
    if size == 0 || start >= size || end < start {
        return Err(Error::new("range", "range is outside the artifact"));
    }
    Ok((start, end, true))
}

fn content_type(key: &str) -> &'static str {
    match key
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

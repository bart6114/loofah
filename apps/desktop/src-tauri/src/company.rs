use std::sync::Arc;

use base64::Engine;
use hypr_vault_access::{
    Credentials, Error, ServerInfo, Versions,
    connection::{ConnectionState, CredentialStore, decode, validate_server_url},
};
use serde_json::{Value, json};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_settings::SettingsPluginExt;

pub struct Keychain(pub String);
impl CredentialStore for Keychain {
    fn load(&self, key: &str) -> hypr_vault_access::Result<Option<Credentials>> {
        let entry = keyring::Entry::new(&format!("{}.company-vault", self.0), key)
            .map_err(|error| Error::new("keychain", error.to_string()))?;
        match entry.get_password() {
            Ok(value) => Ok(Some(serde_json::from_str(&value)?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(Error::new("keychain", error.to_string())),
        }
    }
    fn save(&self, key: &str, credentials: &Credentials) -> hypr_vault_access::Result<()> {
        keyring::Entry::new(&format!("{}.company-vault", self.0), key)
            .map_err(|error| Error::new("keychain", error.to_string()))?
            .set_password(&serde_json::to_string(credentials)?)
            .map_err(|error| Error::new("keychain", error.to_string()))
    }
    fn remove(&self, key: &str) -> hypr_vault_access::Result<()> {
        let entry = keyring::Entry::new(&format!("{}.company-vault", self.0), key)
            .map_err(|error| Error::new("keychain", error.to_string()))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(Error::new("keychain", error.to_string())),
        }
    }
}

pub async fn invoke<R: tauri::Runtime, T: serde::de::DeserializeOwned>(
    app: &tauri::AppHandle<R>,
    operation: &str,
    mut args: Value,
) -> Option<Result<T, String>> {
    let state = app.try_state::<Arc<ConnectionState>>()?;
    if !state.active() {
        return None;
    }
    if matches!(
        operation,
        "session_update_summary" | "session_update_enhanced_doc" | "session_save_enhanced_doc"
    ) {
        args["auto_apply_high_confidence_tags"] =
            json!(app.settings().config().auto_apply_high_confidence_tags);
    }
    let query = hypr_vault_write::access::is_query(operation);
    let result = state.call(operation, args.clone(), query).await;
    if result.is_ok() && !query {
        use tauri_specta::Event;
        let entities: &[crate::session_store::IndexEntity] = if operation.contains("transcript") {
            &[crate::session_store::IndexEntity::Transcripts]
        } else if operation.contains("summary") || operation.contains("enhanced") {
            &[
                crate::session_store::IndexEntity::Docs,
                crate::session_store::IndexEntity::Tasks,
            ]
        } else if operation.contains("tasks") {
            &[crate::session_store::IndexEntity::Tasks]
        } else {
            &[
                crate::session_store::IndexEntity::Sessions,
                crate::session_store::IndexEntity::SessionHeaders,
            ]
        };
        for entity in entities {
            let _ = crate::session_store::IndexChanged {
                entity: *entity,
                ids: args["session_id"]
                    .as_str()
                    .map(|id| vec![id.into()])
                    .unwrap_or_default(),
            }
            .emit(app);
        }
    }
    if result
        .as_ref()
        .is_err_and(|error| matches!(error.code.as_str(), "forbidden" | "unauthenticated"))
    {
        let _ = app.emit("company-access-revoked", ());
    }
    let _ = app.emit("company-status-changed", ());
    Some(
        result
            .and_then(|value| serde_json::from_value(value).map_err(Into::into))
            .map_err(|error| error.to_string()),
    )
}

pub fn changed<R: tauri::Runtime>(app: &tauri::AppHandle<R>, id: Option<&str>) {
    use tauri_specta::Event;
    for entity in [
        crate::session_store::IndexEntity::Sessions,
        crate::session_store::IndexEntity::SessionHeaders,
        crate::session_store::IndexEntity::Docs,
        crate::session_store::IndexEntity::Transcripts,
        crate::session_store::IndexEntity::Tasks,
        crate::session_store::IndexEntity::People,
        crate::session_store::IndexEntity::Tags,
        crate::session_store::IndexEntity::Artifacts,
    ] {
        let _ = crate::session_store::IndexChanged {
            entity,
            ids: id.map(|id| vec![id.into()]).unwrap_or_default(),
        }
        .emit(app);
    }
}

#[tauri::command]
#[specta::specta]
pub async fn company_action<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    operation: String,
    args: Value,
) -> Result<Value, String> {
    action(&app, &operation, args)
        .await
        .map_err(|error| error.to_string())
}

async fn action<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    operation: &str,
    args: Value,
) -> hypr_vault_access::Result<Value> {
    let state = app.state::<Arc<ConnectionState>>().inner().clone();
    let text = |key: &str| {
        args[key]
            .as_str()
            .ok_or_else(|| Error::new("invalid_request", format!("missing {key}")))
    };
    match operation {
        "begin_processing" => Ok(json!(state.begin_processing().await)),
        "end_processing" => {
            state.end_processing(text("id")?);
            Ok(Value::Null)
        }
        "status" => {
            let mut status = state.status();
            status["pending"] = serde_json::to_value(state.pending()?)?;
            status["transfers"] = serde_json::to_value(
                state
                    .transfers()?
                    .into_iter()
                    .map(|(_, job)| job)
                    .collect::<Vec<_>>(),
            )?;
            Ok(status)
        }
        "login" => {
            if state.recording.load(std::sync::atomic::Ordering::Acquire) {
                return Err(Error::new("conflict", "stop recording before connecting"));
            }
            let base = validate_server_url(text("url")?)?;
            let server: ServerInfo = decode(
                state
                    .http
                    .get(format!("{base}/v1/server"))
                    .send()
                    .await
                    .map_err(|error| Error::new("unavailable", error.to_string()))?,
            )
            .await?;
            if server.api_version != 1 {
                return Err(Error::new(
                    "invalid_data",
                    "unsupported company server version",
                ));
            }
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let verifier = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            use sha2::{Digest, Sha256};
            let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(Sha256::digest(verifier.as_bytes()));
            let native_state = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            let callback = format!(
                "http://127.0.0.1:{}/{}",
                listener.local_addr()?.port(),
                uuid::Uuid::new_v4()
            );
            let start: Value = decode(
                state
                    .http
                    .post(format!("{base}/v1/auth/start"))
                    .json(&json!({"callback":callback,"challenge":challenge,"state":native_state}))
                    .send()
                    .await
                    .map_err(|error| Error::new("unavailable", error.to_string()))?,
            )
            .await?;
            let url = start["authorization_url"]
                .as_str()
                .ok_or_else(|| Error::new("invalid_data", "invalid sign-in URL"))?;
            app.opener()
                .open_url(url, None::<&str>)
                .map_err(|error| Error::new("unavailable", error.to_string()))?;
            let code = tokio::time::timeout(
                std::time::Duration::from_secs(300),
                hypr_vault_access::login::receive_callback(&listener, &callback, &native_state),
            )
            .await
            .map_err(|_| Error::new("unauthenticated", "company sign-in timed out"))??;
            let credentials: Credentials = decode(
                state
                    .http
                    .post(format!("{base}/v1/auth/exchange"))
                    .json(&json!({"code":code,"verifier":verifier}))
                    .send()
                    .await
                    .map_err(|error| Error::new("unavailable", error.to_string()))?,
            )
            .await?;
            state.connect(base, server, credentials).await?;
            let _ = app.emit("company-authenticated", ());
            let _ = app.emit("company-status-changed", ());
            Ok(state.status())
        }
        "request" => {
            state
                .request(text("method")?, text("path")?, args.get("body").cloned())
                .await
        }
        "select" => {
            require_main_only(app)?;
            state
                .select(serde_json::from_value(
                    args.get("vault_id").cloned().unwrap_or(Value::Null),
                )?)
                .await?;
            let base = if state.active() {
                state.root.join("preferences")
            } else {
                app.settings()
                    .resolve_startup_vault_base()
                    .map_err(|error| Error::new("storage", error.to_string()))?
            };
            let config = app.state::<tauri_plugin_settings::ConfigState>();
            config
                .switch_base(&base)
                .await
                .map_err(|error| Error::new("storage", error.to_string()))?;
            let _ = app.emit(
                tauri_plugin_settings::CONFIG_CHANGED_EVENT,
                config.snapshot(),
            );
            let _ = app.emit("company-vault-changed", ());
            Ok(state.status())
        }
        "logout" => {
            require_main_only(app)?;
            state.logout().await?;
            let base = app
                .settings()
                .resolve_startup_vault_base()
                .map_err(|error| Error::new("storage", error.to_string()))?;
            app.state::<tauri_plugin_settings::ConfigState>()
                .switch_base(&base)
                .await
                .map_err(|error| Error::new("storage", error.to_string()))?;
            let _ = app.emit("company-vault-changed", ());
            Ok(state.status())
        }
        "retry" => {
            state.retry().await?;
            state.retry_uploads().await?;
            changed(app, None);
            Ok(state.status())
        }
        "resolve" => {
            let expected: Versions =
                serde_json::from_value(args.get("expected").cloned().unwrap_or(json!({})))?;
            state
                .resolve(text("id")?, text("action")?, expected)
                .await?;
            changed(app, None);
            let _ = app.emit("company-status-changed", ());
            Ok(state.status())
        }
        "preview" => {
            let profile = state
                .profile()
                .ok_or_else(|| Error::new("unauthenticated", "connect to a company server"))?;
            let vault = profile
                .vault_id
                .ok_or_else(|| Error::new("invalid_request", "select a vault"))?;
            let pending = state
                .pending()?
                .into_iter()
                .find(|draft| draft.call.mutation_id.as_deref() == args["id"].as_str())
                .ok_or_else(|| Error::new("not_found", "pending draft not found"))?;
            let (operation, query) = if pending.call.operation.contains("summary") {
                (
                    "session_summary_get",
                    json!({"session_id":pending.call.args["session_id"]}),
                )
            } else if pending.call.operation.contains("enhanced") {
                (
                    "enhanced_doc_get",
                    json!({"doc_id":pending.call.args["doc_id"]}),
                )
            } else {
                (
                    "session_get",
                    json!({"session_id":pending.call.args["session_id"]}),
                )
            };
            state
                .request(
                    "POST",
                    &format!("/v1/vaults/{vault}/invoke"),
                    Some(json!(hypr_vault_access::Call::query(operation, query))),
                )
                .await
        }
        "save_separate" => {
            let id = state
                .save_separate(text("id")?, &chrono::Utc::now().to_rfc3339())
                .await?;
            changed(app, None);
            Ok(json!({"session_id":id}))
        }
        "export_draft" | "export_transfer" => {
            use tauri_plugin_dialog::DialogExt;
            let (filename, bytes, source) = if operation == "export_draft" {
                let pending = state
                    .pending()?
                    .into_iter()
                    .find(|draft| {
                        draft.call.mutation_id.as_deref() == Some(text("id").unwrap_or(""))
                    })
                    .ok_or_else(|| Error::new("not_found", "pending draft not found"))?;
                (
                    "recovered-draft.json".to_string(),
                    Some(serde_json::to_vec_pretty(&pending)?),
                    None,
                )
            } else {
                let (_, job) = state
                    .transfers()?
                    .into_iter()
                    .find(|(_, job)| job.id == text("id").unwrap_or(""))
                    .ok_or_else(|| Error::new("not_found", "pending upload not found"))?;
                (job.filename, None, Some(job.source))
            };
            let dialog = app.dialog().file().set_file_name(&filename);
            let path = tokio::task::spawn_blocking(move || dialog.blocking_save_file())
                .await
                .map_err(|error| Error::new("storage", error.to_string()))?;
            if let Some(path) = path {
                let path = path
                    .into_path()
                    .map_err(|error| Error::new("storage", error.to_string()))?;
                if let Some(bytes) = bytes {
                    hypr_vault_access::filesystem::atomic_write(&path, &bytes)?;
                } else if let Some(source) = source {
                    tokio::fs::copy(source, &path).await?;
                    tokio::fs::File::open(&path).await?.sync_all().await?;
                }
            }
            Ok(Value::Null)
        }
        "discard_transfer" => {
            state.discard_transfer(text("id")?).await?;
            Ok(Value::Null)
        }
        _ => Err(Error::new("invalid_request", "unsupported company action")),
    }
}

fn require_main_only<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> hypr_vault_access::Result<()> {
    if app
        .webview_windows()
        .keys()
        .any(|label| label.starts_with("note-"))
    {
        return Err(Error::new(
            "conflict",
            "close standalone note windows before switching vaults or signing out",
        ));
    }
    Ok(())
}

async fn event_received<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &ConnectionState,
    profile: &hypr_vault_access::connection::Profile,
    rejected_access: &str,
    message: &str,
) -> bool {
    let destination = profile.destination().ok();
    if destination.is_none()
        || state
            .profile()
            .and_then(|profile| profile.destination().ok())
            != destination
    {
        return true;
    }
    let kind = message
        .lines()
        .find_map(|line| line.strip_prefix("event:").map(str::trim));
    if kind == Some("unauthenticated") {
        let result = state.refresh_rejected_access(rejected_access).await;
        if state
            .profile()
            .and_then(|profile| profile.destination().ok())
            != destination
        {
            return true;
        }
        match result {
            Ok(_) => state.set_error(None),
            Err(error) => {
                if matches!(error.code.as_str(), "unauthenticated" | "forbidden") {
                    let _ = app.emit("company-access-revoked", ());
                }
                state.set_error(Some(error));
            }
        }
        let _ = app.emit("company-status-changed", ());
        return true;
    }
    if kind == Some("revoked") {
        let _ = app.emit("company-access-revoked", ());
        return true;
    }
    if kind == Some("reset") {
        changed(app, None);
        let _ = app.emit("company-status-changed", ());
    }
    if kind == Some("change") {
        use tauri_specta::Event;
        if let Some(data) = message.lines().find_map(|line| line.strip_prefix("data: ")) {
            if let Ok(event) = serde_json::from_str::<hypr_vault_access::ChangeEvent>(data) {
                if matches!(event.entity.as_str(), "members" | "vaults") {
                    let _ = app.emit("company-vault-list-changed", ());
                }
                if event.vault_id == profile.vault_id {
                    if let Ok(entity) = serde_json::from_value(serde_json::json!(event.entity)) {
                        let _ = crate::session_store::IndexChanged {
                            entity,
                            ids: event.ids,
                        }
                        .emit(app);
                    }
                }
            }
        }
        let _ = app.emit("company-status-changed", ());
    }
    false
}

async fn receive_events<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &ConnectionState,
    profile: &hypr_vault_access::connection::Profile,
    credentials: &Credentials,
    mut response: reqwest::Response,
) {
    if response.status().is_success() {
        let mut buffer = String::new();
        'stream: loop {
            if state
                .profile()
                .and_then(|profile| profile.destination().ok())
                != profile.destination().ok()
            {
                break;
            }
            let chunk =
                tokio::time::timeout(std::time::Duration::from_secs(20), response.chunk()).await;
            let Ok(Ok(Some(chunk))) = chunk else {
                break;
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(position) = buffer.find("\n\n") {
                let message = buffer[..position].to_string();
                buffer.drain(..position + 2);
                if event_received(app, state, profile, &credentials.access_token, &message).await {
                    break 'stream;
                }
            }
            if buffer.len() > 64 * 1024 {
                break;
            }
        }
    } else if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        event_received(
            app,
            state,
            profile,
            &credentials.access_token,
            "event: unauthenticated",
        )
        .await;
    } else if response.status() == reqwest::StatusCode::FORBIDDEN {
        event_received(
            app,
            state,
            profile,
            &credentials.access_token,
            "event: revoked",
        )
        .await;
    }
}

pub fn spawn(app: tauri::AppHandle) {
    let worker = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let state = worker.state::<Arc<ConnectionState>>().inner().clone();
            if state.active() {
                let _ = state.retry().await;
                if let Err(error) = state.retry_uploads().await {
                    tracing::warn!(%error,"company uploads remain queued");
                }
                let _ = worker.emit("company-status-changed", ());
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app.state::<Arc<ConnectionState>>().inner().clone();
            if !state.active() {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
            let (profile, credentials) = match state.authenticated().await {
                Ok(value) => value,
                Err(error) => {
                    state.set_error(Some(error.clone()));
                    if matches!(error.code.as_str(), "unauthenticated" | "forbidden") {
                        let _ = app.emit("company-access-revoked", ());
                    }
                    let _ = app.emit("company-status-changed", json!({"error":error.to_string()}));
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };
            let _ = state.retry().await;
            let request = state
                .http
                .get(format!("{}/v1/events", profile.base_url))
                .bearer_auth(&credentials.access_token)
                .send()
                .await;
            if let Ok(response) = request {
                receive_events(&app, &state, &profile, &credentials, response).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
}

#[cfg(test)]
mod tests;

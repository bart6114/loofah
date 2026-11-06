pub(crate) mod attachments;
pub(crate) mod media;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hypr_loofah_client::{
    CredentialStore, OsCredentials, RemoteClient, RemoteEvent, RemoteOperation, RemoteReply,
    Subscription, failure,
};
use hypr_loofah_protocol::{ApplicationError, Capabilities, ErrorCode, Me};
use tauri::{Manager, ipc::Channel};
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;

pub async fn media_response<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    label: &str,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let forbidden = || {
        tauri::http::Response::builder()
            .status(403)
            .body(Vec::new())
            .unwrap()
    };
    let Some(view) = app.get_webview_window(label) else {
        return forbidden();
    };
    let Ok(url) = view.url() else {
        return forbidden();
    };
    let origin = if url.scheme() == "tauri" && url.host_str() == Some("localhost") {
        "tauri://localhost".into()
    } else if matches!(url.scheme(), "http" | "https") && url.host_str() == Some("tauri.localhost")
    {
        url.origin().ascii_serialization()
    } else if cfg!(debug_assertions)
        && app
            .config()
            .build
            .dev_url
            .as_ref()
            .is_some_and(|expected| expected.origin() == url.origin())
    {
        url.origin().ascii_serialization()
    } else {
        return forbidden();
    };
    let owner = view.label().to_owned();
    drop(view);
    media::response(&app.state::<RemoteState>(), &owner, &origin, request).await
}

#[derive(Default)]
pub struct RemoteState {
    registry: Arc<Mutex<HashMap<String, Connection>>>,
    windows: Mutex<HashMap<String, CancellationToken>>,
    media: media::MediaRegistry,
    attachments: attachments::AttachmentRegistry,
}
type WorkId = (String, String);
struct Connection {
    client: Arc<RemoteClient>,
    leases: HashMap<String, usize>,
    authenticated: bool,
    login: Option<(WorkId, CancellationToken)>,
    requests: HashMap<WorkId, CancellationToken>,
    cancelled: HashMap<WorkId, Instant>,
    cancelled_subscriptions: HashMap<WorkId, Instant>,
    subscriptions: HashMap<WorkId, Subscription>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.client.disconnect();
        if let Some((_, login)) = &self.login {
            login.cancel();
        }
        for cancel in self.requests.values() {
            cancel.cancel();
        }
    }
}
impl RemoteState {
    pub fn shutdown(&self) {
        for (_, token) in self.windows.lock().unwrap().drain() {
            token.cancel();
        }
        self.registry.lock().unwrap().clear();
        self.media.clear();
        self.attachments.clear();
    }
    fn window_lifetime(&self, owner: &str) -> CancellationToken {
        self.windows
            .lock()
            .unwrap()
            .entry(owner.into())
            .or_default()
            .clone()
    }
    pub fn release_window(&self, owner: &str) {
        let mut windows = self.windows.lock().unwrap();
        if let Some(token) = windows.remove(owner) {
            token.cancel();
        }
        self.registry.lock().unwrap().retain(|_, connection| {
            connection.release(owner, true);
            !connection.leases.is_empty()
        });
        self.media.prune();
        self.attachments.prune(self);
    }
    fn register(
        &self,
        client: Arc<RemoteClient>,
        owner: &str,
        lifetime: &CancellationToken,
    ) -> Result<RemoteConnectionInfo, ApplicationError> {
        let _windows = self.windows.lock().unwrap();
        if lifetime.is_cancelled() {
            return Err(missing());
        }
        let mut registry = self.registry.lock().unwrap();
        if let Some((id, existing)) = registry.iter_mut().find(|(_, existing)| {
            existing.client.credential_account() == client.credential_account()
        }) {
            if existing.client.is_closed() {
                return Err(missing());
            }
            if existing.leases.values().sum::<usize>() >= 64 {
                return Err(capacity());
            }
            *existing.leases.entry(owner.into()).or_default() += 1;
            return Ok(RemoteConnectionInfo {
                connection_id: id.clone(),
                server: existing.client.address().into(),
                capabilities: existing.client.capabilities().clone(),
            });
        }
        let id = uuid::Uuid::new_v4().to_string();
        let info = RemoteConnectionInfo {
            connection_id: id.clone(),
            server: client.address().into(),
            capabilities: client.capabilities().clone(),
        };
        if registry.len() >= 8 {
            return Err(capacity());
        }
        registry.insert(
            id,
            Connection {
                client,
                leases: HashMap::from([(owner.into(), 1)]),
                authenticated: false,
                login: None,
                requests: HashMap::new(),
                cancelled: HashMap::new(),
                cancelled_subscriptions: HashMap::new(),
                subscriptions: HashMap::new(),
            },
        );
        Ok(info)
    }
    fn admit(
        &self,
        connection_id: &str,
        owner: &str,
        request_id: &str,
    ) -> Result<(Arc<RemoteClient>, CancellationToken, PendingRequest), ApplicationError> {
        let cancel = CancellationToken::new();
        let key = (owner.into(), request_id.to_owned());
        let (client, pending) = {
            let mut registry = self.registry.lock().unwrap();
            let connection = registry.get_mut(connection_id).ok_or_else(missing)?;
            if !connection.leases.contains_key(owner) {
                return Err(missing());
            }
            connection
                .cancelled
                .retain(|_, at| at.elapsed() < Duration::from_secs(60));
            if connection.cancelled.remove(&key).is_some() {
                let mut error = failure(
                    ErrorCode::Cancelled,
                    "The request was cancelled before admission",
                );
                error.request_id = Some(request_id.to_owned());
                error.mutation_outcome = Some(hypr_loofah_protocol::MutationOutcome::NotCommitted);
                return Err(error);
            }
            if connection.login.is_some() {
                return Err(failure(
                    ErrorCode::Conflict,
                    "Wait for browser login to finish",
                ));
            }
            if connection.requests.contains_key(&key) {
                return Err(failure(
                    ErrorCode::Conflict,
                    "This request is already running",
                ));
            }
            if connection.requests.len() >= 32 {
                return Err(capacity());
            }
            connection.requests.insert(key.clone(), cancel.clone());
            (
                connection.client.clone(),
                PendingRequest {
                    registry: self.registry.clone(),
                    connection: connection_id.to_owned(),
                    id: key,
                    login: false,
                },
            )
        };
        Ok((client, cancel, pending))
    }
    fn client(&self, id: &str, owner: &str) -> Result<Arc<RemoteClient>, ApplicationError> {
        self.registry
            .lock()
            .unwrap()
            .get(id)
            .filter(|connection| connection.leases.contains_key(owner))
            .map(|connection| connection.client.clone())
            .ok_or_else(missing)
    }
}

impl Connection {
    fn release(&mut self, owner: &str, all: bool) {
        if let Some(count) = self.leases.get_mut(owner) {
            if !all && *count > 1 {
                *count -= 1;
                return;
            }
            self.leases.remove(owner);
        } else {
            return;
        }
        if self
            .login
            .as_ref()
            .is_some_and(|((label, _), _)| label == owner)
        {
            self.login.take().unwrap().1.cancel();
        }
        self.requests.retain(|(label, _), cancel| {
            if label == owner {
                cancel.cancel();
                false
            } else {
                true
            }
        });
        self.cancelled.retain(|(label, _), _| label != owner);
        self.cancelled_subscriptions
            .retain(|(label, _), _| label != owner);
        self.subscriptions.retain(|(label, _), _| label != owner);
    }
}

#[derive(Clone, serde::Serialize, specta::Type)]
pub struct RemoteConnectionInfo {
    pub connection_id: String,
    pub server: String,
    pub capabilities: Capabilities,
}

fn missing() -> ApplicationError {
    failure(ErrorCode::Cancelled, "This server connection is closed")
}
fn capacity() -> ApplicationError {
    failure(
        ErrorCode::ResourceLimit,
        "Too many active server connections or requests",
    )
}

async fn discover<R: tauri::Runtime>(
    _app: &tauri::AppHandle<R>,
    server: &str,
    credentials: Arc<dyn CredentialStore>,
) -> Result<Arc<RemoteClient>, ApplicationError> {
    #[cfg(feature = "dev")]
    if _app.config().identifier == "io.loofah.remote-qa" {
        if let Some(path) = std::env::var_os("LOOFAH_QA_ROOT_CA") {
            let root = tokio::task::spawn_blocking(move || {
                let path = std::path::PathBuf::from(path);
                let metadata = std::fs::metadata(&path).map_err(|_| {
                    failure(ErrorCode::Validation, "Cannot read QA root certificate")
                })?;
                if !metadata.is_file() || metadata.len() > 131_072 {
                    return Err(failure(
                        ErrorCode::Validation,
                        "Invalid QA root certificate file",
                    ));
                }
                std::fs::read(path)
                    .map_err(|_| failure(ErrorCode::Validation, "Cannot read QA root certificate"))
            })
            .await
            .map_err(|_| failure(ErrorCode::Internal, "Cannot load QA certificate"))??;
            return RemoteClient::discover_fixture(server, credentials, &root).await;
        }
    }
    RemoteClient::discover(server, credentials, false).await
}

#[tauri::command]
#[specta::specta]
pub async fn remote_connect<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    server: String,
) -> Result<RemoteConnectionInfo, ApplicationError> {
    let state = app.state::<RemoteState>();
    let lifetime = state.window_lifetime(window.label());
    let credentials = Arc::new(OsCredentials::new(format!(
        "{}.remote-auth",
        app.config().identifier
    )));
    let client = tokio::select! {
        _ = lifetime.cancelled() => return Err(missing()),
        result = discover(&app, &server, credentials) => result?,
    };
    state.register(client, window.label(), &lifetime)
}

struct PendingRequest {
    registry: Arc<Mutex<HashMap<String, Connection>>>,
    connection: String,
    id: WorkId,
    login: bool,
}
impl Drop for PendingRequest {
    fn drop(&mut self) {
        if let Some(connection) = self.registry.lock().unwrap().get_mut(&self.connection) {
            if self.login {
                if connection
                    .login
                    .as_ref()
                    .is_some_and(|(id, _)| id == &self.id)
                {
                    connection.login.take().unwrap().1.cancel();
                }
            } else if let Some(cancel) = connection.requests.remove(&self.id) {
                cancel.cancel();
            }
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn remote_login<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
) -> Result<Me, ApplicationError> {
    let state = app.state::<RemoteState>();
    let client = state.client(&connection_id, window.label())?;
    let login = client.begin_login().await?;
    let cancel = login.cancellation();
    let pending = {
        let mut registry = state.registry.lock().unwrap();
        let connection = registry.get_mut(&connection_id).ok_or_else(missing)?;
        if !connection.leases.contains_key(window.label()) {
            return Err(missing());
        }
        if !connection.requests.is_empty() {
            return Err(failure(
                ErrorCode::Conflict,
                "Wait for active server requests before signing in",
            ));
        }
        if connection.authenticated {
            return Err(failure(
                ErrorCode::Conflict,
                "Sign out before changing the server account",
            ));
        }
        if connection.login.is_some() {
            return Err(failure(
                ErrorCode::Conflict,
                "Browser login is already in progress",
            ));
        }
        let id = (window.label().into(), uuid::Uuid::new_v4().to_string());
        connection.login = Some((id.clone(), cancel.clone()));
        PendingRequest {
            registry: state.registry.clone(),
            connection: connection_id,
            id,
            login: true,
        }
    };
    let url = login.authorization_url().to_string();
    let browser_app = app.clone();
    tokio::task::spawn_blocking(move || browser_app.opener().open_url(url, None::<&str>))
        .await
        .map_err(|_| failure(ErrorCode::Unavailable, "Cannot open the browser for login"))?
        .map_err(|_| failure(ErrorCode::Unavailable, "Cannot open the browser for login"))?;
    login.complete().await?;
    let result = client
        .execute(
            RemoteOperation::Provision,
            &uuid::Uuid::new_v4().to_string(),
            cancel,
        )
        .await?;
    match result {
        RemoteReply::Me(me) => {
            if let Some(connection) = state.registry.lock().unwrap().get_mut(&pending.connection) {
                connection.authenticated = true;
            }
            Ok(me)
        }
        _ => Err(failure(
            ErrorCode::IncompatibleVersion,
            "Invalid server login response",
        )),
    }
}

#[tauri::command]
#[specta::specta]
pub fn remote_cancel_login<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
) {
    if let Some(connection) = app
        .state::<RemoteState>()
        .registry
        .lock()
        .unwrap()
        .get(&connection_id)
    {
        if let Some(((owner, _), cancel)) = &connection.login {
            if owner == window.label() {
                cancel.cancel();
            }
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn remote_request<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    request_id: String,
    operation: RemoteOperation,
) -> Result<RemoteReply, ApplicationError> {
    let state = app.state::<RemoteState>();
    let (client, cancel, _pending) = state.admit(&connection_id, window.label(), &request_id)?;
    let reply = client.execute(operation, &request_id, cancel).await?;
    if matches!(&reply, RemoteReply::Me(_)) {
        if let Some(connection) = state.registry.lock().unwrap().get_mut(&_pending.connection) {
            connection.authenticated = true;
        }
    }
    Ok(reply)
}

#[tauri::command]
#[specta::specta]
pub async fn remote_upload_attachment_path<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    upload: hypr_loofah_client::AttachmentUpload,
    source_path: String,
) -> Result<hypr_loofah_protocol::FileMetadata, ApplicationError> {
    let state = app.state::<RemoteState>();
    let (client, cancel, _pending) =
        state.admit(&connection_id, window.label(), &upload.request_id)?;
    if !client
        .capabilities()
        .features
        .iter()
        .any(|feature| feature == "attachment_upload")
    {
        let mut cause = failure(
            ErrorCode::IncompatibleVersion,
            "This server does not support attachment uploads. Upgrade the server, then reconnect.",
        );
        cause.mutation_outcome = Some(hypr_loofah_protocol::MutationOutcome::NotCommitted);
        cause.request_id = Some(upload.request_id);
        return Err(cause);
    }
    let source = app
        .state::<crate::attachment_upload::DeviceUploadIo>()
        .source(source_path, &upload.input)?;
    let _admission = source.admission();
    client.upload_attachment(upload, source, cancel).await
}

#[tauri::command]
#[specta::specta]
pub fn remote_cancel_request<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    request_id: String,
) {
    if uuid::Uuid::parse_str(&request_id).is_err() {
        return;
    }
    let state = app.state::<RemoteState>();
    let mut registry = state.registry.lock().unwrap();
    if let Some(connection) = registry.get_mut(&connection_id) {
        if !connection.leases.contains_key(window.label()) {
            return;
        }
        let key = (window.label().into(), request_id);
        if let Some(cancel) = connection.requests.get(&key) {
            cancel.cancel();
        } else {
            connection
                .cancelled
                .retain(|_, at| at.elapsed() < Duration::from_secs(60));
            if connection.cancelled.len() < 256 {
                connection.cancelled.insert(key, Instant::now());
            }
        }
    }
}

#[tauri::command]
#[specta::specta]
pub fn remote_subscribe<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    vault_id: String,
    subscription_id: String,
    on_event: Channel<RemoteEvent>,
) -> Result<(), ApplicationError> {
    if uuid::Uuid::parse_str(&subscription_id).is_err() {
        return Err(failure(
            ErrorCode::Validation,
            "A UUID subscription ID is required",
        ));
    }
    let state = app.state::<RemoteState>();
    let mut registry = state.registry.lock().unwrap();
    let connection = registry.get_mut(&connection_id).ok_or_else(missing)?;
    if !connection.leases.contains_key(window.label()) {
        return Err(missing());
    }
    let key = (window.label().into(), subscription_id);
    connection
        .cancelled_subscriptions
        .retain(|_, at| at.elapsed() < Duration::from_secs(60));
    if connection.cancelled_subscriptions.remove(&key).is_some() {
        return Ok(());
    }
    if connection.subscriptions.contains_key(&key) {
        return Err(failure(
            ErrorCode::Conflict,
            "This subscription is already running",
        ));
    }
    if connection.subscriptions.len() >= 8 {
        return Err(capacity());
    }
    let subscription = connection
        .client
        .subscribe(vault_id, move |event| on_event.send(event).is_ok())?;
    connection.subscriptions.insert(key, subscription);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn remote_unsubscribe<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    subscription_id: String,
) {
    if let Some(connection) = app
        .state::<RemoteState>()
        .registry
        .lock()
        .unwrap()
        .get_mut(&connection_id)
    {
        if !connection.leases.contains_key(window.label()) {
            return;
        }
        let key = (window.label().into(), subscription_id);
        if connection.subscriptions.remove(&key).is_none() {
            connection
                .cancelled_subscriptions
                .retain(|_, at| at.elapsed() < Duration::from_secs(60));
            if connection.cancelled_subscriptions.len() < 256 {
                connection
                    .cancelled_subscriptions
                    .insert(key, Instant::now());
            }
        }
    }
}

#[tauri::command]
#[specta::specta]
pub fn remote_disconnect<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
) {
    let state = app.state::<RemoteState>();
    let mut registry = state.registry.lock().unwrap();
    if let Some(connection) = registry.get_mut(&connection_id) {
        connection.release(window.label(), false);
        if connection.leases.is_empty() {
            registry.remove(&connection_id);
        }
    }
    drop(registry);
    state.media.prune();
    state.attachments.prune(&state);
}

#[tauri::command]
#[specta::specta]
pub async fn remote_logout<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
) -> Result<(), ApplicationError> {
    let state = app.state::<RemoteState>();
    let client = state.client(&connection_id, window.label())?;
    state.attachments.cancel_connection(&connection_id).await;
    client.logout().await?;
    state.registry.lock().unwrap().remove(&connection_id);
    state.media.prune();
    state.attachments.prune(&state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::get};
    use hypr_loofah_client::CredentialFuture;
    use serde_json::json;

    fn test_window(
        app: &tauri::App<tauri::test::MockRuntime>,
        label: &str,
    ) -> tauri::Window<tauri::test::MockRuntime> {
        let window = tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::default())
            .build()
            .unwrap();
        window.as_ref().window()
    }

    struct NoCredentials;
    impl CredentialStore for NoCredentials {
        fn load<'a>(&'a self, _: &'a str) -> CredentialFuture<'a, Option<String>> {
            Box::pin(async { Ok(None) })
        }
        fn save<'a>(&'a self, _: &'a str, _: Option<&'a str>) -> CredentialFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    async fn clients() -> (
        Arc<RemoteClient>,
        Arc<RemoteClient>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let config = json!({"issuer":origin,"authorization_endpoint":format!("{origin}/authorize"),"token_endpoint":format!("{origin}/token"),"client_id":"desktop","audience":"api","scopes":["loofah:access"]});
        let discovery = json!({"issuer":origin,"authorization_endpoint":format!("{origin}/authorize"),"token_endpoint":format!("{origin}/token"),"code_challenge_methods_supported":["S256"]});
        let app = Router::new()
            .route(
                "/api/v1/capabilities",
                get(|| async {
                    Json(json!({"api_major":1,"application_version":"test","features":["notes"]}))
                }),
            )
            .route(
                "/api/v1/auth/config",
                get(move || async move { Json(config) }),
            )
            .route(
                "/.well-known/openid-configuration",
                get(move || async move { Json(discovery) }),
            );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let first = RemoteClient::discover(&origin, Arc::new(NoCredentials), true)
            .await
            .unwrap();
        let second = RemoteClient::discover(&origin, Arc::new(NoCredentials), true)
            .await
            .unwrap();
        (first, second, task)
    }

    #[tokio::test]
    async fn windows_share_one_refresh_coordinator_and_releasing_one_keeps_the_other_alive() {
        let (first, second, task) = clients().await;
        let app = tauri::test::mock_builder()
            .manage(RemoteState::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let state = app.state::<RemoteState>();
        let first_window = test_window(&app, "first");
        let second_window = test_window(&app, "second");
        let first_info = state
            .register(first.clone(), "first", &state.window_lifetime("first"))
            .unwrap();
        let second_info = state
            .register(second, "second", &state.window_lifetime("second"))
            .unwrap();
        assert_eq!(first_info.connection_id, second_info.connection_id);
        assert!(Arc::ptr_eq(
            &state.client(&first_info.connection_id, "first").unwrap(),
            &first
        ));
        remote_disconnect(
            app.handle().clone(),
            first_window,
            first_info.connection_id.clone(),
        );
        assert!(state.client(&first_info.connection_id, "second").is_ok());
        assert!(state.client(&first_info.connection_id, "first").is_err());
        remote_disconnect(
            app.handle().clone(),
            second_window,
            second_info.connection_id,
        );
        assert!(state.client(&first_info.connection_id, "second").is_err());
        assert!(first.begin_login().await.is_err());
        task.abort();
    }

    #[tokio::test]
    async fn cancellation_can_arrive_before_request_admission_without_contacting_the_api() {
        let (first, _, task) = clients().await;
        let app = tauri::test::mock_builder()
            .manage(RemoteState::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let window = test_window(&app, "first");
        let state = app.state::<RemoteState>();
        let info = state
            .register(first, "first", &state.window_lifetime("first"))
            .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        remote_cancel_request(
            app.handle().clone(),
            window.clone(),
            info.connection_id.clone(),
            id.clone(),
        );
        let error = remote_request(
            app.handle().clone(),
            window.clone(),
            info.connection_id,
            id.clone(),
            RemoteOperation::Provision,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Cancelled);
        assert_eq!(error.request_id.as_deref(), Some(id.as_str()));
        assert_eq!(
            error.mutation_outcome,
            Some(hypr_loofah_protocol::MutationOutcome::NotCommitted)
        );
        let state = app.state::<RemoteState>();
        let connection_id = state
            .registry
            .lock()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        let subscription_id = uuid::Uuid::new_v4().to_string();
        remote_unsubscribe(
            app.handle().clone(),
            window.clone(),
            connection_id.clone(),
            subscription_id.clone(),
        );
        remote_subscribe(
            app.handle().clone(),
            window,
            connection_id.clone(),
            "vault".into(),
            subscription_id,
            Channel::new(|_| Ok(())),
        )
        .unwrap();
        assert!(
            state.registry.lock().unwrap()[&connection_id]
                .subscriptions
                .is_empty()
        );
        task.abort();
    }

    #[tokio::test]
    async fn destroying_a_window_cancels_only_its_work_and_rejects_late_discovery() {
        let (first, second, task) = clients().await;
        let state = RemoteState::default();
        let lifetime = state.window_lifetime("first");
        let info = state.register(first.clone(), "first", &lifetime).unwrap();
        state
            .register(second.clone(), "second", &state.window_lifetime("second"))
            .unwrap();
        let cancelled = CancellationToken::new();
        let retained = CancellationToken::new();
        let login = CancellationToken::new();
        {
            let mut registry = state.registry.lock().unwrap();
            let connection = registry.get_mut(&info.connection_id).unwrap();
            connection
                .requests
                .insert(("first".into(), "request".into()), cancelled.clone());
            connection
                .requests
                .insert(("second".into(), "request".into()), retained.clone());
            connection.login = Some((("first".into(), "login".into()), login.clone()));
            connection.subscriptions.insert(
                ("first".into(), "stream".into()),
                first.subscribe("vault".into(), |_| true).unwrap(),
            );
            connection.subscriptions.insert(
                ("second".into(), "stream".into()),
                first.subscribe("vault".into(), |_| true).unwrap(),
            );
        }
        state.release_window("first");
        assert!(lifetime.is_cancelled());
        assert!(cancelled.is_cancelled());
        assert!(login.is_cancelled());
        assert!(!retained.is_cancelled());
        assert!(!first.is_closed());
        assert!(state.client(&info.connection_id, "first").is_err());
        assert!(state.client(&info.connection_id, "second").is_ok());
        {
            let registry = state.registry.lock().unwrap();
            let connection = &registry[&info.connection_id];
            assert_eq!(connection.requests.len(), 1);
            assert_eq!(connection.subscriptions.len(), 1);
        }
        let reopened = state.window_lifetime("first");
        assert!(!reopened.is_cancelled());
        assert!(state.register(second.clone(), "first", &lifetime).is_err());
        state.register(second, "first", &reopened).unwrap();
        state.release_window("second");
        assert!(retained.is_cancelled());
        assert!(!first.is_closed());
        state.release_window("first");
        assert!(first.is_closed());
        assert!(state.registry.lock().unwrap().is_empty());
        task.abort();
    }

    #[tokio::test]
    async fn windows_cannot_cancel_or_use_another_windows_lease() {
        let (first, second, task) = clients().await;
        let app = tauri::test::mock_builder()
            .manage(RemoteState::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let state = app.state::<RemoteState>();
        let first_window = test_window(&app, "first");
        let second_window = test_window(&app, "second");
        let outsider = test_window(&app, "outsider");
        let info = state
            .register(first, "first", &state.window_lifetime("first"))
            .unwrap();
        state
            .register(second, "second", &state.window_lifetime("second"))
            .unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        state
            .registry
            .lock()
            .unwrap()
            .get_mut(&info.connection_id)
            .unwrap()
            .requests
            .insert(("first".into(), request_id.clone()), cancel.clone());
        remote_cancel_request(
            app.handle().clone(),
            second_window.clone(),
            info.connection_id.clone(),
            request_id.clone(),
        );
        assert!(!cancel.is_cancelled());
        let error = remote_request(
            app.handle().clone(),
            outsider.clone(),
            info.connection_id.clone(),
            uuid::Uuid::new_v4().to_string(),
            RemoteOperation::Provision,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Cancelled);
        remote_disconnect(app.handle().clone(), outsider, info.connection_id.clone());
        assert!(state.client(&info.connection_id, "first").is_ok());
        remote_cancel_request(
            app.handle().clone(),
            first_window,
            info.connection_id.clone(),
            request_id,
        );
        assert!(cancel.is_cancelled());
        state.release_window("first");
        assert!(state.client(&info.connection_id, "second").is_ok());
        remote_disconnect(app.handle().clone(), second_window, info.connection_id);
        task.abort();
    }
}

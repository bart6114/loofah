use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hypr_loofah_client::{
    CredentialFuture, CredentialStore, RemoteClient, RemoteEvent, RemoteOperation, RemoteReply,
    Subscription, failure,
};
use hypr_loofah_protocol::{ApplicationError, Capabilities, ErrorCode, Me};
use tauri::{Manager, ipc::Channel};
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct RemoteState {
    registry: Arc<Mutex<HashMap<String, Connection>>>,
}
struct Connection {
    client: Arc<RemoteClient>,
    leases: usize,
    authenticated: bool,
    login: Option<CancellationToken>,
    requests: HashMap<String, CancellationToken>,
    cancelled: HashMap<String, Instant>,
    cancelled_subscriptions: HashMap<String, Instant>,
    subscriptions: HashMap<String, Subscription>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.client.disconnect();
        if let Some(login) = &self.login {
            login.cancel();
        }
        for cancel in self.requests.values() {
            cancel.cancel();
        }
    }
}
impl RemoteState {
    pub fn shutdown(&self) {
        self.registry.lock().unwrap().clear();
    }
    fn register(
        &self,
        client: Arc<RemoteClient>,
    ) -> Result<RemoteConnectionInfo, ApplicationError> {
        let mut registry = self.registry.lock().unwrap();
        if let Some((id, existing)) = registry.iter_mut().find(|(_, existing)| {
            existing.client.credential_account() == client.credential_account()
        }) {
            if existing.client.is_closed() {
                return Err(missing());
            }
            if existing.leases >= 64 {
                return Err(capacity());
            }
            existing.leases += 1;
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
                leases: 1,
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
    fn client(&self, id: &str) -> Result<Arc<RemoteClient>, ApplicationError> {
        self.registry
            .lock()
            .unwrap()
            .get(id)
            .map(|connection| connection.client.clone())
            .ok_or_else(missing)
    }
}

#[derive(Clone, serde::Serialize, specta::Type)]
pub struct RemoteConnectionInfo {
    pub connection_id: String,
    pub server: String,
    pub capabilities: Capabilities,
}

struct OsCredentials {
    service: String,
}
impl CredentialStore for OsCredentials {
    fn load<'a>(&'a self, account: &'a str) -> CredentialFuture<'a, Option<String>> {
        let service = self.service.clone();
        let account = account.to_string();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let entry =
                    keyring::Entry::new(&service, &account).map_err(|_| credential_error())?;
                match entry.get_password() {
                    Ok(value) => Ok(Some(value)),
                    Err(keyring::Error::NoEntry) => Ok(None),
                    Err(_) => Err(credential_error()),
                }
            })
            .await
            .map_err(|_| credential_error())?
        })
    }
    fn save<'a>(&'a self, account: &'a str, value: Option<&'a str>) -> CredentialFuture<'a, ()> {
        let service = self.service.clone();
        let account = account.to_string();
        let value = value.map(str::to_owned);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let entry =
                    keyring::Entry::new(&service, &account).map_err(|_| credential_error())?;
                if let Some(value) = value {
                    entry.set_password(&value).map_err(|_| credential_error())
                } else {
                    match entry.delete_credential() {
                        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                        Err(_) => Err(credential_error()),
                    }
                }
            })
            .await
            .map_err(|_| credential_error())?
        })
    }
}
fn credential_error() -> ApplicationError {
    failure(
        ErrorCode::Internal,
        "Cannot access saved server credentials. Unlock the device credential store and try again.",
    )
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

#[tauri::command]
#[specta::specta]
pub async fn remote_connect<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    server: String,
) -> Result<RemoteConnectionInfo, ApplicationError> {
    let credentials = Arc::new(OsCredentials {
        service: format!("{}.remote-auth", app.config().identifier),
    });
    let client = RemoteClient::discover(&server, credentials, false).await?;
    app.state::<RemoteState>().register(client)
}

struct PendingRequest {
    registry: Arc<Mutex<HashMap<String, Connection>>>,
    connection: String,
    id: String,
    login: bool,
}
impl Drop for PendingRequest {
    fn drop(&mut self) {
        if let Some(connection) = self.registry.lock().unwrap().get_mut(&self.connection) {
            if self.login {
                if let Some(login) = connection.login.take() {
                    login.cancel();
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
    connection_id: String,
) -> Result<Me, ApplicationError> {
    let state = app.state::<RemoteState>();
    let client = state.client(&connection_id)?;
    let login = client.begin_login().await?;
    let cancel = login.cancellation();
    let pending = {
        let mut registry = state.registry.lock().unwrap();
        let connection = registry.get_mut(&connection_id).ok_or_else(missing)?;
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
        connection.login = Some(cancel.clone());
        PendingRequest {
            registry: state.registry.clone(),
            connection: connection_id,
            id: String::new(),
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
pub fn remote_cancel_login<R: tauri::Runtime>(app: tauri::AppHandle<R>, connection_id: String) {
    if let Some(connection) = app
        .state::<RemoteState>()
        .registry
        .lock()
        .unwrap()
        .get(&connection_id)
    {
        if let Some(cancel) = &connection.login {
            cancel.cancel();
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn remote_request<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    connection_id: String,
    request_id: String,
    operation: RemoteOperation,
) -> Result<RemoteReply, ApplicationError> {
    let state = app.state::<RemoteState>();
    let cancel = CancellationToken::new();
    let (client, _pending) = {
        let mut registry = state.registry.lock().unwrap();
        let connection = registry.get_mut(&connection_id).ok_or_else(missing)?;
        connection
            .cancelled
            .retain(|_, at| at.elapsed() < Duration::from_secs(60));
        if connection.cancelled.remove(&request_id).is_some() {
            let mut error = failure(
                ErrorCode::Cancelled,
                "The request was cancelled before admission",
            );
            error.request_id = Some(request_id);
            error.mutation_outcome = Some(hypr_loofah_protocol::MutationOutcome::NotCommitted);
            return Err(error);
        }
        if connection.login.is_some() {
            return Err(failure(
                ErrorCode::Conflict,
                "Wait for browser login to finish",
            ));
        }
        if connection.requests.contains_key(&request_id) {
            return Err(failure(
                ErrorCode::Conflict,
                "This request is already running",
            ));
        }
        if connection.requests.len() >= 32 {
            return Err(capacity());
        }
        connection
            .requests
            .insert(request_id.clone(), cancel.clone());
        (
            connection.client.clone(),
            PendingRequest {
                registry: state.registry.clone(),
                connection: connection_id,
                id: request_id.clone(),
                login: false,
            },
        )
    };
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
pub fn remote_cancel_request<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    connection_id: String,
    request_id: String,
) {
    if uuid::Uuid::parse_str(&request_id).is_err() {
        return;
    }
    let state = app.state::<RemoteState>();
    let mut registry = state.registry.lock().unwrap();
    if let Some(connection) = registry.get_mut(&connection_id) {
        if let Some(cancel) = connection.requests.get(&request_id) {
            cancel.cancel();
        } else {
            connection
                .cancelled
                .retain(|_, at| at.elapsed() < Duration::from_secs(60));
            if connection.cancelled.len() < 256 {
                connection.cancelled.insert(request_id, Instant::now());
            }
        }
    }
}

#[tauri::command]
#[specta::specta]
pub fn remote_subscribe<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
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
    connection
        .cancelled_subscriptions
        .retain(|_, at| at.elapsed() < Duration::from_secs(60));
    if connection
        .cancelled_subscriptions
        .remove(&subscription_id)
        .is_some()
    {
        return Ok(());
    }
    if connection.subscriptions.contains_key(&subscription_id) {
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
    connection
        .subscriptions
        .insert(subscription_id, subscription);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn remote_unsubscribe<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
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
        if connection.subscriptions.remove(&subscription_id).is_none() {
            connection
                .cancelled_subscriptions
                .retain(|_, at| at.elapsed() < Duration::from_secs(60));
            if connection.cancelled_subscriptions.len() < 256 {
                connection
                    .cancelled_subscriptions
                    .insert(subscription_id, Instant::now());
            }
        }
    }
}

#[tauri::command]
#[specta::specta]
pub fn remote_disconnect<R: tauri::Runtime>(app: tauri::AppHandle<R>, connection_id: String) {
    let state = app.state::<RemoteState>();
    let mut registry = state.registry.lock().unwrap();
    if let Some(connection) = registry.get_mut(&connection_id) {
        if connection.leases > 1 {
            connection.leases -= 1;
        } else {
            registry.remove(&connection_id);
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn remote_logout<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    connection_id: String,
) -> Result<(), ApplicationError> {
    let state = app.state::<RemoteState>();
    let client = state.client(&connection_id)?;
    client.logout().await?;
    state.registry.lock().unwrap().remove(&connection_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::get};
    use serde_json::json;

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
        let first_info = state.register(first.clone()).unwrap();
        let second_info = state.register(second).unwrap();
        assert_eq!(first_info.connection_id, second_info.connection_id);
        assert!(Arc::ptr_eq(
            &state.client(&first_info.connection_id).unwrap(),
            &first
        ));
        remote_disconnect(app.handle().clone(), first_info.connection_id.clone());
        assert!(state.client(&first_info.connection_id).is_ok());
        remote_disconnect(app.handle().clone(), second_info.connection_id);
        assert!(state.client(&first_info.connection_id).is_err());
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
        let info = app.state::<RemoteState>().register(first).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        remote_cancel_request(app.handle().clone(), info.connection_id.clone(), id.clone());
        let error = remote_request(
            app.handle().clone(),
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
            connection_id.clone(),
            subscription_id.clone(),
        );
        remote_subscribe(
            app.handle().clone(),
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
}

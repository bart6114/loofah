use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{Form, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Sse, sse::Event},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt;
use hypr_loofah_protocol::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::*;

#[derive(Default)]
struct MemoryCredentials {
    values: Mutex<HashMap<String, String>>,
    saves: AtomicUsize,
    fail: AtomicBool,
    save_delay: AtomicUsize,
}
impl CredentialStore for MemoryCredentials {
    fn load<'a>(&'a self, account: &'a str) -> CredentialFuture<'a, Option<String>> {
        Box::pin(async move { Ok(self.values.lock().await.get(account).cloned()) })
    }
    fn save<'a>(&'a self, account: &'a str, value: Option<&'a str>) -> CredentialFuture<'a, ()> {
        Box::pin(async move {
            if self.fail.load(Ordering::SeqCst) {
                return Err(failure(ErrorCode::Internal, "Credential store is locked"));
            }
            self.saves.fetch_add(1, Ordering::SeqCst);
            let delay = self.save_delay.load(Ordering::SeqCst);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay as u64)).await;
            }
            let mut values = self.values.lock().await;
            match value {
                Some(value) => {
                    values.insert(account.into(), value.into());
                }
                None => {
                    values.remove(account);
                }
            }
            Ok(())
        })
    }
}

struct Provider {
    origin: String,
    codes: Mutex<HashMap<String, (String, String)>>,
    refresh: Mutex<Option<String>>,
    issued: AtomicUsize,
    refreshes: AtomicUsize,
    denied: AtomicBool,
    stalled_events: AtomicBool,
    delay: AtomicUsize,
    event_connections: Mutex<Vec<Option<String>>>,
}
struct Fixture {
    state: Arc<Provider>,
    task: tokio::task::JoinHandle<()>,
    credentials: Arc<MemoryCredentials>,
    proxy: Option<TlsProxy>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        Self::with_tls(None).await
    }

    async fn with_tls(round_trip: Option<Duration>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        let proxy = match round_trip {
            Some(delay) => Some(TlsProxy::start(upstream, delay).await),
            None => None,
        };
        let origin = proxy
            .as_ref()
            .map(|proxy| proxy.origin.clone())
            .unwrap_or_else(|| format!("http://{upstream}"));
        let state = Arc::new(Provider {
            origin,
            codes: Mutex::new(HashMap::new()),
            refresh: Mutex::new(None),
            issued: AtomicUsize::new(0),
            refreshes: AtomicUsize::new(0),
            denied: AtomicBool::new(false),
            stalled_events: AtomicBool::new(false),
            delay: AtomicUsize::new(0),
            event_connections: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .route(
                "/api/v1/capabilities",
                get(|| async {
                    Json(Capabilities {
                        api_major: API_MAJOR,
                        application_version: "test".into(),
                        features: vec!["notes".into()],
                    })
                }),
            )
            .route("/api/v1/auth/config", get(config))
            .route("/api/v1/me", get(me).post(me))
            .route("/api/v1/vaults/test/events", get(events))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            state,
            task,
            credentials: Arc::new(MemoryCredentials::default()),
            proxy,
        }
    }
    fn browser(&self) -> reqwest::Client {
        let mut builder = reqwest::Client::builder();
        if self.proxy.is_some() {
            builder = builder.tls_certs_only([reqwest::Certificate::from_pem(include_bytes!(
                "../tests/fixtures/localhost-ca.pem"
            ))
            .unwrap()]);
        }
        builder.build().unwrap()
    }
    async fn client(&self) -> Arc<RemoteClient> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .tls_certs_only([reqwest::Certificate::from_pem(include_bytes!(
                "../tests/fixtures/localhost-ca.pem"
            ))
            .unwrap()])
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        RemoteClient::discover_http(
            &self.state.origin,
            self.credentials.clone(),
            self.proxy.is_none(),
            http,
        )
        .await
        .unwrap()
    }
    async fn login(&self, client: &Arc<RemoteClient>) {
        let login = client.begin_login().await.unwrap();
        let url = login.authorization_url().to_owned();
        let complete = tokio::spawn(login.complete());
        let response = self.browser().get(url).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.text().await.unwrap().contains("return to Loofah"));
        complete.await.unwrap().unwrap();
    }
}
async fn discovery(State(state): State<Arc<Provider>>) -> Json<Value> {
    Json(
        json!({"issuer":state.origin,"authorization_endpoint":format!("{}/authorize",state.origin),"token_endpoint":format!("{}/token",state.origin),"code_challenge_methods_supported":["S256"]}),
    )
}
async fn config(State(state): State<Arc<Provider>>) -> Json<RemoteAuthConfig> {
    Json(RemoteAuthConfig {
        issuer: state.origin.clone(),
        authorization_endpoint: format!("{}/authorize", state.origin),
        token_endpoint: format!("{}/token", state.origin),
        client_id: "desktop".into(),
        audience: "api".into(),
        scopes: vec![
            "openid".into(),
            "loofah:access".into(),
            "offline_access".into(),
        ],
    })
}
async fn authorize(
    State(state): State<Arc<Provider>>,
    Query(query): Query<HashMap<String, String>>,
) -> Redirect {
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["audience"], "api");
    assert_eq!(query["client_id"], "desktop");
    let redirect = query["redirect_uri"].clone();
    assert!(redirect.starts_with("http://127.0.0.1:"));
    let code = uuid::Uuid::new_v4().to_string();
    state.codes.lock().await.insert(
        code.clone(),
        (query["code_challenge"].clone(), redirect.clone()),
    );
    let mut callback = Url::parse(&redirect).unwrap();
    callback.query_pairs_mut().extend_pairs([
        ("code", code.as_str()),
        ("state", query["state"].as_str()),
        ("iss", state.origin.as_str()),
    ]);
    Redirect::to(callback.as_str())
}
async fn token(
    State(state): State<Arc<Provider>>,
    Form(form): Form<HashMap<String, String>>,
) -> axum::response::Response {
    tokio::time::sleep(Duration::from_millis(
        state.delay.load(Ordering::SeqCst) as u64
    ))
    .await;
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let Some((challenge, redirect)) = state.codes.lock().await.remove(&form["code"]) else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            assert_eq!(
                challenge,
                URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes()))
            );
            assert_eq!(redirect, form["redirect_uri"]);
        }
        Some("refresh_token") => {
            let mut refresh = state.refresh.lock().await;
            if refresh.as_deref() != form.get("refresh_token").map(String::as_str) {
                return StatusCode::BAD_REQUEST.into_response();
            }
            *refresh = None;
            state.refreshes.fetch_add(1, Ordering::SeqCst);
        }
        _ => return StatusCode::BAD_REQUEST.into_response(),
    }
    let number = state.issued.fetch_add(1, Ordering::SeqCst) + 1;
    let refresh = format!("refresh-{number}");
    *state.refresh.lock().await = Some(refresh.clone());
    Json(json!({"token_type":"Bearer","access_token":format!("access-{number}"),"refresh_token":refresh,"expires_in":3600})).into_response()
}
async fn me(State(state): State<Arc<Provider>>, headers: HeaderMap) -> axum::response::Response {
    tokio::time::sleep(Duration::from_millis(
        state.delay.load(Ordering::SeqCst) as u64
    ))
    .await;
    if headers
        .get("authorization")
        .and_then(|header| header.to_str().ok())
        .is_some_and(|header| header.starts_with("Bearer access-"))
    {
        assert_eq!(headers["x-loofah-api-major"], "1");
        assert!(uuid::Uuid::parse_str(headers["x-request-id"].to_str().unwrap()).is_ok());
        Json(Me {
            member: MemberInfo {
                id: "member".into(),
                name: "Member".into(),
            },
            personal_vault_id: "test".into(),
        })
        .into_response()
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(failure(
                ErrorCode::Unauthenticated,
                "Authentication required",
            )),
        )
            .into_response()
    }
}
async fn events(
    State(state): State<Arc<Provider>>,
    headers: HeaderMap,
) -> axum::response::Response {
    if state.denied.load(Ordering::SeqCst) {
        return (
            StatusCode::FORBIDDEN,
            Json(failure(ErrorCode::Forbidden, "Access revoked")),
        )
            .into_response();
    }
    let cursor = headers
        .get("last-event-id")
        .and_then(|header| header.to_str().ok())
        .map(str::to_owned);
    let mut connections = state.event_connections.lock().await;
    connections.push(cursor.clone());
    let index = connections.len();
    drop(connections);
    let change = if index == 1 {
        Change::ResyncRequired
    } else {
        Change::IndexChanged {
            entity: IndexEntity::Sessions,
            ids: vec!["note".into()],
        }
    };
    let event = ChangeEvent {
        vault_id: "test".into(),
        cursor: format!("epoch:{index}"),
        change,
    };
    let stream = futures_util::stream::iter([Ok::<_, std::convert::Infallible>(
        Event::default()
            .id(&event.cursor)
            .event("change")
            .data(serde_json::to_string(&event).unwrap()),
    )]);
    if state.stalled_events.load(Ordering::SeqCst) {
        Sse::new(stream.chain(futures_util::stream::pending())).into_response()
    } else {
        Sse::new(stream).into_response()
    }
}

#[tokio::test]
async fn coherent_editor_reads_require_an_advertised_feature_before_authentication_or_requests() {
    let fixture = Fixture::new().await;
    let client = fixture.client().await;
    let request_id = uuid::Uuid::new_v4().to_string();
    let error = client
        .execute(
            RemoteOperation::NoteEditor {
                vault_id: "test".into(),
                session_id: "note".into(),
            },
            &request_id,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::IncompatibleVersion);
    assert_eq!(error.request_id.as_deref(), Some(request_id.as_str()));
    assert!(!error.retryable);
    assert_eq!(fixture.state.issued.load(Ordering::SeqCst), 0);
    assert!(fixture.credentials.values.lock().await.is_empty());
}

#[tokio::test]
async fn browser_pkce_login_keeps_only_refresh_credentials_and_restores_without_browser() {
    let fixture = Fixture::new().await;
    let client = fixture.client().await;
    fixture.login(&client).await;
    let reply = client
        .execute(
            RemoteOperation::Me,
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        matches!(reply, RemoteReply::Me(Me { personal_vault_id, .. }) if personal_vault_id == "test")
    );
    let saved = fixture
        .credentials
        .values
        .lock()
        .await
        .values()
        .next()
        .unwrap()
        .clone();
    assert!(saved.contains("refresh-1"));
    assert!(!saved.contains("access-1"));
    client.disconnect();
    let restored = fixture.client().await;
    assert_eq!(restored.access_token().await.unwrap(), "access-2");
    assert_eq!(fixture.state.refreshes.load(Ordering::SeqCst), 1);
}

#[cfg(feature = "dev-fixtures")]
#[tokio::test]
async fn fixture_transport_validates_tls_and_cannot_target_public_or_plain_http_origins() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let pem = include_bytes!("../tests/fixtures/localhost-ca.pem");
    for address in ["http://127.0.0.1:8027", "https://company.example/"] {
        let result =
            RemoteClient::discover_fixture(address, fixture.credentials.clone(), pem).await;
        assert!(matches!(
            result,
            Err(ApplicationError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }
    let client =
        RemoteClient::discover_fixture(&fixture.state.origin, fixture.credentials.clone(), pem)
            .await
            .unwrap();
    fixture.login(&client).await;
    assert!(matches!(
        client
            .execute(
                RemoteOperation::Me,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new()
            )
            .await
            .unwrap(),
        RemoteReply::Me(_)
    ));
}

#[tokio::test]
async fn forged_callback_does_not_consume_the_login_and_identity_mixup_is_rejected() {
    let fixture = Fixture::new().await;
    let client = fixture.client().await;
    let login = client.begin_login().await.unwrap();
    let url = Url::parse(login.authorization_url()).unwrap();
    let query: HashMap<_, _> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let redirect = query["redirect_uri"].clone();
    let complete = tokio::spawn(login.complete());
    let browser = reqwest::Client::new();
    let response = browser
        .get(&redirect)
        .query(&[("state", "forged"), ("code", "bad")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = browser
        .get(&redirect)
        .query(&[
            ("state", query["state"].as_str()),
            ("code", "bad"),
            ("iss", "https://other.example"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!complete.is_finished());
    assert_eq!(
        browser.get(url).send().await.unwrap().status(),
        StatusCode::OK
    );
    complete.await.unwrap().unwrap();
}

#[tokio::test]
async fn cancelling_browser_login_closes_callback_and_never_stores_credentials() {
    let fixture = Fixture::new().await;
    let client = fixture.client().await;
    let login = client.begin_login().await.unwrap();
    let cancel = login.cancellation();
    cancel.cancel();
    assert_eq!(
        login.complete().await.unwrap_err().code,
        ErrorCode::Cancelled
    );
    assert!(fixture.credentials.values.lock().await.is_empty());
}

#[tokio::test]
async fn concurrent_requests_rotate_a_refresh_token_once_and_cancellation_does_not_lose_it() {
    let fixture = Fixture::new().await;
    let client = fixture.client().await;
    fixture.login(&client).await;
    client.expire_access("access-1").await;
    fixture.state.delay.store(100, Ordering::SeqCst);
    let abandoned = tokio::spawn({
        let client = client.clone();
        async move { client.access_token().await }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    abandoned.abort();
    let mut requests = Vec::new();
    for _ in 0..8 {
        requests.push(tokio::spawn({
            let client = client.clone();
            async move { client.access_token().await.unwrap() }
        }));
    }
    for request in requests {
        assert_eq!(request.await.unwrap(), "access-2");
    }
    assert_eq!(fixture.state.refreshes.load(Ordering::SeqCst), 1);
    let saved = fixture
        .credentials
        .values
        .lock()
        .await
        .values()
        .next()
        .unwrap()
        .clone();
    assert!(saved.contains("refresh-2"));
}

#[tokio::test]
async fn abandoned_refresh_retains_the_profile_process_lock_until_rotated_credentials_are_saved() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(1))).await;
    let root = tempfile::tempdir().unwrap();
    let profiles = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
    let lease = profiles
        .create("work", &fixture.state.origin)
        .await
        .unwrap();
    let weak_lease = Arc::downgrade(&lease);
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client).unwrap().credentials = lease.credentials(fixture.credentials.clone());
    drop(lease);
    fixture.login(&client).await;
    client.expire_access("access-1").await;
    fixture.credentials.save_delay.store(250, Ordering::SeqCst);
    let abandoned = tokio::spawn({
        let client = client.clone();
        async move { client.access_token().await }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.credentials.saves.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    abandoned.abort();
    assert!(abandoned.await.unwrap_err().is_cancelled());
    drop(client);
    assert!(weak_lease.upgrade().is_some());
    let next = profiles.open("work");
    tokio::pin!(next);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut next)
            .await
            .is_err()
    );
    let _next = tokio::time::timeout(Duration::from_secs(5), next)
        .await
        .unwrap()
        .unwrap();
    assert!(weak_lease.upgrade().is_none());
    assert!(
        fixture
            .credentials
            .values
            .lock()
            .await
            .values()
            .next()
            .unwrap()
            .contains("refresh-2")
    );
}

#[tokio::test]
async fn logout_wins_over_inflight_refresh_and_removes_persistent_credentials() {
    let fixture = Fixture::new().await;
    let client = fixture.client().await;
    fixture.login(&client).await;
    client.expire_access("access-1").await;
    fixture.state.delay.store(100, Ordering::SeqCst);
    let refresh = tokio::spawn({
        let client = client.clone();
        async move { client.access_token().await }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    client.logout().await.unwrap();
    let _ = refresh.await;
    assert!(fixture.credentials.values.lock().await.is_empty());
    assert_eq!(
        client.access_token().await.unwrap_err().code,
        ErrorCode::Cancelled
    );
    assert_eq!(
        fixture
            .client()
            .await
            .access_token()
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unauthenticated
    );
}

#[tokio::test]
async fn failed_credential_persistence_does_not_report_successful_login() {
    let fixture = Fixture::new().await;
    let client = fixture.client().await;
    fixture.credentials.fail.store(true, Ordering::SeqCst);
    let login = client.begin_login().await.unwrap();
    let url = login.authorization_url().to_owned();
    let complete = tokio::spawn(login.complete());
    reqwest::get(url).await.unwrap();
    assert_eq!(
        complete.await.unwrap().unwrap_err().code,
        ErrorCode::Internal
    );
    assert_eq!(
        client.access_token().await.unwrap_err().code,
        ErrorCode::Unauthenticated
    );
}

#[tokio::test]
async fn real_tcp_sse_reconnects_with_the_last_complete_cursor_and_stops_after_revocation() {
    let fixture = Fixture::new().await;
    let client = fixture.client().await;
    fixture.login(&client).await;
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let subscription = client
        .subscribe("test".into(), move |event| sender.send(event).is_ok())
        .unwrap();
    let mut changes = Vec::new();
    while changes.len() < 2 {
        if let RemoteEvent::Change { event } =
            tokio::time::timeout(Duration::from_secs(5), receiver.recv())
                .await
                .unwrap()
                .unwrap()
        {
            changes.push(event);
        }
    }
    assert_eq!(changes[0].change, Change::ResyncRequired);
    assert_eq!(
        fixture.state.event_connections.lock().await.as_slice(),
        &[None, Some("epoch:1".into())]
    );
    fixture.state.denied.store(true, Ordering::SeqCst);
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        if matches!(
            event,
            RemoteEvent::Connection {
                state: ConnectionState::AccessDenied,
                ..
            }
        ) {
            break;
        }
    }
    subscription.close().await;
}

#[tokio::test]
async fn https_sse_detects_a_blackholed_connection_and_recovers_the_last_cursor() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let client = fixture.client().await;
    fixture.login(&client).await;
    fixture.state.stalled_events.store(true, Ordering::SeqCst);
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let subscription = client
        .subscribe("test".into(), move |event| sender.send(event).is_ok())
        .unwrap();
    let mut changes = Vec::new();
    let mut reconnecting = false;
    while changes.len() < 2 {
        let event = tokio::time::timeout(Duration::from_secs(55), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        match event {
            RemoteEvent::Change { event } => {
                changes.push(event);
                fixture.state.stalled_events.store(false, Ordering::SeqCst);
            }
            RemoteEvent::Connection {
                state: ConnectionState::Reconnecting,
                ..
            } => reconnecting = true,
            _ => {}
        }
    }
    assert!(reconnecting);
    assert_eq!(changes[0].cursor, "epoch:1");
    assert_eq!(changes[1].cursor, "epoch:2");
    assert_eq!(
        fixture.state.event_connections.lock().await.as_slice(),
        &[None, Some("epoch:1".into())]
    );
    subscription.close().await;
}

#[tokio::test]
async fn cancelled_requests_have_a_request_id_and_conservative_mutation_outcome() {
    let fixture = Fixture::new().await;
    let client = fixture.client().await;
    fixture.login(&client).await;
    fixture.state.delay.store(500, Ordering::SeqCst);
    let cancel = CancellationToken::new();
    let id = uuid::Uuid::new_v4().to_string();
    let request = tokio::spawn({
        let client = client.clone();
        let id = id.clone();
        let cancel = cancel.clone();
        async move {
            client
                .execute(RemoteOperation::Provision, &id, cancel)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    cancel.cancel();
    let error = request.await.unwrap().unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert_eq!(error.request_id.as_deref(), Some(id.as_str()));
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
}

#[tokio::test]
async fn production_addresses_never_allow_http_or_credentials_and_path_segments_cannot_escape() {
    let fixture = Fixture::new().await;
    for address in [
        fixture.state.origin.as_str(),
        "https://user:pass@example.com",
        "https://example.com/#fragment",
        "https://example.com/api",
    ] {
        assert!(
            RemoteClient::discover(address, fixture.credentials.clone(), false)
                .await
                .is_err()
        );
    }
    let client = fixture.client().await;
    for id in ["..", "../personal", "session/other", "session\\other", ""] {
        assert!(client.endpoint(&["vaults", id]).is_err());
    }
    assert!(
        client
            .endpoint(&["sessions", "a?b#c"])
            .unwrap()
            .as_str()
            .ends_with("a%3Fb%23c")
    );
}

struct TlsProxy {
    origin: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for TlsProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl TlsProxy {
    async fn start(upstream: std::net::SocketAddr, rtt: Duration) -> Self {
        use tokio_rustls::rustls::{
            self,
            pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
        };
        let cert =
            CertificateDer::from_pem_slice(include_bytes!("../tests/fixtures/localhost.pem"))
                .unwrap();
        let key = PrivateKeyDer::from_pem_slice(include_bytes!("../tests/fixtures/localhost.key"))
            .unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("https://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let (stream, _) = connection.unwrap();
                        let acceptor = acceptor.clone();
                        connections.spawn(async move {
                            let Ok(tls) = acceptor.accept(stream).await else { return; };
                            let Ok(server) = tokio::net::TcpStream::connect(upstream).await else { return; };
                            let (incoming, outgoing) = tokio::io::split(tls);
                            let (server_in, server_out) = server.into_split();
                            tokio::select! {
                                _ = delayed_copy(incoming, server_out, rtt/2) => {},
                                _ = delayed_copy(server_in, outgoing, rtt/2) => {},
                            }
                        });
                    },
                    _ = connections.join_next(), if !connections.is_empty() => {},
                }
            }
        });
        Self { origin, task }
    }
}
async fn delayed_copy(
    mut input: impl tokio::io::AsyncRead + Unpin,
    mut output: impl tokio::io::AsyncWrite + Unpin,
    delay: Duration,
) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buffer = vec![0_u8; 65536];
    loop {
        let length = input.read(&mut buffer).await?;
        if length == 0 {
            return Ok(());
        }
        tokio::time::sleep(delay).await;
        output.write_all(&buffer[..length]).await?;
        output.flush().await?;
    }
}

#[tokio::test]
async fn trusted_https_proxy_supports_login_requests_and_reconnect_at_realistic_round_trips() {
    for milliseconds in [50, 150, 500] {
        let fixture = Fixture::with_tls(Some(Duration::from_millis(milliseconds))).await;
        assert!(
            RemoteClient::discover(&fixture.state.origin, fixture.credentials.clone(), false)
                .await
                .is_err()
        );
        let public = fixture
            .browser()
            .get(format!("{}/api/v1/capabilities", fixture.state.origin))
            .send()
            .await;
        assert!(public.is_ok(), "TLS test proxy: {public:?}");
        let client = fixture.client().await;
        fixture.login(&client).await;
        let started = std::time::Instant::now();
        let result = client
            .execute(
                RemoteOperation::Me,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(matches!(result, RemoteReply::Me(_)));
        assert!(started.elapsed() >= Duration::from_millis(milliseconds));
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let subscription = client
            .subscribe("test".into(), move |event| sender.send(event).is_ok())
            .unwrap();
        let mut changes = 0;
        while changes < 2 {
            if matches!(
                tokio::time::timeout(Duration::from_secs(10), receiver.recv())
                    .await
                    .unwrap()
                    .unwrap(),
                RemoteEvent::Change { .. }
            ) {
                changes += 1;
            }
        }
        assert_eq!(
            fixture.state.event_connections.lock().await.as_slice(),
            &[None, Some("epoch:1".into())]
        );
        subscription.close().await;
    }
}

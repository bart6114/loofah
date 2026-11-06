use super::*;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect},
    routing::{get, post, put},
};
use hypr_loofah_client::CredentialFuture;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::atomic::AtomicUsize;
use tauri::Url;
use tauri_plugin_http::reqwest;

struct NoCredentials;
impl CredentialStore for NoCredentials {
    fn load<'a>(&'a self, _: &'a str) -> CredentialFuture<'a, Option<String>> {
        Box::pin(async { Ok(None) })
    }
    fn save<'a>(&'a self, _: &'a str, _: Option<&'a str>) -> CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}
struct Api {
    origin: String,
    current: Mutex<Arc<Vec<u8>>>,
    leases: Mutex<HashMap<String, (RecordingLease, Arc<Vec<u8>>)>>,
    opens: AtomicUsize,
    bytes: AtomicUsize,
}
struct Fixture {
    state: RemoteState,
    api: Arc<Api>,
    connection: String,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.state.shutdown();
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let api = Arc::new(Api {
            origin: origin.clone(),
            current: Mutex::new(Arc::new(
                (0..3 * RECORDING_CHUNK_LIMIT + 31)
                    .map(|index| (index % 251) as u8)
                    .collect(),
            )),
            leases: Default::default(),
            opens: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
        });
        let discovery = json!({"issuer":origin,"authorization_endpoint":format!("{origin}/authorize"),"token_endpoint":format!("{origin}/token"),"code_challenge_methods_supported":["S256"]});
        let config = json!({"issuer":origin,"authorization_endpoint":format!("{origin}/authorize"),"token_endpoint":format!("{origin}/token"),"client_id":"desktop","audience":"api","scopes":["loofah:access"]});
        let app = Router::new()
            .route("/.well-known/openid-configuration",get(move||{let value=discovery.clone(); async move {Json(value)}}))
            .route("/api/v1/auth/config",get(move||{let value=config.clone(); async move {Json(value)}}))
            .route("/api/v1/capabilities",get(||async{Json(json!({"api_major":1,"application_version":"test","features":["recording_leases","recording_waveforms"]}))}))
            .route("/authorize",get(authorize))
            .route("/token",post(||async{Json(json!({"token_type":"Bearer","access_token":"access-for-media","refresh_token":"refresh-for-media","expires_in":3600}))}))
            .route("/api/v1/me",get(||async{Json(json!({"member":{"id":"member","name":"Member"},"personal_vault_id":"vault"}))}))
            .route("/api/v1/vaults/vault/sessions/{session}/audio/leases/{id}",put(open))
            .route("/api/v1/vaults/vault/audio/leases/{id}",get(read).head(head).delete(close))
            .with_state(api.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = RemoteClient::discover(&origin, Arc::new(NoCredentials), true)
            .await
            .unwrap();
        let login = client.begin_login().await.unwrap();
        let auth = login.authorization_url().to_owned();
        let completion = tokio::spawn(login.complete());
        assert!(reqwest::get(auth).await.unwrap().status().is_success());
        completion.await.unwrap().unwrap();
        let state = RemoteState::default();
        let lifetime = state.window_lifetime("first");
        let connection = state
            .register(client, "first", &lifetime)
            .unwrap()
            .connection_id;
        state
            .registry
            .lock()
            .unwrap()
            .get_mut(&connection)
            .unwrap()
            .authenticated = true;
        Self {
            state,
            api,
            connection,
            server,
        }
    }
    async fn entry(&self) -> Arc<MediaEntry> {
        let id = uuid::Uuid::new_v4().to_string();
        let entry = self
            .state
            .media
            .reserve(
                &self.state,
                &self.connection,
                "first",
                id,
                "vault".into(),
                "note".into(),
            )
            .unwrap();
        entry.open().await.unwrap().unwrap();
        entry
    }
    async fn request(
        &self,
        owner: &str,
        id: &str,
        method: &str,
        range: Option<&str>,
    ) -> Response<Vec<u8>> {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("loofah-media://localhost/{id}"));
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        response(
            &self.state,
            owner,
            "tauri://localhost",
            request.body(Vec::new()).unwrap(),
        )
        .await
    }
}
async fn authorize(
    State(api): State<Arc<Api>>,
    Query(query): Query<HashMap<String, String>>,
) -> Redirect {
    let mut url = Url::parse(&query["redirect_uri"]).unwrap();
    url.query_pairs_mut().extend_pairs([
        ("code", "media-code"),
        ("state", query["state"].as_str()),
        ("iss", api.origin.as_str()),
    ]);
    Redirect::to(url.as_str())
}
fn authorized(headers: &tauri::http::HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .is_some_and(|value| value == "Bearer access-for-media")
}
async fn open(
    State(api): State<Arc<Api>>,
    Path((session, id)): Path<(String, String)>,
    headers: tauri::http::HeaderMap,
) -> axum::response::Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if session == "absent" {
        return Json(None::<RecordingLease>).into_response();
    }
    api.opens.fetch_add(1, Ordering::SeqCst);
    let bytes = api.current.lock().unwrap().clone();
    let digest: String = Sha256::digest(bytes.as_slice())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let lease = RecordingLease {
        id: id.clone(),
        vault_id: "vault".into(),
        session_id: session,
        idle_timeout_secs: 600,
        metadata: FileMetadata {
            filename: "audio.wav".into(),
            content_type: "audio/wav".into(),
            size_bytes: bytes.len() as u64,
            sha256: digest,
        },
    };
    let mut leases = api.leases.lock().unwrap();
    let (lease, _) = leases.entry(id).or_insert((lease, bytes));
    Json(lease.clone()).into_response()
}
async fn head(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    headers: tauri::http::HeaderMap,
) -> axum::response::Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let leases = api.leases.lock().unwrap();
    let Some((lease, _)) = leases.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    axum::response::Response::builder()
        .header("etag", format!("\"sha256:{}\"", lease.metadata.sha256))
        .header("content-type", "audio/wav")
        .header("content-length", lease.metadata.size_bytes)
        .body(axum::body::Body::empty())
        .unwrap()
}
async fn read(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    headers: tauri::http::HeaderMap,
) -> axum::response::Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let leases = api.leases.lock().unwrap();
    let Some((lease, bytes)) = leases.get(&id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(failure(ErrorCode::NotFound, "Recording missing")),
        )
            .into_response();
    };
    let etag = format!("\"sha256:{}\"", lease.metadata.sha256);
    if headers
        .get("if-match")
        .is_some_and(|value| value != etag.as_str())
    {
        return StatusCode::PRECONDITION_FAILED.into_response();
    }
    let value = headers
        .get("range")
        .unwrap()
        .to_str()
        .unwrap()
        .strip_prefix("bytes=")
        .unwrap();
    let (start, end) = value.split_once('-').unwrap();
    let start = start.parse::<usize>().unwrap();
    let end = end.parse::<usize>().unwrap();
    let body = bytes[start..=end].to_vec();
    api.bytes.fetch_add(body.len(), Ordering::SeqCst);
    axum::response::Response::builder()
        .status(206)
        .header("etag", etag)
        .header("content-type", "audio/wav")
        .header("content-length", body.len())
        .header(
            "content-range",
            format!("bytes {start}-{end}/{}", bytes.len()),
        )
        .body(axum::body::Body::from(body))
        .unwrap()
}
async fn close(State(api): State<Arc<Api>>, Path(id): Path<String>) -> StatusCode {
    api.leases.lock().unwrap().remove(&id);
    StatusCode::NO_CONTENT
}

#[test]
fn native_media_ranges_are_single_bounded_and_handle_suffixes_without_overflow() {
    assert_eq!(byte_range(None, 10), Ok((0, 10, false)));
    assert_eq!(byte_range(Some("bytes=2-4"), 10), Ok((2, 3, true)));
    assert_eq!(byte_range(Some("bytes=-2"), 10), Ok((8, 2, true)));
    assert_eq!(byte_range(Some("bytes=8-"), 10), Ok((8, 2, true)));
    assert_eq!(
        byte_range(None, u64::MAX),
        Ok((0, RECORDING_CHUNK_LIMIT, true))
    );
    for value in [
        "bytes=-0",
        "bytes=10-",
        "bytes=9-8",
        "bytes=0-1,2-3",
        "bytes=18446744073709551616-",
        "items=0-1",
    ] {
        assert!(byte_range(Some(value), 10).is_err());
    }
    assert!(byte_range(None, 0).is_err());
}

#[tokio::test]
async fn native_media_is_window_scoped_streams_bounded_ranges_and_reopens_only_the_same_generation()
{
    let fixture = Fixture::new().await;
    let entry = fixture.entry().await;
    let original = fixture.api.current.lock().unwrap().clone();
    let first = fixture.request("first", &entry.id, "GET", None).await;
    assert_eq!(first.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(first.body().as_slice(), &original[..RECORDING_CHUNK_LIMIT]);
    assert_eq!(
        first.headers()[header::CONTENT_LENGTH],
        RECORDING_CHUNK_LIMIT.to_string()
    );
    assert_eq!(
        fixture.api.bytes.load(Ordering::SeqCst),
        RECORDING_CHUNK_LIMIT
    );
    assert_eq!(
        fixture
            .request("second", &entry.id, "GET", Some("bytes=0-1"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let head = fixture.request("first", &entry.id, "HEAD", None).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert!(head.body().is_empty());
    assert_eq!(
        head.headers()[header::CONTENT_LENGTH],
        original.len().to_string()
    );
    fixture.api.leases.lock().unwrap().clear();
    let resumed = fixture
        .request("first", &entry.id, "GET", Some("bytes=2000000-2000015"))
        .await;
    assert_eq!(resumed.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resumed.body().as_slice(), &original[2_000_000..2_000_016]);
    fixture.api.leases.lock().unwrap().clear();
    *fixture.api.current.lock().unwrap() = Arc::new(b"Replaced recording".to_vec());
    let changed = fixture
        .request("first", &entry.id, "GET", Some("bytes=0-1"))
        .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert!(!changed.body().windows(8).any(|bytes| bytes == b"Replaced"));
    fixture.state.release_window("first");
    assert!(entry.cancel.is_cancelled());
    assert_eq!(
        fixture
            .request("first", &entry.id, "GET", Some("bytes=0-1"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn native_media_reservations_are_idempotent_cannot_cross_coordinates_and_keep_admission_until_readers_drop()
 {
    let fixture = Fixture::new().await;
    let entry = fixture.entry().await;
    let same = fixture
        .state
        .media
        .reserve(
            &fixture.state,
            &fixture.connection,
            "first",
            entry.id.clone(),
            "vault".into(),
            "note".into(),
        )
        .unwrap();
    assert!(Arc::ptr_eq(&entry, &same));
    assert!(
        fixture
            .state
            .media
            .reserve(
                &fixture.state,
                &fixture.connection,
                "first",
                entry.id.clone(),
                "vault".into(),
                "other".into()
            )
            .is_err()
    );
    assert_eq!(fixture.api.opens.load(Ordering::SeqCst), 1);
    let other = fixture.state.window_lifetime("second");
    let info = fixture
        .state
        .register(entry.client.clone(), "second", &other)
        .unwrap();
    assert!(
        fixture
            .state
            .media
            .reserve(
                &fixture.state,
                &info.connection_id,
                "second",
                entry.id.clone(),
                "vault".into(),
                "note".into()
            )
            .is_err()
    );
    fixture
        .state
        .media
        .remove("first", &fixture.connection, &entry.id);
    assert_eq!(fixture.state.media.slots.available_permits(), 63);
    assert!(entry.cancel.is_cancelled());
    drop(same);
    drop(entry);
    assert_eq!(fixture.state.media.slots.available_permits(), 64);
}

#[tokio::test]
async fn native_media_refuses_foreign_origins_and_invalid_ranges_without_fetching_bytes() {
    let fixture = Fixture::new().await;
    let entry = fixture.entry().await;
    let request = Request::builder()
        .uri(format!("loofah-media://localhost/{}", entry.id))
        .header(header::ORIGIN, "https://attacker.invalid")
        .body(Vec::new())
        .unwrap();
    assert_eq!(
        response(&fixture.state, "first", "tauri://localhost", request)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for range in ["bytes=0-1,2-3", "bytes=9999999999999999999-", "bytes=2-1"] {
        assert_eq!(
            fixture
                .request("first", &entry.id, "GET", Some(range))
                .await
                .status(),
            StatusCode::RANGE_NOT_SATISFIABLE
        );
    }
    assert_eq!(fixture.api.bytes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_media_close_before_admission_and_window_quota_never_leak_request_or_worker_slots() {
    let fixture = Fixture::new().await;
    let cancelled = uuid::Uuid::new_v4().to_string();
    fixture
        .state
        .registry
        .lock()
        .unwrap()
        .get_mut(&fixture.connection)
        .unwrap()
        .cancelled
        .insert(("first".into(), cancelled.clone()), Instant::now());
    let reserve = |id| {
        fixture.state.media.reserve(
            &fixture.state,
            &fixture.connection,
            "first",
            id,
            "vault".into(),
            "note".into(),
        )
    };
    let failure = reserve(cancelled).err().unwrap();
    assert_eq!(failure.code, ErrorCode::Cancelled);
    assert_eq!(
        failure.mutation_outcome,
        Some(hypr_loofah_protocol::MutationOutcome::NotCommitted)
    );
    assert_eq!(fixture.state.media.slots.available_permits(), 64);
    assert!(
        fixture.state.registry.lock().unwrap()[&fixture.connection]
            .requests
            .is_empty()
    );
    let entries: Vec<_> = (0..8)
        .map(|_| reserve(uuid::Uuid::new_v4().to_string()).unwrap())
        .collect();
    assert_eq!(
        reserve(uuid::Uuid::new_v4().to_string())
            .err()
            .unwrap()
            .code,
        ErrorCode::ResourceLimit
    );
    assert_eq!(fixture.state.media.slots.available_permits(), 56);
    assert_eq!(
        fixture.state.registry.lock().unwrap()[&fixture.connection]
            .requests
            .len(),
        8
    );
    fixture.state.release_window("first");
    assert_eq!(fixture.state.media.slots.available_permits(), 56);
    assert!(entries.iter().all(|entry| entry.cancel.is_cancelled()));
    drop(entries);
    assert_eq!(fixture.state.media.slots.available_permits(), 64);
    assert_eq!(fixture.api.opens.load(Ordering::SeqCst), 0);
}

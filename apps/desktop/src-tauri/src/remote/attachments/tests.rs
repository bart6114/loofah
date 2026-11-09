use super::*;
use axum::{
    Json, Router,
    extract::{Path as ApiPath, Query, State},
    response::{IntoResponse, Redirect},
    routing::{get, post, put},
};
use hypr_loofah_client::CredentialFuture;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::Url;
use tauri_plugin_http::reqwest;

struct Credentials;
impl CredentialStore for Credentials {
    fn load<'a>(&'a self, _: &'a str) -> CredentialFuture<'a, Option<String>> {
        Box::pin(async { Ok(None) })
    }
    fn save<'a>(&'a self, _: &'a str, _: Option<&'a str>) -> CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

struct Api {
    origin: String,
    bytes: Arc<Vec<u8>>,
    mode: AtomicUsize,
    opens: AtomicUsize,
    reads: AtomicUsize,
    closes: AtomicUsize,
    leases: Mutex<HashMap<String, AttachmentLease>>,
    started: tokio::sync::Notify,
}

struct Fixture {
    state: Arc<RemoteState>,
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
        let mut bytes = br##"<svg xmlns="http://www.w3.org/2000/svg" width="600" height="400"><rect width="600" height="400" fill="#168879"/><!--"##.to_vec();
        bytes.resize(3 * 1024 * 1024 + 17, b'a');
        bytes.extend_from_slice(b"--></svg>");
        let api = Arc::new(Api {
            origin: origin.clone(),
            bytes: Arc::new(bytes),
            mode: AtomicUsize::new(0),
            opens: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            closes: AtomicUsize::new(0),
            leases: Default::default(),
            started: Default::default(),
        });
        let discovery = json!({"issuer":origin,"authorization_endpoint":format!("{origin}/authorize"),"token_endpoint":format!("{origin}/token"),"code_challenge_methods_supported":["S256"]});
        let config = json!({"issuer":origin,"authorization_endpoint":format!("{origin}/authorize"),"token_endpoint":format!("{origin}/token"),"client_id":"desktop","audience":"api","scopes":["loofah:access"]});
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(move || { let value = discovery.clone(); async { Json(value) } }))
            .route("/api/v1/auth/config", get(move || { let value = config.clone(); async { Json(value) } }))
            .route("/api/v1/capabilities", get(|| async { Json(json!({"api_major":1,"application_version":"test","features":["attachment_leases"]})) }))
            .route("/authorize", get(authorize))
            .route("/token", post(|| async { Json(json!({"token_type":"Bearer","access_token":"attachment-access","refresh_token":"refresh","expires_in":3600})) }))
            .route("/api/v1/me", get(|| async { Json(json!({"member":{"id":"member","name":"Member"},"personal_vault_id":"vault"})) }))
            .route("/api/v1/vaults/vault/sessions/{session}/attachments/{attachment}/leases/{id}", put(open))
            .route("/api/v1/vaults/vault/attachments/leases/{id}", get(read).delete(close))
            .with_state(api.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = RemoteClient::discover(&origin, Arc::new(Credentials), true)
            .await
            .unwrap();
        let login = client.begin_login().await.unwrap();
        let url = login.authorization_url().to_owned();
        let complete = tokio::spawn(login.complete());
        assert!(reqwest::get(url).await.unwrap().status().is_success());
        complete.await.unwrap().unwrap();
        let state = Arc::new(RemoteState::default());
        let connection = state
            .register(client, "first", &state.window_lifetime("first"))
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

    fn entry(&self, name: &str) -> Arc<Entry> {
        self.state
            .attachments
            .reserve(
                &self.state,
                &self.connection,
                "first",
                &uuid::Uuid::new_v4().to_string(),
                target(name),
            )
            .unwrap()
    }

    async fn request(
        &self,
        owner: &str,
        id: &str,
        method: Method,
        origin: Option<&str>,
    ) -> Response<Vec<u8>> {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("loofah-media://localhost/attachments/{id}"));
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        super::super::media::response(
            &self.state,
            owner,
            "tauri://localhost",
            request.body(Vec::new()).unwrap(),
        )
        .await
    }
}

fn target(name: &str) -> AttachmentTarget {
    AttachmentTarget {
        vault_id: "vault".into(),
        session_id: "note".into(),
        attachment_id: name.into(),
    }
}

#[test]
fn native_attachment_copies_keep_file_bytes_and_apply_platform_download_metadata() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("download.txt");
    std::fs::write(&path, b"Preserve complete original bytes").unwrap();
    mark_download(&path).unwrap();
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/bin/xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains(";Loofah;")
        );
    }
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_os_string();
        stream.push(":Zone.Identifier");
        assert!(
            std::fs::read_to_string(PathBuf::from(stream))
                .unwrap()
                .contains("ZoneId=3")
        );
    }
    assert_eq!(
        std::fs::read(path).unwrap(),
        b"Preserve complete original bytes"
    );
}

async fn authorize(
    State(api): State<Arc<Api>>,
    Query(query): Query<HashMap<String, String>>,
) -> Redirect {
    let mut url = Url::parse(&query["redirect_uri"]).unwrap();
    url.query_pairs_mut().extend_pairs([
        ("code", "code"),
        ("state", query["state"].as_str()),
        ("iss", api.origin.as_str()),
    ]);
    Redirect::to(url.as_str())
}
fn authorized(headers: &tauri::http::HeaderMap) {
    assert_eq!(headers[header::AUTHORIZATION], "Bearer attachment-access");
}

async fn open(
    State(api): State<Arc<Api>>,
    ApiPath((session, attachment, id)): ApiPath<(String, String, String)>,
    headers: tauri::http::HeaderMap,
) -> axum::response::Response {
    authorized(&headers);
    assert_eq!(session, "note");
    api.opens.fetch_add(1, Ordering::SeqCst);
    let metadata = FileMetadata {
        filename: attachment.clone(),
        content_type: if attachment.ends_with(".svg") {
            "image/svg+xml"
        } else {
            "application/octet-stream"
        }
        .into(),
        size_bytes: if api.mode.load(Ordering::SeqCst) == 1 {
            PREVIEW_LIMIT + 1
        } else {
            api.bytes.len() as u64
        },
        sha256: Sha256::digest(api.bytes.as_slice())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    };
    let lease = AttachmentLease {
        id: id.clone(),
        vault_id: "vault".into(),
        session_id: session,
        attachment_id: attachment,
        metadata,
        idle_timeout_secs: 600,
    };
    api.leases.lock().unwrap().insert(id, lease.clone());
    Json(Some(lease)).into_response()
}

async fn read(
    State(api): State<Arc<Api>>,
    ApiPath(id): ApiPath<String>,
    headers: tauri::http::HeaderMap,
) -> axum::response::Response {
    authorized(&headers);
    let Some(lease) = api.leases.lock().unwrap().get(&id).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    api.reads.fetch_add(1, Ordering::SeqCst);
    let mode = api.mode.load(Ordering::SeqCst);
    let mut bytes = api.bytes.as_ref().clone();
    if mode == 2 {
        bytes[2 * 1024 * 1024] ^= 1;
    }
    let body = if mode == 3 {
        let signal = api.clone();
        let stream = futures_util::stream::unfold(false, move |sent| {
            let api = signal.clone();
            async move {
                if sent {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    return None;
                }
                api.started.notify_one();
                Some((
                    Ok::<_, std::io::Error>(api.bytes[..1024 * 1024 + 17].to_vec()),
                    true,
                ))
            }
        });
        axum::body::Body::from_stream(stream)
    } else {
        axum::body::Body::from(bytes)
    };
    Response::builder()
        .header(header::CONTENT_TYPE, lease.metadata.content_type)
        .header(header::CONTENT_LENGTH, lease.metadata.size_bytes)
        .header(
            header::ETAG,
            format!("\"sha256:{}\"", lease.metadata.sha256),
        )
        .body(body)
        .unwrap()
}

async fn close(
    State(api): State<Arc<Api>>,
    ApiPath(id): ApiPath<String>,
    headers: tauri::http::HeaderMap,
) -> StatusCode {
    authorized(&headers);
    api.closes.fetch_add(1, Ordering::SeqCst);
    api.leases.lock().unwrap().remove(&id);
    StatusCode::NO_CONTENT
}

#[tokio::test]
async fn native_attachment_protocol_accepts_tauri_encoded_paths_without_general_path_decoding() {
    let fixture = Fixture::new().await;
    let entry = fixture.entry("a.svg");
    for separator in ["%2F", "%2f"] {
        let response = super::super::media::response(
            &fixture.state,
            "first",
            "http://localhost:1422",
            Request::builder()
                .uri(format!(
                    "loofah-media://localhost/attachments{separator}{}",
                    entry.id
                ))
                .header(header::ORIGIN, "http://localhost:1422")
                .body(Vec::new())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), fixture.api.bytes.as_ref());
    }
    let opened = fixture.api.opens.load(Ordering::SeqCst);
    for suffix in [
        format!("%252F{}", entry.id),
        format!("%2F{}%2F..", entry.id),
    ] {
        let response = super::super::media::response(
            &fixture.state,
            "first",
            "http://localhost:1422",
            Request::builder()
                .uri(format!("loofah-media://localhost/attachments{suffix}"))
                .body(Vec::new())
                .unwrap(),
        )
        .await;
        assert!(!response.status().is_success());
    }
    assert_eq!(fixture.api.opens.load(Ordering::SeqCst), opened);
}

#[tokio::test]
async fn native_attachment_handles_are_cheap_scoped_idempotent_and_separate_from_request_admission()
{
    let fixture = Fixture::new().await;
    let entry = fixture.entry("a.svg");
    assert_eq!(fixture.api.opens.load(Ordering::SeqCst), 0);
    assert!(
        fixture.state.registry.lock().unwrap()[&fixture.connection]
            .requests
            .is_empty()
    );
    assert!(Arc::ptr_eq(
        &entry,
        &fixture
            .state
            .attachments
            .reserve(
                &fixture.state,
                &fixture.connection,
                "first",
                &entry.id,
                target("a.svg")
            )
            .unwrap()
    ));
    assert_eq!(
        fixture
            .state
            .attachments
            .reserve(
                &fixture.state,
                &fixture.connection,
                "first",
                &entry.id,
                target("other.svg")
            )
            .err()
            .unwrap()
            .code,
        ErrorCode::Conflict
    );
    fixture
        .state
        .registry
        .lock()
        .unwrap()
        .get_mut(&fixture.connection)
        .unwrap()
        .leases
        .insert("second".into(), 1);
    assert!(
        fixture
            .state
            .attachments
            .get(&fixture.state, "second", &entry.id)
            .is_err()
    );
    fixture
        .state
        .attachments
        .close(&fixture.state, "second", &fixture.connection, &entry.id);
    assert!(
        fixture
            .state
            .attachments
            .get(&fixture.state, "first", &entry.id)
            .is_ok()
    );
    for _ in 1..128 {
        fixture.entry("a.svg");
    }
    assert_eq!(
        fixture
            .state
            .attachments
            .reserve(
                &fixture.state,
                &fixture.connection,
                "first",
                &uuid::Uuid::new_v4().to_string(),
                target("a.svg")
            )
            .err()
            .unwrap()
            .code,
        ErrorCode::ResourceLimit
    );
    assert_eq!(fixture.api.opens.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_attachment_preview_delivers_complete_svg_bytes_and_closes_each_backend_lease() {
    let fixture = Fixture::new().await;
    let entry = fixture.entry("a.svg");
    let response = fixture
        .request("first", &entry.id, Method::GET, Some("tauri://localhost"))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
    assert!(
        response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(response.body(), fixture.api.bytes.as_ref());
    assert!(response.body().len() > 1024 * 1024);
    assert!(fixture.api.leases.lock().unwrap().is_empty());
    let head = fixture
        .request("first", &entry.id, Method::HEAD, None)
        .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert!(head.body().is_empty());
    assert_eq!(fixture.api.reads.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.api.closes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn native_attachment_preview_rejects_foreign_origins_owners_oversize_and_nonimage_sources() {
    let fixture = Fixture::new().await;
    let entry = fixture.entry("a.svg");
    assert_eq!(
        fixture
            .request(
                "first",
                &entry.id,
                Method::GET,
                Some("https://foreign.example")
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request("second", &entry.id, Method::GET, None)
            .await
            .status(),
        StatusCode::GONE
    );
    assert_eq!(fixture.api.opens.load(Ordering::SeqCst), 0);
    fixture.api.mode.store(1, Ordering::SeqCst);
    assert_eq!(
        fixture
            .request("first", &entry.id, Method::GET, None)
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    fixture.api.mode.store(0, Ordering::SeqCst);
    let file = fixture.entry("a.bin");
    assert_eq!(
        fixture
            .request("first", &file.id, Method::GET, None)
            .await
            .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(fixture.api.reads.load(Ordering::SeqCst), 0);
    assert!(fixture.api.leases.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_attachment_preview_refuses_unverified_bytes() {
    let fixture = Fixture::new().await;
    fixture.api.mode.store(2, Ordering::SeqCst);
    let entry = fixture.entry("a.svg");
    assert_eq!(
        fixture
            .request("first", &entry.id, Method::GET, None)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert!(fixture.api.leases.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_attachment_early_close_and_window_release_refuse_late_or_waiting_work() {
    let fixture = Fixture::new().await;
    let id = uuid::Uuid::new_v4().to_string();
    fixture
        .state
        .attachments
        .close(&fixture.state, "first", &fixture.connection, &id);
    assert_eq!(
        fixture
            .state
            .attachments
            .reserve(
                &fixture.state,
                &fixture.connection,
                "first",
                &id,
                target("a.svg")
            )
            .err()
            .unwrap()
            .code,
        ErrorCode::Cancelled
    );
    let entry = fixture.entry("a.svg");
    let held = fixture
        .state
        .attachments
        .transfers
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    let state = fixture.state.clone();
    let id = entry.id.clone();
    let request = tokio::spawn(async move {
        preview_response(
            &state,
            "first",
            Request::builder()
                .uri(format!("loofah-media://localhost/attachments/{id}"))
                .body(Vec::new())
                .unwrap(),
        )
        .await
    });
    tokio::task::yield_now().await;
    fixture.state.release_window("first");
    assert_eq!(
        request.await.unwrap().err().unwrap().code,
        ErrorCode::Cancelled
    );
    assert_eq!(fixture.api.opens.load(Ordering::SeqCst), 0);
    drop(held);
}

#[tokio::test]
async fn native_attachment_copies_preserve_existing_files_reuse_local_edits_and_save_fresh_originals()
 {
    let fixture = Fixture::new().await;
    let entry = fixture.entry("a.svg");
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.svg"), b"Preserve original user file").unwrap();
    let first = entry
        .local_copy(
            root.path().into(),
            fixture.state.attachments.transfers.clone(),
            fixture.state.attachments.copies.clone(),
            CancellationToken::new(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(first.path.file_name().unwrap(), "a (1).svg");
    assert_eq!(std::fs::read(&first.path).unwrap(), *fixture.api.bytes);
    std::fs::write(&first.path, b"User edited their downloaded copy").unwrap();
    let again = entry
        .local_copy(
            root.path().into(),
            fixture.state.attachments.transfers.clone(),
            fixture.state.attachments.copies.clone(),
            CancellationToken::new(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(again.path, first.path);
    assert_eq!(fixture.api.reads.load(Ordering::SeqCst), 1);
    let fresh = entry
        .local_copy(
            root.path().into(),
            fixture.state.attachments.transfers.clone(),
            fixture.state.attachments.copies.clone(),
            CancellationToken::new(),
            true,
        )
        .await
        .unwrap();
    assert_eq!(fresh.path.file_name().unwrap(), "a (2).svg");
    assert_eq!(std::fs::read(&fresh.path).unwrap(), *fixture.api.bytes);
    assert_eq!(
        std::fs::read(&first.path).unwrap(),
        b"User edited their downloaded copy"
    );
    assert_eq!(
        std::fs::read(root.path().join("a.svg")).unwrap(),
        b"Preserve original user file"
    );
    assert!(fixture.api.leases.lock().unwrap().is_empty());
    let alias = fixture
        .entry("CON.txt")
        .local_copy(
            root.path().into(),
            fixture.state.attachments.transfers.clone(),
            fixture.state.attachments.copies.clone(),
            CancellationToken::new(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(alias.path.file_name().unwrap(), "_CON.txt");
    assert_eq!(alias.metadata.filename, "CON.txt");
}

#[tokio::test]
async fn cancelled_native_copies_remove_only_private_staging_and_keep_io_admission_until_cleanup() {
    let fixture = Fixture::new().await;
    fixture.api.mode.store(3, Ordering::SeqCst);
    let entry = fixture.entry("a.svg");
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("canary"), b"Keep me").unwrap();
    let worker = entry.clone();
    let directory = root.path().to_path_buf();
    let transfers = fixture.state.attachments.transfers.clone();
    let copies = fixture.state.attachments.copies.clone();
    let request = tokio::spawn(async move {
        worker
            .local_copy(
                directory,
                transfers,
                copies,
                CancellationToken::new(),
                false,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), fixture.api.started.notified())
        .await
        .unwrap();
    fixture
        .state
        .attachments
        .close(&fixture.state, "first", &fixture.connection, &entry.id);
    assert_eq!(
        request.await.unwrap().err().unwrap().code,
        ErrorCode::Cancelled
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.attachments.transfers.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    assert_eq!(
        std::fs::read(root.path().join("canary")).unwrap(),
        b"Keep me"
    );
    assert!(fixture.api.leases.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_attachment_copy_work_leaves_a_transfer_available_for_previews() {
    let fixture = Fixture::new().await;
    fixture.api.mode.store(3, Ordering::SeqCst);
    let copy = fixture.entry("copy.svg");
    let preview = fixture.entry("preview.svg");
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().to_path_buf();
    let transfers = fixture.state.attachments.transfers.clone();
    let copies = fixture.state.attachments.copies.clone();
    let cancel = CancellationToken::new();
    let stopped = cancel.clone();
    let worker = tokio::spawn(async move {
        copy.local_copy(directory, transfers, copies, stopped, false)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), fixture.api.started.notified())
        .await
        .unwrap();
    assert_eq!(fixture.state.attachments.transfers.available_permits(), 1);
    assert_eq!(fixture.state.attachments.copies.available_permits(), 0);
    fixture.api.mode.store(0, Ordering::SeqCst);
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        fixture.request("first", &preview.id, Method::GET, None),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.body(), fixture.api.bytes.as_ref());
    cancel.cancel();
    assert_eq!(
        worker.await.unwrap().err().unwrap().code,
        ErrorCode::Cancelled
    );
    assert_eq!(fixture.state.attachments.transfers.available_permits(), 2);
    assert_eq!(fixture.state.attachments.copies.available_permits(), 1);
}

#[tokio::test]
async fn native_attachment_logout_closes_known_leases_before_credentials_are_stopped() {
    let fixture = Fixture::new().await;
    fixture.api.mode.store(3, Ordering::SeqCst);
    let entry = fixture.entry("a.svg");
    let state = fixture.state.clone();
    let id = entry.id.clone();
    let request = tokio::spawn(async move {
        preview_response(
            &state,
            "first",
            Request::builder()
                .uri(format!("loofah-media://localhost/attachments/{id}"))
                .body(Vec::new())
                .unwrap(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), fixture.api.started.notified())
        .await
        .unwrap();
    fixture
        .state
        .attachments
        .cancel_connection(&fixture.connection)
        .await;
    assert!(!entry.client.is_closed());
    assert!(fixture.api.leases.lock().unwrap().is_empty());
    assert_eq!(
        request.await.unwrap().err().unwrap().code,
        ErrorCode::Cancelled
    );
    assert!(
        fixture
            .state
            .attachments
            .get(&fixture.state, "first", &entry.id)
            .is_err()
    );
}

#[tokio::test]
async fn export_attachment_is_fresh_bounded_scoped_and_retires_its_resource() {
    let fixture = Fixture::new().await;
    let root = tempfile::tempdir().unwrap();
    let target = AttachmentTarget {
        vault_id: "vault".into(),
        session_id: "note".into(),
        attachment_id: "a.svg".into(),
    };
    let result = export_attachment(
        &fixture.state,
        &fixture.connection,
        "first",
        target.clone(),
        root.path().into(),
        CancellationToken::new(),
        1,
    )
    .await;
    assert_eq!(result.err().unwrap().code, ErrorCode::ResourceLimit);
    assert_eq!(fixture.api.reads.load(Ordering::SeqCst), 0);
    assert!(fixture.state.attachments.entries.lock().unwrap().is_empty());
    assert!(fixture.api.leases.lock().unwrap().is_empty());
    let copy = export_attachment(
        &fixture.state,
        &fixture.connection,
        "first",
        target,
        root.path().into(),
        CancellationToken::new(),
        32 * 1024 * 1024,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&copy.path).unwrap(), *fixture.api.bytes);
    assert_eq!(fixture.state.attachments.entries.lock().unwrap().len(), 1);
    fixture.state.release_window("first");
    assert!(copy.check_current(&CancellationToken::new()).is_err());
    drop(copy);
    assert!(fixture.state.attachments.entries.lock().unwrap().is_empty());
}

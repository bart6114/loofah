use std::sync::Arc;

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use hypr_loofah_protocol::*;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::{
    auth::now,
    config::{Config, OidcConfig},
    routes,
    service::Service,
};

struct Fixture {
    _root: tempfile::TempDir,
    issuer: String,
    issuer_task: tokio::task::JoinHandle<()>,
    service: Arc<Service>,
    app: Router,
}

impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let metadata = json!({"issuer":issuer,"jwks_uri":format!("{issuer}/keys"),"authorization_endpoint":format!("{issuer}/authorize"),"token_endpoint":format!("{issuer}/token"),"code_challenge_methods_supported":["S256"]});
        let keys: Value =
            serde_json::from_str(include_str!("../tests/fixtures/jwks.json")).unwrap();
        let oauth = Arc::new(MockOAuth {
            issuer: issuer.clone(),
            codes: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        });
        let provider = Router::new()
            .route("/authorize", axum::routing::get(mock_authorize))
            .route("/token", axum::routing::post(mock_token))
            .route(
                "/.well-known/openid-configuration",
                axum::routing::get(move || async move { Json(metadata) }),
            )
            .route(
                "/keys",
                axum::routing::get(move || async move { Json(keys) }),
            )
            .with_state(oauth);
        let issuer_task = tokio::spawn(async move {
            axum::serve(listener, provider).await.unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            storage_root: root.path().join("storage"),
            cache_root: root.path().join("cache"),
            public_url: "https://loofah.company.example".parse().unwrap(),
            listen: "127.0.0.1:0".parse().unwrap(),
            oidc: OidcConfig {
                issuer: issuer.clone(),
                audience: "loofah-api".into(),
                client_id: "loofah-desktop".into(),
                required_scope: "loofah:access".into(),
                login_scopes: vec![],
                access_token_types: vec!["at+jwt".into()],
            },
            allow_insecure_loopback: true,
        };
        let service = Service::open(config).await.unwrap();
        let app = routes::router(service.clone());
        Self {
            _root: root,
            issuer,
            issuer_task,
            service,
            app,
        }
    }

    fn claims(&self, subject: &str) -> Value {
        json!({"iss":self.issuer,"sub":subject,"aud":"loofah-api","exp":now()+3600,"iat":now(),"scope":"openid profile loofah:access","name":subject})
    }

    fn token(&self, subject: &str) -> String {
        sign(self.claims(subject), "at+jwt", "test-1")
    }

    async fn call(
        &self,
        subject: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.call_token(
            &self.token(subject),
            method,
            path,
            body,
            Some(&uuid::Uuid::new_v4().to_string()),
        )
        .await
    }

    async fn call_token(
        &self,
        token: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
        request_id: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("x-loofah-api-major", "1");
        if let Some(id) = request_id {
            request = request.header("x-request-id", id);
        }
        let body = if let Some(body) = body {
            request = request.header("content-type", "application/json");
            Body::from(body.to_string())
        } else {
            Body::empty()
        };
        let response = self
            .app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        assert!(response.headers().get("x-request-id").is_some());
        let bytes = to_bytes(response.into_body(), 6 * 1024 * 1024)
            .await
            .unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, body)
    }

    async fn login(&self, subject: &str) -> Me {
        let (status, body) = self.call(subject, "POST", "/api/v1/me", None).await;
        assert_eq!(status, StatusCode::OK);
        serde_json::from_value(body).unwrap()
    }

    async fn tcp(&self) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = self.app.clone();
        let stop = self.service.stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
                .unwrap();
        });
        (url, task)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.service.stop.cancel();
        self.issuer_task.abort();
    }
}

#[tokio::test]
async fn cancelling_first_read_does_not_cancel_or_duplicate_vault_initialization() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let (entered, receiver) = std::sync::mpsc::channel();
    let resume = Arc::new(std::sync::Barrier::new(2));
    *fixture.service.startup_pause.lock().unwrap() = Some((entered, resume.clone()));
    let task = {
        let service = fixture.service.clone();
        let id = me.personal_vault_id.clone();
        tokio::spawn(async move { service.vault(&id).await })
    };
    tokio::task::spawn_blocking(move || receiver.recv_timeout(std::time::Duration::from_secs(2)))
        .await
        .unwrap()
        .unwrap();
    task.abort();
    let Err(cancelled) = task.await else {
        panic!("read completed before cancellation");
    };
    assert!(cancelled.is_cancelled());
    let mut second = {
        let service = fixture.service.clone();
        let id = me.personal_vault_id.clone();
        tokio::spawn(async move { service.vault(&id).await })
    };
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut second)
            .await
            .is_err()
    );
    tokio::task::spawn_blocking(move || resume.wait())
        .await
        .unwrap();
    let context = second.await.unwrap().unwrap();
    assert!(Arc::ptr_eq(
        &context,
        &fixture.service.vault(&me.personal_vault_id).await.unwrap()
    ));
    fixture.service.shutdown().await.unwrap();
}

fn sign(claims: Value, typ: &str, kid: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some(typ.into());
    header.kid = Some(kid.into());
    encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(include_bytes!("../tests/fixtures/oidc-private.pem")).unwrap(),
    )
    .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn canonical_storage_and_derived_search_cache_are_private_to_the_service_user() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new().await;
    for path in [
        &fixture.service.config.storage_root,
        &fixture.service.config.cache_root,
    ] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    fixture.service.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_cannot_redirect_one_vault_into_another() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let context = fixture.service.vault(&bob.personal_vault_id).await.unwrap();
    let meta = serde_json::from_value(
        json!({"id":"same","title":"Bob private","created_at":"2026-11-05T10:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "bob-secret")
        .await
        .unwrap();
    let alice_session = fixture
        .service
        .config
        .storage_root
        .join("vaults")
        .join(&alice.personal_vault_id)
        .join("sessions/same");
    std::fs::create_dir_all(&alice_session).unwrap();
    for name in ["_meta.json", "notes.md"] {
        std::os::unix::fs::symlink(
            context.store.vault_base().join("sessions/same").join(name),
            alice_session.join(name),
        )
        .unwrap();
    }
    let (status, body) = fixture
        .call(
            "alice",
            "GET",
            &format!("/api/v1/vaults/{}/sessions/same", alice.personal_vault_id),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!body.to_string().contains("bob-secret"));
    assert_eq!(
        context.read_note("same").await.unwrap().markdown.as_deref(),
        Some("bob-secret")
    );
    fixture.service.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn service_storage_aliases_cannot_redirect_the_entire_vault_namespace() {
    let fixture = Fixture::new().await;
    let root = tempfile::tempdir().unwrap();
    let storage = root.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    std::os::unix::fs::symlink(
        fixture.service.config.storage_root.join("vaults"),
        storage.join("vaults"),
    )
    .unwrap();
    let mut config = fixture.service.config.clone();
    config.storage_root = storage;
    config.cache_root = root.path().join("cache");
    let error = Service::open(config)
        .await
        .err()
        .expect("aliased storage was accepted");
    assert_eq!(error.0.code, ErrorCode::Validation);
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn access_tokens_are_bound_to_issuer_audience_scope_expiry_type_and_signature() {
    let fixture = Fixture::new().await;
    let mut rejected = vec![
        sign(fixture.claims("alice"), "JWT", "test-1"),
        sign(fixture.claims("alice"), "at+jwt", "unknown"),
    ];
    for (key, value) in [
        ("iss", json!("https://attacker.example")),
        ("aud", json!("loofah-desktop")),
        ("scope", json!("openid profile")),
        ("exp", json!(now() - 1)),
        ("nbf", json!(now() + 100)),
        ("iat", json!(now() + 100)),
    ] {
        let mut claims = fixture.claims("alice");
        claims[key] = value;
        rejected.push(sign(claims, "at+jwt", "test-1"));
    }
    let token = fixture.token("alice");
    let mut pieces: Vec<String> = token.split('.').map(str::to_owned).collect();
    let replacement = if pieces[2].starts_with('A') { "B" } else { "A" };
    pieces[2].replace_range(..1, replacement);
    rejected.push(pieces.join("."));
    for token in rejected {
        let (status, body) = fixture
            .call_token(&token, "POST", "/api/v1/me", None, None)
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "unauthenticated");
        assert!(body["request_id"].as_str().is_some());
    }
    assert!(
        fixture
            .service
            .catalog
            .me(&crate::auth::Identity {
                issuer: fixture.issuer.clone(),
                subject: "alice".into(),
                name: "Alice".into(),
                expires_at: now() + 10
            })
            .await
            .is_err()
    );
    let (status, body) = fixture
        .call("alice", "GET", "/api/v1/auth/config", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["audience"], "loofah-api");
    assert!(body.get("storage_root").is_none());
    fixture.login("alice").await;
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn protocol_negotiation_and_client_request_ids_precede_mutations() {
    let fixture = Fixture::new().await;
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/me")
                .header(
                    "authorization",
                    format!("Bearer {}", fixture.token("alice")),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
    let me = fixture.login("alice").await;
    let path = format!("/api/v1/vaults/{}/sessions", me.personal_vault_id);
    let (status, body) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &path,
            Some(json!({"title":"Note","markdown":"body"})),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "validation");
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(
                    "authorization",
                    format!("Bearer {}", fixture.token("alice")),
                )
                .header("x-loofah-api-major", "1")
                .header("content-type", "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ApplicationError =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(error.code, ErrorCode::Validation);
    assert!(error.request_id.is_some());
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn unsupported_search_cost_options_are_rejected_before_running_a_query() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let path = format!("/api/v1/vaults/{}/search", me.personal_vault_id);
    for input in [
        json!({"query":"note","limit":0}),
        json!({"query":"note","options":{"distance":3}}),
        json!({"query":"note","options":{"snippet_max_chars":1000000}}),
    ] {
        let (status, body) = fixture.call("alice", "POST", &path, Some(input)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "validation");
        assert_eq!(body["retryable"], false);
    }
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_and_shared_core_agree_on_creates_conflicts_and_private_isolation() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let path = format!("/api/v1/vaults/{}/sessions", alice.personal_vault_id);
    let request_id = uuid::Uuid::new_v4().to_string();
    let request = json!({"title":"Title","markdown":"original body"});
    let (status, body) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &path,
            Some(request.clone()),
            Some(&request_id),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let created: SessionDocument = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(
        fixture
            .call_token(
                &fixture.token("alice"),
                "POST",
                &path,
                Some(request),
                Some(&request_id)
            )
            .await
            .1,
        body
    );
    let context = fixture
        .service
        .vault(&alice.personal_vault_id)
        .await
        .unwrap();
    assert_eq!(
        context.session_document(&created.meta.id).await.unwrap(),
        created
    );
    let note_path = format!("{path}/{}/note", created.meta.id);
    assert_eq!(
        fixture.call("bob", "GET", &note_path, None).await.0,
        StatusCode::NOT_FOUND
    );
    let update =
        json!({"markdown":"winner","title":"Updated","expected_revision":created.note.revision});
    let (status, saved) = fixture
        .call("alice", "PUT", &note_path, Some(update.clone()))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, conflict) = fixture.call("alice", "PUT", &note_path, Some(update)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["kind"], "conflict");
    assert_eq!(conflict["note"], saved["note"]);
    assert_eq!(
        context
            .read_note(&created.meta.id)
            .await
            .unwrap()
            .markdown
            .as_deref(),
        Some("winner")
    );
    assert_eq!(
        fixture
            .call(
                "bob",
                "GET",
                &format!("/api/v1/vaults/{}/sessions", bob.personal_vault_id),
                None
            )
            .await
            .1["sessions"],
        json!([])
    );
    let (status, error) = fixture
        .call("alice", "GET", &format!("{path}/%2e%2e/note"), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "validation");
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn same_session_id_in_two_vaults_keeps_content_separate_and_unknown_files_are_not_content() {
    let fixture = Fixture::new().await;
    for user in ["alice", "bob"] {
        let me = fixture.login(user).await;
        let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
        let meta = serde_json::from_value(
            json!({"id":"same","title":user,"created_at":"2026-11-05T10:00:00Z","tags":[]}),
        )
        .unwrap();
        context
            .store
            .create_note_session(&meta, user)
            .await
            .unwrap();
        std::fs::write(
            context.store.vault_base().join("sessions/same/unknown.txt"),
            "foreign-secret",
        )
        .unwrap();
        context.store.rebuild_index().await.unwrap();
        let (_, body) = fixture
            .call(
                user,
                "GET",
                &format!("/api/v1/vaults/{}/sessions/same", me.personal_vault_id),
                None,
            )
            .await;
        assert_eq!(body["note"]["markdown"], user);
        assert!(!body.to_string().contains("foreign-secret"));
        let search = hypr_search_cache::SearchRequest {
            query: "foreign-secret".into(),
            collection: None,
            filters: Default::default(),
            limit: 10,
            options: Default::default(),
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(result) = context.search.search(search.clone()).await {
                    assert_eq!(result.count, 0);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn temporary_write_capacity_rejection_is_retryable_and_does_not_change_the_note() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let base = format!("/api/v1/vaults/{}/sessions", me.personal_vault_id);
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            &base,
            Some(json!({"title":"Original","markdown":"original"})),
        )
        .await;
    let path = format!("{base}/{}/note", created["meta"]["id"].as_str().unwrap());
    let input = json!({"markdown":"updated","title":"Updated","expected_revision":created["note"]["revision"]});
    let capacity = fixture
        .service
        .writes
        .clone()
        .acquire_many_owned(32)
        .await
        .unwrap();
    let (status, body) = fixture
        .call("alice", "PUT", &path, Some(input.clone()))
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["retryable"], true);
    assert_eq!(
        fixture.call("alice", "GET", &path, None).await.1["markdown"],
        "original"
    );
    drop(capacity);
    let (status, body) = fixture.call("alice", "PUT", &path, Some(input)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["note"]["markdown"], "updated");
    fixture.service.shutdown().await.unwrap();
}

async fn next_event(response: &mut reqwest::Response, buffer: &mut String) -> ChangeEvent {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(end) = buffer.find("\n\n") {
                let frame: String = buffer.drain(..end + 2).collect();
                if let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) {
                    return serde_json::from_str(data).unwrap();
                }
            } else {
                let chunk = response
                    .chunk()
                    .await
                    .unwrap()
                    .expect("stream ended before the event");
                buffer.push_str(std::str::from_utf8(&chunk).unwrap());
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn catalog_changes_invalidate_an_already_open_personal_vault_stream() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let (url, task) = fixture.tcp().await;
    let mut stream = reqwest::Client::new()
        .get(format!(
            "{url}/api/v1/vaults/{}/events",
            me.personal_vault_id
        ))
        .bearer_auth(fixture.token("alice"))
        .header("x-loofah-api-major", "1")
        .send()
        .await
        .unwrap();
    let mut buffer = String::new();
    assert_eq!(
        next_event(&mut stream, &mut buffer).await.change,
        Change::ResyncRequired
    );
    assert_eq!(
        fixture
            .call(
                "alice",
                "POST",
                "/api/v1/vaults",
                Some(json!({"name":"New shared vault"}))
            )
            .await
            .0,
        StatusCode::OK
    );
    loop {
        if next_event(&mut stream, &mut buffer).await.change == Change::CatalogChanged {
            break;
        }
    }
    drop(stream);
    fixture.service.shutdown().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn real_tcp_stream_replays_changes_and_closes_after_membership_revocation() {
    let fixture = Fixture::new().await;
    fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Team"})),
        )
        .await;
    let id = shared["id"].as_str().unwrap();
    let membership_path = format!("/api/v1/vaults/{id}/members/{}", bob.member.id);
    let (status, changed) = fixture
        .call(
            "alice",
            "PUT",
            &membership_path,
            Some(json!({"role":"reader","expected_revision":shared["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (url, task) = fixture.tcp().await;
    let client = reqwest::Client::new();
    let events_url = format!("{url}/api/v1/vaults/{id}/events");
    let mut response = client
        .get(&events_url)
        .bearer_auth(fixture.token("bob"))
        .header("x-loofah-api-major", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut buffer = String::new();
    let initial = next_event(&mut response, &mut buffer).await;
    assert_eq!(initial.change, Change::ResyncRequired);
    drop(response);
    fixture
        .call(
            "alice",
            "POST",
            &format!("/api/v1/vaults/{id}/sessions"),
            Some(json!({"title":"Created","markdown":"content"})),
        )
        .await;
    let mut response = client
        .get(&events_url)
        .bearer_auth(fixture.token("bob"))
        .header("x-loofah-api-major", "1")
        .header("last-event-id", initial.cursor)
        .send()
        .await
        .unwrap();
    let mut buffer = String::new();
    loop {
        let event = next_event(&mut response, &mut buffer).await;
        if matches!(event.change, Change::IndexChanged { .. }) {
            break;
        }
    }
    let (status, _) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.call(
            "alice",
            "DELETE",
            &format!(
                "{membership_path}?expected_revision={}",
                changed["revision"].as_str().unwrap()
            ),
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::OK);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while response.chunk().await.unwrap().is_some() {}
    })
    .await
    .unwrap();
    assert_eq!(
        fixture
            .call("bob", "GET", &format!("/api/v1/vaults/{id}"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    fixture.service.shutdown().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn real_tcp_stream_expires_and_overflow_requires_resync() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let (url, task) = fixture.tcp().await;
    let client = reqwest::Client::new();
    let mut claims = fixture.claims("alice");
    claims["exp"] = json!(now() + 2);
    let url = format!("{url}/api/v1/vaults/{}/events", me.personal_vault_id);
    let mut response = client
        .get(&url)
        .bearer_auth(sign(claims, "at+jwt", "test-1"))
        .header("x-loofah-api-major", "1")
        .send()
        .await
        .unwrap();
    let initial = next_event(&mut response, &mut String::new()).await;
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        while response.chunk().await.unwrap().is_some() {}
    })
    .await
    .unwrap();
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    for _ in 0..300 {
        context.events.publish(Change::SearchReady);
    }
    let mut response = client
        .get(&url)
        .bearer_auth(fixture.token("alice"))
        .header("x-loofah-api-major", "1")
        .header("last-event-id", initial.cursor)
        .send()
        .await
        .unwrap();
    assert_eq!(
        next_event(&mut response, &mut String::new()).await.change,
        Change::ResyncRequired
    );
    drop(response);
    fixture.service.shutdown().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
}

struct MockOAuth {
    issuer: String,
    codes: tokio::sync::Mutex<std::collections::HashMap<String, (String, String)>>,
}

async fn mock_authorize(
    axum::extract::State(state): axum::extract::State<Arc<MockOAuth>>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Redirect {
    assert_eq!(query["client_id"], "loofah-desktop");
    assert_eq!(query["audience"], "loofah-api");
    assert_eq!(query["code_challenge_method"], "S256");
    let code = uuid::Uuid::new_v4().to_string();
    state.codes.lock().await.insert(
        code.clone(),
        (
            query["code_challenge"].clone(),
            query["redirect_uri"].clone(),
        ),
    );
    let mut redirect = url::Url::parse(&query["redirect_uri"]).unwrap();
    redirect.query_pairs_mut().extend_pairs([
        ("code", code.as_str()),
        ("state", query["state"].as_str()),
        ("iss", state.issuer.as_str()),
    ]);
    axum::response::Redirect::to(redirect.as_str())
}

async fn mock_token(
    axum::extract::State(state): axum::extract::State<Arc<MockOAuth>>,
    axum::extract::Form(form): axum::extract::Form<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    use base64::Engine;
    use sha2::Digest;
    let (challenge, redirect) = state.codes.lock().await.remove(&form["code"]).unwrap();
    assert_eq!(
        challenge,
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(form["code_verifier"].as_bytes()))
    );
    assert_eq!(redirect, form["redirect_uri"]);
    Json(
        json!({"access_token":sign(json!({"iss":state.issuer,"sub":"native-client","name":"Native client","aud":"loofah-api","exp":now()+3600,"iat":now(),"scope":"loofah:access"}),"at+jwt","test-1"),"token_type":"Bearer","expires_in":3600}),
    )
}

struct EphemeralCredentials;
impl hypr_loofah_client::CredentialStore for EphemeralCredentials {
    fn load<'a>(
        &'a self,
        _account: &'a str,
    ) -> hypr_loofah_client::CredentialFuture<'a, Option<String>> {
        Box::pin(async { Ok(None) })
    }
    fn save<'a>(
        &'a self,
        _account: &'a str,
        _value: Option<&'a str>,
    ) -> hypr_loofah_client::CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn native_clients_login_create_share_notes_receive_changes_and_reject_stale_replacements() {
    use hypr_loofah_client::{RemoteClient, RemoteEvent, RemoteOperation, RemoteReply};
    use tokio_util::sync::CancellationToken;
    let fixture = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = fixture.app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let first = RemoteClient::discover(&origin, Arc::new(EphemeralCredentials), true)
        .await
        .unwrap();
    let second = RemoteClient::discover(&origin, Arc::new(EphemeralCredentials), true)
        .await
        .unwrap();
    for client in [&first, &second] {
        let login = client.begin_login().await.unwrap();
        let url = login.authorization_url().to_string();
        let complete = tokio::spawn(login.complete());
        assert!(reqwest::get(url).await.unwrap().status().is_success());
        complete.await.unwrap().unwrap();
    }
    let request = |client: Arc<RemoteClient>, operation| async move {
        client
            .execute(
                operation,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
    };
    let RemoteReply::Me(me) = request(first.clone(), RemoteOperation::Provision).await else {
        panic!("me response");
    };
    let vault_id = me.personal_vault_id;
    let RemoteReply::Session(created) = request(
        first.clone(),
        RemoteOperation::CreateSession {
            vault_id: vault_id.clone(),
            input: CreateSessionRequest {
                title: "Shared application".into(),
                markdown: "Original".into(),
            },
        },
    )
    .await
    else {
        panic!("create response");
    };
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let subscription = second
        .subscribe(vault_id.clone(), move |event| sender.send(event).is_ok())
        .unwrap();
    loop {
        if matches!(
            tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
                .await
                .unwrap()
                .unwrap(),
            RemoteEvent::Change {
                event: ChangeEvent {
                    change: Change::ResyncRequired,
                    ..
                }
            }
        ) {
            break;
        }
    }
    let RemoteReply::NoteSave(NoteSaveOutcome::Saved(saved)) = request(
        first.clone(),
        RemoteOperation::SaveNote {
            vault_id: vault_id.clone(),
            session_id: created.meta.id.clone(),
            input: SaveNoteRequest {
                markdown: "Changed".into(),
                title: None,
                expected_revision: created.note.revision.clone(),
            },
        },
    )
    .await
    else {
        panic!("save response");
    };
    loop {
        if let RemoteEvent::Change {
            event:
                ChangeEvent {
                    change:
                        Change::IndexChanged {
                            entity: IndexEntity::Sessions,
                            ids,
                        },
                    ..
                },
        } = tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .unwrap()
        {
            if ids.contains(&created.meta.id) {
                break;
            }
        }
    }
    let RemoteReply::Note(current) = request(
        second.clone(),
        RemoteOperation::Note {
            vault_id: vault_id.clone(),
            session_id: created.meta.id.clone(),
        },
    )
    .await
    else {
        panic!("read response");
    };
    assert_eq!(current, saved);
    let RemoteReply::NoteSave(NoteSaveOutcome::Conflict(current)) = request(
        second.clone(),
        RemoteOperation::SaveNote {
            vault_id,
            session_id: created.meta.id,
            input: SaveNoteRequest {
                markdown: "Stale".into(),
                title: None,
                expected_revision: created.note.revision,
            },
        },
    )
    .await
    else {
        panic!("conflict response");
    };
    assert_eq!(current, saved);
    subscription.close().await;
    first.disconnect();
    second.disconnect();
    server.abort();
    fixture.service.shutdown().await.unwrap();
}

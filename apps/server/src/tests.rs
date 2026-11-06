mod attachment_remove;
mod attachment_upload;
mod attachments;
mod audio_metadata;
mod metadata;
mod note_editor;
mod recording;
mod recording_delete;
mod tasks;
mod waveform;

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

async fn wait_agent_projection(context: &hypr_loofah_core::VaultContext) {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            match context.agent_list(ListMeetingsInput::default()).await {
                Ok(_) => return,
                Err(error) if error.code == ErrorCode::Unavailable => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await
                }
                Err(error) => panic!("Agent projection failed: {error}"),
            }
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn agent_projections_preserve_cli_models_and_reader_private_vault_boundaries() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Agent reads"})),
        )
        .await;
    let id = shared["id"].as_str().unwrap();
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &format!("/api/v1/vaults/{id}/members/{}", bob.member.id),
                Some(json!({"role":"reader","expected_revision":shared["revision"]}))
            )
            .await
            .0,
        StatusCode::OK
    );
    let context = fixture.service.vault(id).await.unwrap();
    let meta = serde_json::from_value(json!({"id":"note","title":"Agent fixture","created_at":"2026-11-05T10:00:00Z","tags":["project"]})).unwrap();
    context.store.write_meta(&meta).await.unwrap();
    let dir = context.store.vault_base().join("sessions/note");
    std::fs::write(dir.join("_memo.md"), "Legacy canonical zephyr note").unwrap();
    std::fs::write(dir.join("unknown.md"), "ATTACHMENT CANARY").unwrap();
    std::fs::write(dir.join(".hidden.md"), "HIDDEN CANARY").unwrap();
    context.store.ensure_summary("note").await.unwrap();
    context
        .store
        .update_generated_summary("note", "Summary\n\n- [ ] Action", Some(""))
        .await
        .unwrap();
    context.store.write_transcript("note", serde_json::from_value(json!({"id":"transcript","session_id":"note","words":[{"text":"First","start_ms":0,"end_ms":100,"channel":0},{"text":"last.","start_ms":110,"end_ms":200,"channel":0}]})).unwrap()).await.unwrap();
    wait_agent_projection(&context).await;
    let responses = [
        (
            "",
            serde_json::to_value(context.agent_meeting("note").await.unwrap()).unwrap(),
        ),
        (
            "/transcript",
            serde_json::to_value(context.agent_transcript("note").await.unwrap()).unwrap(),
        ),
        (
            "/export",
            serde_json::to_value(context.agent_export("note").await.unwrap()).unwrap(),
        ),
        (
            "/segments",
            serde_json::to_value(context.agent_segments("note").await.unwrap()).unwrap(),
        ),
    ];
    for (suffix, expected) in responses {
        let path = format!("/api/v1/vaults/{id}/agent/sessions/note{suffix}");
        let (status, body) = fixture.call("bob", "GET", &path, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, expected);
        assert!(!body.to_string().contains("CANARY"));
        assert!(
            !body
                .to_string()
                .contains(&context.store.vault_base().to_string_lossy().to_string())
        );
        assert_eq!(
            fixture.call("charlie", "GET", &path, None).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            fixture
                .call(
                    "bob",
                    "GET",
                    &format!(
                        "/api/v1/vaults/{}/agent/sessions/note{suffix}",
                        alice.personal_vault_id
                    ),
                    None
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            fixture
                .call(
                    "bob",
                    "GET",
                    &format!("/api/v1/vaults/{id}/agent/sessions/missing{suffix}"),
                    None
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    for (route, input) in [
        (
            "list",
            json!({"query":"zephyr","limit":1,"tags":["PROJECT"]}),
        ),
        ("search", json!({"query":"zephyr","limit":1})),
    ] {
        let path = format!("/api/v1/vaults/{id}/agent/sessions/{route}");
        let (status, body) = fixture
            .call("bob", "POST", &path, Some(input.clone()))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["pagination"]["returned"], 1);
        assert_eq!(
            fixture.call("charlie", "POST", &path, Some(input)).await.0,
            StatusCode::NOT_FOUND
        );
        let (status, body) = fixture
            .call(
                "bob",
                "POST",
                &path,
                Some(json!({"query":"zephyr","offset":u32::MAX})),
            )
            .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["code"], "resource_limit");
        assert_eq!(body["retryable"], false);
    }
    assert!(dir.join("_memo.md").exists());
    assert!(!dir.join("notes.md").exists());
    assert_eq!(
        std::fs::read_to_string(dir.join("unknown.md")).unwrap(),
        "ATTACHMENT CANARY"
    );
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn document_edits_share_canonical_guards_and_never_grant_readers_write_access() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Documents"})),
        )
        .await;
    let id = shared["id"].as_str().unwrap();
    let (status, _) = fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{id}/members/{}", bob.member.id),
            Some(json!({"role":"reader","expected_revision":shared["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let context = fixture.service.vault(id).await.unwrap();
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Meeting","created_at":"2026-11-05T10:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Original note")
        .await
        .unwrap();
    context
        .store
        .write_enhanced_doc(&EnhancedDoc {
            id: "output".into(),
            session_id: "note".into(),
            kind: "template_output".into(),
            title: "Report".into(),
            template_id: "report".into(),
            sort_order: 1,
            markdown: "Original output".into(),
        })
        .await
        .unwrap();
    for resource in ["summary", "outputs/output"] {
        let path = format!("/api/v1/vaults/{id}/sessions/note/{resource}");
        let (status, snapshot) = fixture.call("bob", "GET", &path, None).await;
        assert_eq!(status, StatusCode::OK);
        let input =
            json!({"markdown":"Updated\n\n- [ ] Action", "expected_revision":snapshot["revision"]});
        assert_eq!(
            fixture
                .call("bob", "PUT", &path, Some(input.clone()))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fixture.call("charlie", "GET", &path, None).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            fixture
                .call("charlie", "PUT", &path, Some(input.clone()))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        let (status, saved) = fixture
            .call("alice", "PUT", &path, Some(input.clone()))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(saved["kind"], "saved");
        let (status, conflict) = fixture.call("alice", "PUT", &path, Some(input)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(conflict["snapshot"], saved["snapshot"]);
        assert_eq!(
            fixture.call("alice", "GET", &path, None).await.1,
            saved["snapshot"]
        );
        let private = format!(
            "/api/v1/vaults/{}/sessions/note/{resource}",
            alice.personal_vault_id
        );
        assert_eq!(
            fixture.call("bob", "GET", &private, None).await.0,
            StatusCode::NOT_FOUND
        );
    }
    let tasks = context.session_tasks("note").await.unwrap();
    assert_eq!(tasks.len(), 2);
    assert_ne!(tasks[0].source_type, tasks[1].source_type);
    assert_eq!(
        context.read_note("note").await.unwrap().markdown.as_deref(),
        Some("Original note")
    );
    let missing = format!("/api/v1/vaults/{id}/sessions/note/outputs/missing");
    let (_, absent) = fixture.call("alice", "GET", &missing, None).await;
    assert!(absent["document"].is_null());
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &missing,
                Some(json!({"markdown":"Do not create", "expected_revision":absent["revision"]}))
            )
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    for input in [
        json!({"markdown":"No revision"}),
        json!({"markdown":"Blank revision","expected_revision":""}),
    ] {
        assert_eq!(
            fixture
                .call(
                    "alice",
                    "PUT",
                    &format!("/api/v1/vaults/{id}/sessions/note/summary"),
                    Some(input)
                )
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    fixture.service.shutdown().await.unwrap();
}

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
        // Route fixtures isolate source delivery; native delivery has its own filesystem/HTTPS checks.
        service
            .injected_changes
            .store(true, std::sync::atomic::Ordering::Release);
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
async fn canonical_content_reads_are_available_to_readers_and_isolated_from_other_vaults() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Team"})),
        )
        .await;
    let vault_id = shared["id"].as_str().unwrap();
    let (status, _) = fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault_id}/members/{}", bob.member.id),
            Some(json!({"role":"reader","expected_revision":shared["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let context = fixture.service.vault(vault_id).await.unwrap();
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Team note","created_at":"2026-11-05T10:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Note")
        .await
        .unwrap();
    context.store.ensure_summary("note").await.unwrap();
    context
        .store
        .update_summary("note", "Private summary", Some(""))
        .await
        .unwrap();
    let transcript: TranscriptWithData = serde_json::from_value(json!({"id":"t","session_id":"note","words":[{"text":"Private transcript","start_ms":0,"end_ms":250,"channel":0}]})).unwrap();
    context
        .store
        .write_transcript("note", transcript)
        .await
        .unwrap();
    context
        .store
        .replace_tasks(
            "session_raw_note",
            "note",
            vec![hypr_vault_write::TaskInput {
                id: "task".into(),
                source_order: 0,
                status: "todo".into(),
                text: "Private task".into(),
                body: json!([]),
                due_at: String::new(),
            }],
        )
        .await
        .unwrap();
    context.store.ensure_person("Private person").await.unwrap();
    context.store.ensure_tag("private-tag").await.unwrap();
    let unknown = context.store.vault_base().join("sessions/note/unknown.txt");
    std::fs::write(&unknown, "foreign secret").unwrap();
    let responses = [
        (
            "sessions/note/documents",
            serde_json::to_value(context.session_documents("note").await.unwrap()).unwrap(),
        ),
        (
            "sessions/note/transcripts",
            serde_json::to_value(context.session_transcripts("note").await.unwrap()).unwrap(),
        ),
        (
            "sessions/note/tasks",
            serde_json::to_value(context.session_tasks("note").await.unwrap()).unwrap(),
        ),
        (
            "people",
            serde_json::to_value(context.people().await.unwrap()).unwrap(),
        ),
        (
            "tags",
            serde_json::to_value(context.tags().await.unwrap()).unwrap(),
        ),
    ];
    for (resource, expected) in responses {
        let path = format!("/api/v1/vaults/{vault_id}/{resource}");
        for member in ["alice", "bob"] {
            let (status, actual) = fixture.call(member, "GET", &path, None).await;
            assert_eq!(status, StatusCode::OK, "{resource}");
            assert_eq!(actual, expected);
            assert!(!actual.to_string().contains("foreign secret"));
        }
        assert_eq!(
            fixture.call("charlie", "GET", &path, None).await.0,
            StatusCode::NOT_FOUND
        );
        let private = format!("/api/v1/vaults/{}/{resource}", alice.personal_vault_id);
        assert_eq!(
            fixture.call("bob", "GET", &private, None).await.0,
            StatusCode::NOT_FOUND
        );
    }
    for resource in ["documents", "transcripts", "tasks"] {
        let (status, body) = fixture
            .call(
                "bob",
                "GET",
                &format!("/api/v1/vaults/{vault_id}/sessions/missing/{resource}"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "not_found");
    }
    assert_eq!(std::fs::read_to_string(unknown).unwrap(), "foreign secret");
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn content_read_capacity_is_retryable_and_shutdown_waits_for_admitted_reads() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let capacity = fixture
        .service
        .content_reads
        .clone()
        .acquire_many_owned(4)
        .await
        .unwrap();
    let path = format!("/api/v1/vaults/{}/tags", me.personal_vault_id);
    let (status, body) = fixture.call("alice", "GET", &path, None).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["code"], "resource_limit");
    assert_eq!(body["retryable"], true);
    drop(capacity);
    assert_eq!(
        fixture.call("alice", "GET", &path, None).await,
        (StatusCode::OK, json!([]))
    );
    let read = fixture
        .service
        .content_reads
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    let service = fixture.service.clone();
    let mut shutdown = tokio::spawn(async move { service.shutdown().await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), &mut shutdown)
            .await
            .is_err()
    );
    drop(read);
    tokio::time::timeout(std::time::Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
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
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_native_source_delivers_external_attachment_events_over_authenticated_tcp() {
    let fixture = Fixture::new().await;
    fixture
        .service
        .injected_changes
        .store(false, std::sync::atomic::Ordering::Release);
    let me = fixture.login("alice").await;
    let root = fixture
        .service
        .config
        .storage_root
        .join("vaults")
        .join(&me.personal_vault_id);
    let seed = hypr_vault_write::SessionStore::new(root.clone());
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Native source","created_at":"2026-11-06T19:00:00Z","tags":[]}),
    )
    .unwrap();
    seed.create_note_session(&meta, "Before external edit")
        .await
        .unwrap();
    seed.save_attachment("note", "image.svg", b"<svg/>".to_vec())
        .await
        .unwrap();
    let directory = root.join("sessions/note");
    std::fs::write(
        directory.join("unknown-user.txt"),
        b"Keep unknown attachment",
    )
    .unwrap();
    let before_meta = std::fs::read(directory.join("_meta.json")).unwrap();
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
    assert_eq!(stream.status(), StatusCode::OK);
    let mut buffer = String::new();
    assert_eq!(
        next_event(&mut stream, &mut buffer).await.change,
        Change::ResyncRequired
    );
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    std::fs::write(
        directory.join("attachments/image.svg"),
        b"<svg><!--external--></svg>",
    )
    .unwrap();
    std::fs::write(directory.join("notes.md"), b"After external edit").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let event = next_event(&mut stream, &mut buffer).await;
            assert_eq!(event.vault_id, me.personal_vault_id);
            if let Change::IndexChanged {
                entity: IndexEntity::Artifacts,
                artifacts: Some(details),
                ..
            } = event.change
                && details.contains(&ArtifactChange {
                    session_id: "note".into(),
                    target: ArtifactTarget::Attachment {
                        attachment_id: "image.svg".into(),
                    },
                })
            {
                break;
            }
        }
    })
    .await
    .expect("production source must deliver a scoped attachment notification");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while context
            .store
            .session_get("note")
            .unwrap()
            .note_markdown
            .as_deref()
            != Some("After external edit")
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("production service must reconcile external canonical edits");
    assert_eq!(
        std::fs::read(directory.join("_meta.json")).unwrap(),
        before_meta
    );
    assert_eq!(
        std::fs::read(directory.join("unknown-user.txt")).unwrap(),
        b"Keep unknown attachment"
    );
    drop(stream);
    fixture.service.shutdown().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
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
    let context = fixture.service.vault(&vault_id).await.unwrap();
    context
        .store
        .ensure_summary(&created.meta.id)
        .await
        .unwrap();
    context
        .store
        .update_summary(&created.meta.id, "Canonical summary", Some(""))
        .await
        .unwrap();
    context.store.ensure_person("Native person").await.unwrap();
    context.store.ensure_tag("native-tag").await.unwrap();
    assert!(
        matches!(request(first.clone(), RemoteOperation::Documents { vault_id: vault_id.clone(), session_id: created.meta.id.clone() }).await, RemoteReply::Documents(value) if value.len() == 1 && value[0].markdown == "Canonical summary")
    );
    let RemoteReply::Document(summary) = request(
        first.clone(),
        RemoteOperation::Document {
            vault_id: vault_id.clone(),
            session_id: created.meta.id.clone(),
            target: DocumentTarget::Summary,
        },
    )
    .await
    else {
        panic!("document response");
    };
    let input = SaveDocumentRequest {
        markdown: "Native document replacement".into(),
        expected_revision: summary.revision,
    };
    assert!(matches!(
        request(
            first.clone(),
            RemoteOperation::SaveDocument {
                vault_id: vault_id.clone(),
                session_id: created.meta.id.clone(),
                target: DocumentTarget::Summary,
                input: input.clone()
            }
        )
        .await,
        RemoteReply::DocumentSave(DocumentSaveOutcome::Saved(_))
    ));
    assert!(matches!(
        request(
            second.clone(),
            RemoteOperation::SaveDocument {
                vault_id: vault_id.clone(),
                session_id: created.meta.id.clone(),
                target: DocumentTarget::Summary,
                input
            }
        )
        .await,
        RemoteReply::DocumentSave(DocumentSaveOutcome::Conflict(_))
    ));
    assert!(
        matches!(request(first.clone(), RemoteOperation::Transcripts { vault_id: vault_id.clone(), session_id: created.meta.id.clone() }).await, RemoteReply::Transcripts(value) if value.is_empty())
    );
    assert!(
        matches!(request(first.clone(), RemoteOperation::Tasks { vault_id: vault_id.clone(), session_id: created.meta.id.clone() }).await, RemoteReply::Tasks(value) if value.is_empty())
    );
    assert!(
        matches!(request(first.clone(), RemoteOperation::People { vault_id: vault_id.clone() }).await, RemoteReply::People(value) if value.len() == 1 && value[0].name == "Native person")
    );
    assert!(
        matches!(request(first.clone(), RemoteOperation::Tags { vault_id: vault_id.clone() }).await, RemoteReply::Tags(value) if value.len() == 1 && value[0].name == "native-tag")
    );
    let RemoteReply::Metadata(metadata) = request(
        first.clone(),
        RemoteOperation::Metadata {
            vault_id: vault_id.clone(),
            session_id: created.meta.id.clone(),
        },
    )
    .await
    else {
        panic!("metadata response");
    };
    let metadata_input = SaveMetadataRequest {
        patch: SessionMetaPatch {
            tags: Some(vec!["native-tag".into()]),
            ..Default::default()
        },
        expected_revision: metadata.revision,
    };
    assert!(
        matches!(request(first.clone(), RemoteOperation::SaveMetadata { vault_id: vault_id.clone(), session_id: created.meta.id.clone(), input: metadata_input.clone() }).await, RemoteReply::MetadataSave(MetadataSaveOutcome::Saved(value)) if value.meta.tags == ["native-tag"])
    );
    assert!(matches!(
        request(
            second.clone(),
            RemoteOperation::SaveMetadata {
                vault_id: vault_id.clone(),
                session_id: created.meta.id.clone(),
                input: metadata_input
            }
        )
        .await,
        RemoteReply::MetadataSave(MetadataSaveOutcome::Conflict(_))
    ));
    let RemoteReply::TasksSnapshot(task_snapshot) = request(
        first.clone(),
        RemoteOperation::TasksSnapshot {
            vault_id: vault_id.clone(),
            session_id: created.meta.id.clone(),
        },
    )
    .await
    else {
        panic!("task snapshot response");
    };
    let tasks_input = ReplaceTasksRequest {
        source: TaskSource::RawNote,
        tasks: vec![TaskInput {
            id: "native-task".into(),
            source_order: 0,
            status: "done".into(),
            text: "Native task".into(),
            body: json!([]),
            due_at: "".into(),
        }],
        expected_revision: task_snapshot.revision,
    };
    assert!(
        matches!(request(first.clone(),RemoteOperation::ReplaceTasks{vault_id:vault_id.clone(),session_id:created.meta.id.clone(),input:tasks_input.clone()}).await,RemoteReply::TasksSave(TasksSaveOutcome::Saved(snapshot)) if snapshot.tasks.len()==1 && snapshot.tasks[0].id=="native-task")
    );
    assert!(matches!(
        request(
            second.clone(),
            RemoteOperation::ReplaceTasks {
                vault_id: vault_id.clone(),
                session_id: created.meta.id.clone(),
                input: tasks_input
            }
        )
        .await,
        RemoteReply::TasksSave(TasksSaveOutcome::Conflict(_))
    ));
    wait_agent_projection(&context).await;
    assert!(
        matches!(request(first.clone(), RemoteOperation::AgentList { vault_id: vault_id.clone(), input: ListMeetingsInput::default() }).await, RemoteReply::AgentList(value) if value.meetings.len() == 1 && value.meetings[0].id == created.meta.id)
    );
    assert!(
        matches!(request(first.clone(), RemoteOperation::AgentSearch { vault_id: vault_id.clone(), input: SearchMeetingsInput { query: "Original".into(), limit: Some(10), offset: None } }).await, RemoteReply::AgentSearch(value) if value.hits.len() == 1)
    );
    assert!(
        matches!(request(first.clone(), RemoteOperation::AgentMeeting { vault_id: vault_id.clone(), session_id: created.meta.id.clone() }).await, RemoteReply::AgentMeeting(value) if value.note.as_ref().unwrap().markdown == "Original")
    );
    assert!(
        matches!(request(first.clone(), RemoteOperation::AgentTranscript { vault_id: vault_id.clone(), session_id: created.meta.id.clone() }).await, RemoteReply::AgentTranscript(value) if value.meeting_id == created.meta.id && value.text.is_empty())
    );
    assert!(
        matches!(request(first.clone(), RemoteOperation::AgentExport { vault_id: vault_id.clone(), session_id: created.meta.id.clone() }).await, RemoteReply::AgentExport(value) if value.meeting.id == created.meta.id && value.transcripts.is_empty())
    );
    assert!(
        matches!(request(first.clone(), RemoteOperation::AgentSegments { vault_id: vault_id.clone(), session_id: created.meta.id.clone() }).await, RemoteReply::AgentSegments(value) if value.is_empty())
    );
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
                markdown: "Changed\n\n- [ ] Native note task".into(),
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
                            ..
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
    let RemoteReply::TasksSnapshot(saved_tasks) = request(
        second.clone(),
        RemoteOperation::TasksSnapshot {
            vault_id: vault_id.clone(),
            session_id: created.meta.id.clone(),
        },
    )
    .await
    else {
        panic!("task snapshot response")
    };
    assert_eq!(saved_tasks.tasks.len(), 1);
    assert_eq!(saved_tasks.tasks[0].text, "Native note task");
    assert_eq!(saved_tasks.tasks[0].status, "todo");
    assert!(
        matches!(request(second.clone(), RemoteOperation::NoteEditor { vault_id: vault_id.clone(), session_id: created.meta.id.clone() }).await, RemoteReply::NoteEditor(editor) if editor.note == saved && editor.tasks == saved_tasks)
    );
    let RemoteReply::NoteSave(NoteSaveOutcome::Conflict(current)) = request(
        second.clone(),
        RemoteOperation::SaveNote {
            vault_id: vault_id.clone(),
            session_id: created.meta.id.clone(),
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
    assert!(
        matches!(request(second.clone(), RemoteOperation::TasksSnapshot { vault_id: vault_id.clone(), session_id: created.meta.id.clone() }).await, RemoteReply::TasksSnapshot(current) if current == saved_tasks)
    );
    assert!(matches!(
        request(
            first.clone(),
            RemoteOperation::AudioMetadata {
                vault_id: vault_id.clone(),
                session_id: created.meta.id.clone()
            }
        )
        .await,
        RemoteReply::AudioMetadata(None)
    ));
    assert!(
        first
            .open_recording(
                &vault_id,
                &created.meta.id,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new()
            )
            .await
            .unwrap()
            .is_none()
    );
    let context = fixture.service.vault(&vault_id).await.unwrap();
    let recording = context
        .store
        .vault_base()
        .join("sessions")
        .join(&created.meta.id)
        .join("audio.mp3");
    std::fs::write(&recording, b"Native recording metadata bytes").unwrap();
    let expected = context.audio_metadata(&created.meta.id).await.unwrap();
    assert!(
        matches!(request(second.clone(),RemoteOperation::AudioMetadata {vault_id:vault_id.clone(),session_id:created.meta.id.clone()}).await,RemoteReply::AudioMetadata(metadata) if metadata == expected)
    );
    let lease_id = uuid::Uuid::new_v4().to_string();
    let lease = first
        .open_recording(
            &vault_id,
            &created.meta.id,
            &lease_id,
            CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Some(lease.metadata.clone()), expected);
    first
        .head_recording(&lease, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        first
            .open_recording(
                &vault_id,
                &created.meta.id,
                &lease_id,
                CancellationToken::new()
            )
            .await
            .unwrap(),
        Some(lease.clone())
    );
    assert_eq!(
        first
            .read_recording_chunk(&lease, 7, 9, CancellationToken::new())
            .await
            .unwrap(),
        b"recording"
    );
    let mut stale = lease.clone();
    stale.metadata.sha256 = "0".repeat(64);
    assert_eq!(
        first
            .read_recording_chunk(&stale, 0, 1, CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    first
        .close_recording(&vault_id, &lease_id, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        first
            .read_recording_chunk(&lease, 0, 1, CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    std::fs::remove_file(&recording).unwrap();
    let wav_path = recording.parent().unwrap().join("audio.wav");
    std::fs::write(&wav_path, waveform::wav()).unwrap();
    let wav_id = uuid::Uuid::new_v4().to_string();
    let wav_lease = first
        .open_recording(
            &vault_id,
            &created.meta.id,
            &wav_id,
            CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap();
    let waveform = first
        .recording_waveform(&wav_lease, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(waveform.recording_sha256, wav_lease.metadata.sha256);
    assert_eq!(waveform.peaks.duration_sec, 1.0);
    let deletion_id = uuid::Uuid::new_v4().to_string();
    let deletion = RemoteOperation::DeleteRecording {
        vault_id: vault_id.clone(),
        session_id: created.meta.id.clone(),
        input: DeleteRecordingRequest {
            expected_sha256: wav_lease.metadata.sha256.clone(),
        },
    };
    assert!(matches!(
        first
            .execute(deletion.clone(), &deletion_id, CancellationToken::new())
            .await
            .unwrap(),
        RemoteReply::RecordingDeleted(true)
    ));
    assert!(matches!(
        first
            .execute(deletion.clone(), &deletion_id, CancellationToken::new())
            .await
            .unwrap(),
        RemoteReply::RecordingDeleted(false)
    ));
    assert_eq!(
        first
            .read_recording_chunk(&wav_lease, 0, 1, CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    std::fs::write(&wav_path, b"New recording after deletion").unwrap();
    let rejected = first
        .execute(deletion, &deletion_id, CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(rejected.code, ErrorCode::Conflict);
    assert_eq!(
        rejected.mutation_outcome,
        Some(MutationOutcome::NotCommitted)
    );
    assert_eq!(
        std::fs::read(&wav_path).unwrap(),
        b"New recording after deletion"
    );
    let payload = b"Portable attachment bytes";
    let attachment = context
        .store
        .save_attachment(&created.meta.id, "spec + # ü.pdf", payload.to_vec())
        .await
        .unwrap();
    let attachment_id = uuid::Uuid::new_v4().to_string();
    let RemoteReply::AttachmentInfo(Some(info)) = request(
        first.clone(),
        RemoteOperation::AttachmentInfo {
            vault_id: vault_id.clone(),
            session_id: created.meta.id.clone(),
            attachment_id: attachment.attachment_id.clone(),
        },
    )
    .await
    else {
        panic!("attachment information response")
    };
    assert_eq!(info.filename, attachment.attachment_id);
    assert_eq!(info.content_type, "application/pdf");
    assert_eq!(info.size_bytes, payload.len() as u64);
    context
        .store
        .save_attachment(&created.meta.id, "zz-tail.txt", b"tail".to_vec())
        .await
        .unwrap();
    let RemoteReply::Attachments(page) = request(
        first.clone(),
        RemoteOperation::Attachments {
            vault_id: vault_id.clone(),
            session_id: created.meta.id.clone(),
            limit: 1,
            after: None,
        },
    )
    .await
    else {
        panic!("attachment page response")
    };
    assert_eq!(page.files, vec![info.clone()]);
    assert_eq!(
        page.next_cursor.as_deref(),
        Some(attachment.attachment_id.as_str())
    );
    let RemoteReply::Attachments(page) = request(
        first.clone(),
        RemoteOperation::Attachments {
            vault_id: vault_id.clone(),
            session_id: created.meta.id.clone(),
            limit: 1,
            after: page.next_cursor,
        },
    )
    .await
    else {
        panic!("attachment continuation response")
    };
    assert_eq!(page.files[0].filename, "zz-tail.txt");
    assert!(page.next_cursor.is_none());

    let attachment_lease = first
        .open_attachment(
            &vault_id,
            &created.meta.id,
            &attachment.attachment_id,
            &attachment_id,
            CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(attachment_lease.metadata.filename, attachment.attachment_id);
    assert_eq!(attachment_lease.metadata.content_type, "application/pdf");
    first
        .head_attachment(&attachment_lease, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        first
            .read_attachment_chunk(
                &attachment_lease,
                0,
                payload.len(),
                CancellationToken::new()
            )
            .await
            .unwrap(),
        payload
    );
    first
        .close_attachment(&vault_id, &attachment_id, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        first
            .head_attachment(&attachment_lease, CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    for (name, bytes) in [
        ("full-download.bin", payload.as_slice()),
        ("empty-download.bin", b"".as_slice()),
    ] {
        context
            .store
            .save_attachment(&created.meta.id, name, bytes.to_vec())
            .await
            .unwrap();
        let lease = first
            .open_attachment(
                &vault_id,
                &created.meta.id,
                name,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .unwrap();
        let downloaded = first
            .download_attachment(lease.clone(), Vec::new(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(downloaded, bytes);
        first
            .close_attachment(&vault_id, &lease.id, CancellationToken::new())
            .await
            .unwrap();
    }
    let input = SaveNoteEditorRequest {
        markdown: "Changed\n\n- [x] Renamed native task".into(),
        title: None,
        expected_revision: saved.revision,
        expected_tasks_revision: saved_tasks.revision,
        tasks: vec![TaskInput {
            id: saved_tasks.tasks[0].id.clone(),
            source_order: 0,
            status: "done".into(),
            text: "Renamed native task".into(),
            body: serde_json::json!([{"type":"paragraph","content":[{"type":"text","text":"Renamed native task"}]}]),
            due_at: "2026-12-01".into(),
        }],
    };
    let RemoteReply::NoteEditorSave(NoteEditorSaveOutcome::Saved(editor)) = request(
        first.clone(),
        RemoteOperation::SaveNoteEditor {
            vault_id: vault_id.clone(),
            session_id: created.meta.id.clone(),
            input: input.clone(),
        },
    )
    .await
    else {
        panic!("native editor save response")
    };
    assert_eq!(editor.tasks.tasks[0].id, saved_tasks.tasks[0].id);
    assert_eq!(editor.tasks.tasks[0].status, "done");
    assert_eq!(editor.tasks.tasks[0].due_at, "2026-12-01");
    assert!(
        matches!(request(second.clone(), RemoteOperation::SaveNoteEditor { vault_id, session_id: created.meta.id, input }).await, RemoteReply::NoteEditorSave(NoteEditorSaveOutcome::Conflict(current)) if current == editor)
    );
    subscription.close().await;
    first.disconnect();
    second.disconnect();
    server.abort();
    fixture.service.shutdown().await.unwrap();
}

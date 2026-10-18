use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use hypr_vault_access::{Call, Role, User, Versions, digest};
use loofah_server::{Config, Server, content, http::router, registry::Session};
use serde_json::{Value, json};
use tower::ServiceExt;

fn open(root: &std::path::Path) -> Arc<Server> {
    Server::open(Config {
        data_dir: root.into(),
        public_url: "http://127.0.0.1:8947".into(),
        issuer: "http://127.0.0.1:8948".into(),
        client_id: "test".into(),
        client_secret: None,
        max_upload_bytes: 1024 * 1024,
    })
    .unwrap()
}

struct Fixture {
    root: tempfile::TempDir,
    server: Arc<Server>,
    app: Router,
    users: Vec<User>,
    vault: String,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let server = open(root.path());
        let mut users = Vec::new();
        for name in ["owner", "editor", "viewer", "stranger"] {
            let user = server
                .provision("issuer", name, name, &format!("{name}@example.com"))
                .await
                .unwrap();
            let now = chrono::Utc::now().timestamp();
            server.registry.write().await.sessions.insert(
                digest(format!("refresh-{name}").as_bytes()),
                Session {
                    user_id: user.id.clone(),
                    access_hash: digest(name.as_bytes()),
                    refresh_hash: digest(format!("refresh-{name}").as_bytes()),
                    access_expires: now + 3600,
                    expires: now + 7200,
                },
            );
            users.push(user);
        }
        let vault = server.create_vault(&users[0], "Team").await.unwrap().id;
        server
            .update_member(&users[0], &vault, &users[1].id, Some(Role::Editor))
            .await
            .unwrap();
        server
            .update_member(&users[0], &vault, &users[2].id, Some(Role::Viewer))
            .await
            .unwrap();
        let f = Self {
            root,
            app: router(server.clone()),
            server,
            users,
            vault,
        };
        f.mutate("session_write_meta", json!({"meta":{"id":"s1","title":"Meeting","created_at":"2026-10-18T10:00:00Z","tags":[]}})).await;
        f
    }

    fn prefix(&self) -> String {
        format!("/v1/vaults/{}", self.vault)
    }

    async fn invoke(&self, call: Call) -> Value {
        let (status, body) = request(
            &self.app,
            "POST",
            &format!("{}/invoke", self.prefix()),
            "owner",
            json!(call),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["data"].clone()
    }

    async fn query(&self, operation: &str, args: Value) -> Value {
        self.invoke(Call::query(operation, args)).await
    }

    async fn mutate(&self, operation: &str, args: Value) -> Value {
        use hypr_vault_access::filesystem::VaultStorage;
        let runtime = self.server.runtime(&self.vault).await.unwrap();
        let keys =
            content::document_keys(&runtime.storage.content, &runtime.store.session_ids()).unwrap();
        let expected = runtime.storage.versions(&keys).unwrap();
        self.invoke(Call::mutation(operation, args, expected)).await
    }

    async fn begin(&self, bytes: &[u8], filename: &str) -> String {
        let (status, upload) = request(&self.app, "POST", &format!("{}/uploads", self.prefix()), "owner", json!({"session_id":"s1","filename":filename,"size":bytes.len(),"sha256":digest(bytes)})).await;
        assert_eq!(status, StatusCode::OK, "{upload}");
        format!(
            "{}/uploads/{}",
            self.prefix(),
            upload["id"].as_str().unwrap()
        )
    }
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes))),
    )
}

async fn chunk(
    app: &Router,
    path: &str,
    token: &str,
    offset: Option<&str>,
    bytes: &[u8],
) -> StatusCode {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(path)
        .header("authorization", format!("Bearer {token}"));
    if let Some(offset) = offset {
        builder = builder.header("Upload-Offset", offset);
    }
    app.clone()
        .oneshot(builder.body(Body::from(bytes.to_vec())).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn authorization_matrix_covers_every_resource_and_personal_vault_isolation() {
    let f = Fixture::new().await;
    for token in ["", "wrong-token", "owner-tampered"] {
        for path in [
            "/v1/me".into(),
            "/v1/users".into(),
            "/v1/vaults".into(),
            format!("{}/sessions/s1/artifacts", f.prefix()),
            format!("{}/uploads/unknown", f.prefix()),
            format!("{}/operations/unknown", f.prefix()),
        ] {
            assert_eq!(
                request(&f.app, "GET", &path, token, Value::Null).await.0,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
    }
    for (actor, can_read, can_write, can_manage) in [
        ("owner", true, true, true),
        ("editor", true, true, false),
        ("viewer", true, false, false),
        ("stranger", false, false, false),
    ] {
        let (status, visible) = request(&f.app, "GET", "/v1/vaults", actor, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            visible
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["id"] == f.vault),
            can_read
        );
        assert_eq!(
            request(
                &f.app,
                "POST",
                &format!("{}/invoke", f.prefix()),
                actor,
                json!(Call::query("session_get", json!({"session_id":"s1"})))
            )
            .await
            .0,
            if can_read {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
        assert_eq!(
            request(
                &f.app,
                "POST",
                &format!("{}/invoke", f.prefix()),
                actor,
                json!(Call::mutation(
                    "session_update_meta",
                    json!({"session_id":"s1","patch":{"title":actor}}),
                    Versions::new()
                ))
            )
            .await
            .0,
            if can_write {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
        assert_eq!(
            request(&f.app, "PUT", &f.prefix(), actor, json!({"name":"Renamed"}))
                .await
                .0,
            if can_manage {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
        assert_eq!(
            request(
                &f.app,
                "PUT",
                &format!("{}/members/{}", f.prefix(), f.users[3].id),
                actor,
                json!({"role":"viewer"})
            )
            .await
            .0,
            if can_manage {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
        if can_manage {
            f.server
                .update_member(&f.users[0], &f.vault, &f.users[3].id, None)
                .await
                .unwrap();
        }
    }
    let personal = f
        .server
        .registry
        .read()
        .await
        .vaults
        .values()
        .find(|v| v.personal && v.members.contains_key(&f.users[0].id))
        .unwrap()
        .id
        .clone();
    assert_eq!(
        request(
            &f.app,
            "POST",
            &format!("/v1/vaults/{personal}/invoke"),
            "stranger",
            json!(Call::query("session_ids", json!({})))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for member in [&f.users[0].id, &f.users[1].id] {
        assert_eq!(
            request(
                &f.app,
                "PUT",
                &format!("/v1/vaults/{personal}/members/{member}"),
                "owner",
                json!({"role":"viewer"})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &f.app,
                "DELETE",
                &format!("/v1/vaults/{personal}/members/{member}"),
                "owner",
                Value::Null
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    let (status, users) = request(&f.app, "GET", "/v1/users", "owner", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        users
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u.get("issuer").is_none() && u.get("subject").is_none())
    );
}

#[tokio::test]
async fn concurrent_owner_departures_retain_an_owner_and_failed_registry_writes_publish_nothing() {
    let f = Fixture::new().await;
    f.server
        .update_member(&f.users[0], &f.vault, &f.users[1].id, Some(Role::Owner))
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        f.server
            .update_member(&f.users[0], &f.vault, &f.users[0].id, None),
        f.server
            .update_member(&f.users[1], &f.vault, &f.users[1].id, None)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        f.server.registry.read().await.vaults[&f.vault]
            .members
            .values()
            .filter(|r| **r == Role::Owner)
            .count(),
        1
    );
    let survivor = if a.is_ok() { &f.users[1] } else { &f.users[0] };
    assert_eq!(
        f.server
            .update_member(survivor, &f.vault, &survivor.id, Some(Role::Viewer))
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    let registry = f.root.path().join("service/registry.json");
    let original = std::fs::read(&registry).unwrap();
    std::fs::remove_file(&registry).unwrap();
    std::fs::create_dir(&registry).unwrap();
    let before = f.server.registry.read().await.vaults.len();
    assert!(
        f.server
            .create_vault(survivor, "Must not appear")
            .await
            .is_err()
    );
    assert_eq!(f.server.registry.read().await.vaults.len(), before);
    assert!(
        f.server
            .update_member(survivor, &f.vault, &f.users[3].id, Some(Role::Editor))
            .await
            .is_err()
    );
    assert!(
        !f.server.registry.read().await.vaults[&f.vault]
            .members
            .contains_key(&f.users[3].id)
    );
    std::fs::remove_dir(&registry).unwrap();
    std::fs::write(&registry, original).unwrap();
}

#[tokio::test]
async fn mutation_receipts_are_actor_scoped_and_reused_ids_cannot_change_requests() {
    let f = Fixture::new().await;
    let call = Call::mutation(
        "session_update_meta",
        json!({"session_id":"s1","patch":{"title":"Original request"}}),
        Versions::new(),
    );
    let id = call.mutation_id.as_deref().unwrap();
    let path = format!("{}/operations/{id}", f.prefix());
    assert_eq!(
        request(&f.app, "GET", &path, "owner", Value::Null).await.1,
        Value::Null
    );
    let first = f.invoke(call.clone()).await;
    assert_eq!(f.invoke(call.clone()).await, first);
    assert!(
        request(&f.app, "GET", &path, "owner", Value::Null)
            .await
            .1
            .get("versions")
            .is_some()
    );
    assert_eq!(
        request(&f.app, "GET", &path, "editor", Value::Null).await.1,
        Value::Null
    );
    let mut changed = call;
    changed.args["patch"]["title"] = json!("Different request");
    assert_eq!(
        request(
            &f.app,
            "POST",
            &format!("{}/invoke", f.prefix()),
            "owner",
            json!(changed)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.query("session_get", json!({"session_id":"s1"})).await["meta"]["title"],
        "Original request"
    );
}

#[tokio::test]
async fn upload_validation_retries_and_cancellation_never_publish_partial_or_corrupt_content() {
    let f = Fixture::new().await;
    let base =
        json!({"session_id":"s1","filename":"file.txt","size":8,"sha256":digest(b"12345678")});
    for (key, value, status) in [
        ("size", json!(0), StatusCode::PAYLOAD_TOO_LARGE),
        (
            "size",
            json!(1024 * 1024 + 1),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        ("sha256", json!("bogus"), StatusCode::BAD_REQUEST),
        ("filename", json!(".hidden"), StatusCode::BAD_REQUEST),
        ("filename", json!("../escape"), StatusCode::BAD_REQUEST),
        ("filename", json!("a\\b"), StatusCode::BAD_REQUEST),
        ("filename", json!("audio.txt"), StatusCode::OK),
        ("session_id", json!("missing"), StatusCode::NOT_FOUND),
    ] {
        let mut body = base.clone();
        body[key] = value;
        assert_eq!(
            request(
                &f.app,
                "POST",
                &format!("{}/uploads", f.prefix()),
                "owner",
                body
            )
            .await
            .0,
            status
        );
    }
    let upload = f.begin(b"12345678", "file.txt").await;
    assert_eq!(
        chunk(&f.app, &upload, "owner", None, b"1234").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        chunk(&f.app, &upload, "owner", Some("-1"), b"1234").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        chunk(&f.app, &upload, "owner", Some("0"), b"").await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        chunk(&f.app, &upload, "owner", Some("4"), b"5678").await,
        StatusCode::PRECONDITION_FAILED
    );
    assert_eq!(
        chunk(&f.app, &upload, "owner", Some("0"), b"1234").await,
        StatusCode::OK
    );
    assert_eq!(
        chunk(&f.app, &upload, "owner", Some("0"), b"xxxx").await,
        StatusCode::PRECONDITION_FAILED
    );
    assert_eq!(
        chunk(&f.app, &upload, "owner", Some("2"), b"3456").await,
        StatusCode::PRECONDITION_FAILED
    );
    assert_eq!(
        chunk(&f.app, &upload, "owner", Some("4"), b"56789").await,
        StatusCode::PRECONDITION_FAILED
    );
    for actor in ["editor", "viewer", "stranger"] {
        assert_eq!(
            request(&f.app, "GET", &upload, actor, Value::Null).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            chunk(&f.app, &upload, actor, Some("4"), b"5678").await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(&f.app, "DELETE", &upload, actor, Value::Null)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &f.app,
                "POST",
                &format!("{upload}/finish"),
                actor,
                Value::Null
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        request(&f.app, "GET", &upload, "owner", Value::Null)
            .await
            .1["offset"],
        4
    );
    assert_ne!(
        request(
            &f.app,
            "POST",
            &format!("{upload}/finish"),
            "owner",
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        !f.root
            .path()
            .join(format!(
                "vaults/{}/sessions/s1/attachments/file.txt",
                f.vault
            ))
            .exists()
    );
    assert_eq!(
        chunk(&f.app, &upload, "owner", Some("4"), b"xxxx").await,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            &format!("{upload}/finish"),
            "owner",
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&f.app, "DELETE", &upload, "owner", Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&f.app, "GET", &upload, "owner", Value::Null)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert!(
        request(
            &f.app,
            "GET",
            &format!("{}/sessions/s1/artifacts", f.prefix()),
            "owner",
            Value::Null
        )
        .await
        .1
        .as_array()
        .unwrap()
        .is_empty()
    );
}

#[tokio::test]
async fn concurrent_duplicate_chunks_resume_after_restart_and_deleted_sessions_stay_deleted() {
    let f = Fixture::new().await;
    let upload = f.begin(b"12345678", "file.txt").await;
    let (a, b) = tokio::join!(
        chunk(&f.app, &upload, "owner", Some("0"), b"1234"),
        chunk(&f.app, &upload, "owner", Some("0"), b"1234")
    );
    assert_eq!((a, b), (StatusCode::OK, StatusCode::OK));
    f.server
        .save_registry(&*f.server.registry.read().await)
        .await
        .unwrap();
    let Fixture {
        root,
        server,
        app,
        users,
        vault,
    } = f;
    drop(app);
    drop(server);
    let server = open(root.path());
    let app = router(server.clone());
    assert_eq!(
        request(&app, "GET", &upload, "owner", Value::Null).await.1["offset"],
        4
    );
    assert_eq!(
        chunk(&app, &upload, "owner", Some("4"), b"5678").await,
        StatusCode::OK
    );
    let read = content::invoke(
        server.clone(),
        &users[0],
        &vault,
        Call::query("session_get", json!({"session_id":"s1"})),
    )
    .await
    .unwrap();
    content::invoke(
        server.clone(),
        &users[0],
        &vault,
        Call::mutation("session_delete", json!({"session_id":"s1"}), read.versions),
    )
    .await
    .unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("{upload}/finish"),
            "owner",
            Value::Null
        )
        .await
        .0,
        StatusCode::PRECONDITION_FAILED
    );
    assert!(
        !root
            .path()
            .join(format!("vaults/{vault}/sessions/s1"))
            .exists()
    );
}

#[tokio::test]
async fn operational_routes_and_disabled_sessions_report_the_current_persisted_state() {
    let f = Fixture::new().await;
    for path in ["/health", "/ready", "/v1/server", "/v1/openapi.json"] {
        let response = f
            .app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    }
    let before = request(&f.app, "GET", "/v1/server", "", Value::Null)
        .await
        .1;
    assert_eq!(before["api_version"], 1);
    assert!(
        request(&f.app, "GET", "/v1/openapi.json", "", Value::Null)
            .await
            .1["paths"]
            .get("/v1/vaults/{vault}/invoke")
            .is_some()
    );
    f.server
        .registry
        .write()
        .await
        .users
        .get_mut(&f.users[0].id)
        .unwrap()
        .disabled = true;
    assert_eq!(
        request(&f.app, "GET", "/v1/me", "owner", Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/v1/auth/refresh",
            "",
            json!({"refresh_token":"refresh-owner"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.server
            .provision("issuer", "owner", "Renamed", "new@example.com")
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    let visible = request(&f.app, "GET", "/v1/users", "viewer", Value::Null)
        .await
        .1;
    assert!(
        !visible
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["id"] == f.users[0].id)
    );
}

#[tokio::test]
async fn refresh_rotation_is_single_use_under_concurrency_and_does_not_extend_absolute_expiry() {
    let f = Fixture::new().await;
    let expires = chrono::Utc::now().timestamp() + 120;
    f.server
        .registry
        .write()
        .await
        .sessions
        .get_mut(&digest(b"refresh-owner"))
        .unwrap()
        .expires = expires;
    let renew = || {
        request(
            &f.app,
            "POST",
            "/v1/auth/refresh",
            "",
            json!({"refresh_token":"refresh-owner"}),
        )
    };
    let (a, b) = tokio::join!(renew(), renew());
    let (success, failure) = if a.0 == StatusCode::OK {
        (a, b)
    } else {
        (b, a)
    };
    assert_eq!(success.0, StatusCode::OK);
    assert_eq!(failure.0, StatusCode::UNAUTHORIZED);
    assert_eq!(success.1["expires_at"], expires);
    assert_eq!(
        request(&f.app, "GET", "/v1/me", "owner", Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &f.app,
            "GET",
            "/v1/me",
            success.1["access_token"].as_str().unwrap(),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn notes_summaries_templates_tasks_tags_and_people_roundtrip_through_the_public_protocol() {
    let f = Fixture::new().await;
    assert_eq!(
        f.query("session_is_empty", json!({"session_id":"s1"}))
            .await,
        false
    );
    f.mutate(
        "session_update_meta",
        json!({"session_id":"s1","patch":{"tracking_id":"calendar-id","folder":"Work"}}),
    )
    .await;
    assert_eq!(
        f.query(
            "session_find_by_tracking_id",
            json!({"tracking_id":"calendar-id"})
        )
        .await["id"],
        "s1"
    );
    f.mutate(
        "session_write_note",
        json!({"session_id":"s1","markdown":"Private notes"}),
    )
    .await;
    assert_eq!(
        f.query("session_read_note", json!({"session_id":"s1"}))
            .await,
        "Private notes"
    );
    f.mutate(
        "session_save_note",
        json!({"session_id":"s1","markdown":"Revised notes","title":"Revised title"}),
    )
    .await;
    assert_eq!(
        f.query("session_get", json!({"session_id":"s1"})).await["meta"]["title"],
        "Revised title"
    );
    f.mutate("session_ensure_summary", json!({"session_id":"s1"}))
        .await;
    f.mutate("session_update_summary",json!({"session_id":"s1","markdown":"Summary","suggested_tags":[{"name":"Project","confidence":0.6}],"auto_apply_high_confidence_tags":false,"reconcile_tasks":true})).await;
    assert_eq!(
        f.query("session_summary_get", json!({"session_id":"s1"}))
            .await,
        "Summary"
    );
    f.mutate(
        "session_accept_tag_suggestion",
        json!({"session_id":"s1","name":"Project"}),
    )
    .await;
    f.mutate(
        "session_dismiss_tag_suggestion",
        json!({"session_id":"s1","name":"Other"}),
    )
    .await;
    f.mutate("tags_ensure", json!({"name":" #Project "})).await;
    f.mutate("people_ensure", json!({"name":"Alice Example"}))
        .await;
    assert!(
        f.query("tags_list", json!({}))
            .await
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "project")
    );
    assert_eq!(
        f.query("people_list", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.query("session_tag_context", json!({"session_id":"s1"}))
        .await;
    f.mutate("session_save_summary",json!({"session_id":"s1","markdown":"Final summary","expected_markdown":"Summary","title":"Final title"})).await;
    let doc_id = uuid::Uuid::new_v4().to_string();
    f.mutate("session_write_enhanced_doc",json!({"doc":{"id":doc_id,"session_id":"s1","kind":"template_output","title":"Actions","template_id":"template","sort_order":0,"markdown":"Initial template"}})).await;
    f.mutate("session_update_enhanced_doc",json!({"session_id":"s1","doc_id":doc_id,"patch":{"markdown":"Updated template","expected_markdown":"Initial template"}})).await;
    f.mutate(
        "session_save_enhanced_doc",
        json!({"session_id":"s1","doc_id":doc_id,"patch":{"title":"Renamed actions"}}),
    )
    .await;
    assert_eq!(
        f.query("enhanced_doc_get", json!({"doc_id":doc_id})).await["markdown"],
        "Updated template"
    );
    assert!(
        f.query("session_enhanced_docs", json!({"session_id":"s1"}))
            .await
            .as_array()
            .unwrap()
            .iter()
            .any(|doc| doc["id"] == doc_id)
    );
    let task_id = uuid::Uuid::new_v4().to_string();
    f.mutate("session_replace_tasks",json!({"source_type":"session_raw_note","source_id":"s1","tasks":[{"id":task_id,"source_order":0,"status":"todo","text":"Follow up","body":[]}]})).await;
    assert_eq!(
        f.query(
            "session_list_tasks",
            json!({"source_type":"session_raw_note","source_id":"s1"})
        )
        .await[0]["text"],
        "Follow up"
    );
    f.mutate("session_move_tasks",json!({"task_ids":[task_id],"source_type":"enhanced_note","source_id":doc_id,"insertion_order":0})).await;
    assert_eq!(
        f.query(
            "session_list_tasks",
            json!({"source_type":"enhanced_note","source_id":doc_id})
        )
        .await[0]["id"],
        task_id
    );
    f.mutate(
        "session_remove_tasks",
        json!({"source_type":"enhanced_note","source_id":doc_id,"task_ids":[task_id]}),
    )
    .await;
    assert!(
        f.query(
            "session_list_tasks",
            json!({"source_type":"enhanced_note","source_id":doc_id})
        )
        .await
        .as_array()
        .unwrap()
        .is_empty()
    );
    f.mutate(
        "session_delete_enhanced_doc",
        json!({"session_id":"s1","doc_id":doc_id}),
    )
    .await;
    assert!(
        f.query("enhanced_doc_get", json!({"doc_id":doc_id}))
            .await
            .is_null()
    );
    f.mutate("session_delete_summary", json!({"session_id":"s1"}))
        .await;
    assert!(
        f.query("session_summary_get", json!({"session_id":"s1"}))
            .await
            .is_null()
    );
    assert_eq!(f.query("session_ids", json!({})).await, json!(["s1"]));
    assert_eq!(
        f.query("session_list_headers", json!({"limit":1}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        f.query("session_list_headers", json!({"after":"s1","limit":1}))
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.query("vault_stats", json!({})).await;
    f.query("vault_storage_stats", json!({})).await;
    assert_eq!(
        f.query("session_list_audio", json!({"session_id":"s1"}))
            .await,
        json!([])
    );
    let path = f
        .root
        .path()
        .join(format!("vaults/{}/sessions/s1/notes.md", f.vault));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "Revised notes");
}

#[tokio::test]
async fn transcript_streaming_replacement_and_speaker_edits_preserve_unknown_metadata() {
    let f = Fixture::new().await;
    let word = json!({"id":"word-1","text":"Hello","start_ms":0,"end_ms":500,"channel":0});
    f.mutate("session_prepare_recording", json!({"session_id":"s1"}))
        .await;
    f.mutate(
        "session_mark_recording_started",
        json!({"session_id":"s1","at":"2026-10-18T10:00:00Z"}),
    )
    .await;
    f.mutate("session_append_transcript",json!({"session_id":"s1","delta":{"transcript_id":"t1","new_words":[word],"replaced_ids":[],"new_hints":[],"started_at_ms":1000}})).await;
    f.mutate("session_flush_transcript", json!({"session_id":"s1"}))
        .await;
    f.mutate(
        "session_finish_transcript",
        json!({"session_id":"s1","transcript_id":"t1"}),
    )
    .await;
    f.mutate(
        "session_mark_recording_ended",
        json!({"session_id":"s1","at":"2026-10-18T10:01:00Z"}),
    )
    .await;
    assert_eq!(
        f.query("session_has_transcript", json!({"session_id":"s1"}))
            .await,
        true
    );
    assert_eq!(
        f.query("transcript_get", json!({"transcript_id":"t1"}))
            .await["words"][0]["text"],
        "Hello"
    );
    f.query("session_transcript_metadata", json!({"session_id":"s1"}))
        .await;
    let path = f
        .root
        .path()
        .join(format!("vaults/{}/sessions/s1/transcript.json", f.vault));
    let mut canonical: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    canonical["future_registry"] = json!({"keep":true});
    canonical["transcripts"][0]["future_transcript"] = json!(42);
    canonical["transcripts"][0]["words"][0]["future_word"] = json!("preserve");
    std::fs::write(&path, serde_json::to_vec(&canonical).unwrap()).unwrap();
    f.mutate("session_assign_transcript_speaker",json!({"transcript_id":"t1","channel":0,"speaker_index":null,"speaker_label":"Alice","anchor_word_id":"word-1"})).await;
    let edited: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(edited["future_registry"]["keep"], true);
    assert_eq!(edited["transcripts"][0]["future_transcript"], 42);
    assert_eq!(
        edited["transcripts"][0]["words"][0]["future_word"],
        "preserve"
    );
    f.mutate(
        "session_write_transcript",
        json!({"session_id":"s1","transcript":{"id":"t2","session_id":"s1","words":[word]}}),
    )
    .await;
    assert_eq!(
        f.query("session_transcripts", json!({"session_id":"s1"}))
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );
    f.mutate("session_begin_audio_import", json!({"session_id":"s1"}))
        .await;
    f.mutate(
        "session_replace_transcripts",
        json!({"session_id":"s1","transcript":{"id":"t3","session_id":"s1","words":[word]}}),
    )
    .await;
    f.mutate("session_finish_audio_import", json!({"session_id":"s1"}))
        .await;
    let transcripts = f
        .query("session_transcripts", json!({"session_id":"s1"}))
        .await;
    assert_eq!(transcripts.as_array().unwrap().len(), 1);
    assert_eq!(transcripts[0]["id"], "t3");
}

#[tokio::test]
async fn revocation_while_an_upload_request_waits_on_its_lock_prevents_cancellation() {
    let f = Fixture::new().await;
    let upload = f.begin(b"1234", "file.txt").await;
    let id = upload.rsplit('/').next().unwrap();
    let runtime = f.server.runtime(&f.vault).await.unwrap();
    let upload_lock = runtime
        .transfer_locks
        .lock()
        .await
        .entry(id.into())
        .or_default()
        .clone();
    let held = upload_lock.lock().await;
    let app = f.app.clone();
    let path = upload.clone();
    let cancel =
        tokio::spawn(async move { request(&app, "DELETE", &path, "owner", Value::Null).await });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while Arc::strong_count(&upload_lock) < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.server
        .update_member(&f.users[0], &f.vault, &f.users[1].id, Some(Role::Owner))
        .await
        .unwrap();
    f.server
        .update_member(&f.users[1], &f.vault, &f.users[0].id, None)
        .await
        .unwrap();
    drop(held);
    assert_eq!(cancel.await.unwrap().0, StatusCode::FORBIDDEN);
    assert!(
        f.root
            .path()
            .join(format!(
                "service/vaults/{}/uploads/{id}/state.json",
                f.vault
            ))
            .exists()
    );
}

#[tokio::test]
async fn media_tickets_enforce_ranges_session_scope_expiry_and_logout() {
    let f = Fixture::new().await;
    let upload = f.begin(b"12345678", "file.pdf").await;
    assert_eq!(
        chunk(&f.app, &upload, "owner", Some("0"), b"12345678").await,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            &format!("{upload}/finish"),
            "owner",
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    for key in [
        "sessions/other/attachments/file.pdf",
        "sessions/s1/attachments/../file.pdf",
        "sessions/s1/unknown.pdf",
    ] {
        assert_eq!(
            request(
                &f.app,
                "POST",
                &format!("{}/sessions/s1/media", f.prefix()),
                "owner",
                json!({"key":key})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let (_, ticket) = request(
        &f.app,
        "POST",
        &format!("{}/sessions/s1/media", f.prefix()),
        "owner",
        json!({"key":"sessions/s1/attachments/file.pdf"}),
    )
    .await;
    let url = url::Url::parse(ticket["url"].as_str().unwrap()).unwrap();
    let path = url.path();
    for (range, expected, status) in [
        ("bytes=0-2", &b"123"[..], StatusCode::PARTIAL_CONTENT),
        ("bytes=-2", &b"78"[..], StatusCode::PARTIAL_CONTENT),
        ("bytes=3-", &b"45678"[..], StatusCode::PARTIAL_CONTENT),
    ] {
        let response = f
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("Range", range)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()["content-type"], "application/pdf");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(response.headers()["access-control-allow-origin"], "*");
        assert_eq!(response.headers()["accept-ranges"], "bytes");
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            expected
        );
    }
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("Range", "bytes=99-")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    let token = path.rsplit('/').next().unwrap();
    let state = f
        .root
        .path()
        .join("service/media")
        .join(digest(token.as_bytes()));
    let mut persisted: Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
    persisted["expires"] = json!(0);
    std::fs::write(&state, serde_json::to_vec(&persisted).unwrap()).unwrap();
    assert_eq!(
        request(&f.app, "GET", path, "", Value::Null).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (_, ticket) = request(
        &f.app,
        "POST",
        &format!("{}/sessions/s1/media", f.prefix()),
        "owner",
        json!({"key":"sessions/s1/attachments/file.pdf"}),
    )
    .await;
    let fresh = url::Url::parse(ticket["url"].as_str().unwrap()).unwrap();
    assert_eq!(
        request(&f.app, "POST", "/v1/auth/logout", "owner", Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&f.app, "GET", fresh.path(), "", Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn interrupted_commits_block_partial_reads_then_publish_recovered_content_and_revisions_together()
 {
    use hypr_vault_access::filesystem::{Change, VaultStorage};
    let f = Fixture::new().await;
    f.mutate(
        "session_write_note",
        json!({"session_id":"s1","markdown":"Original"}),
    )
    .await;
    let runtime = f.server.runtime(&f.vault).await.unwrap();
    let blocked = runtime.storage.content.join("sessions/s1/summary.md");
    std::fs::create_dir(&blocked).unwrap();
    let error = runtime
        .storage
        .commit(
            "interrupted",
            "same",
            &Versions::new(),
            vec![
                Change::Write("sessions/s1/notes.md".into(), b"Recovered note".to_vec()),
                Change::Write(
                    "sessions/s1/summary.md".into(),
                    b"Recovered summary".to_vec(),
                ),
            ],
            json!(true),
        )
        .unwrap_err();
    assert_eq!(error.code, "commit_pending");
    runtime
        .failed
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        runtime
            .store
            .session_get("s1")
            .unwrap()
            .note_markdown
            .as_deref(),
        Some("Original")
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            &format!("{}/invoke", f.prefix()),
            "owner",
            json!(Call::query("session_get", json!({"session_id":"s1"})))
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(runtime.failed.load(std::sync::atomic::Ordering::Acquire));
    std::fs::remove_dir(blocked).unwrap();
    let (status, reply) = request(
        &f.app,
        "POST",
        &format!("{}/invoke", f.prefix()),
        "owner",
        json!(Call::query("session_get", json!({"session_id":"s1"}))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply["data"]["note_markdown"], "Recovered note");
    assert_eq!(
        reply["versions"]["sessions/s1/notes.md"],
        digest(b"Recovered note")
    );
    assert_eq!(
        reply["versions"]["sessions/s1/summary.md"],
        digest(b"Recovered summary")
    );
    assert_eq!(
        f.query("session_summary_get", json!({"session_id":"s1"}))
            .await,
        "Recovered summary"
    );
    assert!(!runtime.failed.load(std::sync::atomic::Ordering::Acquire));
}

#[tokio::test]
async fn shared_search_tracks_edits_and_deletion_without_exposing_another_vault() {
    let f = Fixture::new().await;
    f.mutate(
        "session_write_note",
        json!({"session_id":"s1","markdown":"UniqueTermForStorageSearch"}),
    )
    .await;
    let search = || async {
        request(
            &f.app,
            "POST",
            &format!("{}/invoke", f.prefix()),
            "viewer",
            json!(Call::query(
                "search",
                json!({"request":{"query":"UniqueTermForStorageSearch"}})
            )),
        )
        .await
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let (status, result) = search().await;
            if status == StatusCode::OK && result["data"]["count"].as_u64().unwrap() > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        request(
            &f.app,
            "POST",
            &format!("{}/invoke", f.prefix()),
            "stranger",
            json!(Call::query(
                "search",
                json!({"request":{"query":"UniqueTermForStorageSearch"}})
            ))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    f.mutate("session_delete", json!({"session_id":"s1"})).await;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let (status, result) = search().await;
            if status == StatusCode::OK && result["data"]["count"] == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn identities_use_issuer_and_subject_and_registry_corruption_fails_closed() {
    let f = Fixture::new().await;
    let first = f
        .server
        .provision("issuer-a", "same-subject", "Name", "same@example.com")
        .await
        .unwrap();
    let other_issuer = f
        .server
        .provision("issuer-b", "same-subject", "Name", "same@example.com")
        .await
        .unwrap();
    let other_subject = f
        .server
        .provision("issuer-a", "other-subject", "Name", "same@example.com")
        .await
        .unwrap();
    assert_ne!(first.id, other_issuer.id);
    assert_ne!(first.id, other_subject.id);
    let changed = f
        .server
        .provision("issuer-a", "same-subject", "Renamed", "new@example.com")
        .await
        .unwrap();
    assert_eq!(first.id, changed.id);
    for user in [&first, &other_issuer, &other_subject] {
        assert_eq!(
            f.server
                .registry
                .read()
                .await
                .vaults
                .values()
                .filter(|v| v.personal && v.members.contains_key(&user.id))
                .count(),
            1
        );
    }
    let root = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: root.path().into(),
        public_url: "https://notes.example.com".into(),
        issuer: "https://identity.example.com/realm".into(),
        client_id: "test".into(),
        client_secret: None,
        max_upload_bytes: 1024,
    };
    let server = Server::open(config.clone()).unwrap();
    drop(server);
    let path = root.path().join("service/registry.json");
    let mut original: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    original["version"] = json!(999);
    std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    assert_eq!(
        Server::open(config.clone()).err().unwrap().code,
        "invalid_data"
    );
    std::fs::write(&path, b"{broken").unwrap();
    assert_eq!(
        Server::open(config.clone()).err().unwrap().code,
        "invalid_data"
    );
    let mut unsafe_config = config;
    unsafe_config.issuer = "http://attacker.invalid".into();
    assert_eq!(
        Server::open(unsafe_config).err().unwrap().code,
        "invalid_request"
    );
}

#[tokio::test]
async fn queued_receipt_reads_recheck_membership_after_an_earlier_revocation() {
    let f = Fixture::new().await;
    f.server
        .update_member(&f.users[0], &f.vault, &f.users[1].id, Some(Role::Owner))
        .await
        .unwrap();
    let runtime = f.server.runtime(&f.vault).await.unwrap();
    let gate = runtime.gate.write().await;
    let mut revoke = Box::pin(
        f.server
            .update_member(&f.users[1], &f.vault, &f.users[0].id, None),
    );
    assert!(futures_util::poll!(revoke.as_mut()).is_pending());
    let path = format!("{}/operations/any-operation", f.prefix());
    let mut read = Box::pin(request(&f.app, "GET", &path, "owner", Value::Null));
    assert!(futures_util::poll!(read.as_mut()).is_pending());
    drop(gate);
    revoke.await.unwrap();
    assert_eq!(read.await.0, StatusCode::FORBIDDEN);
}

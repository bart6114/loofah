use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use hypr_vault_access::{Call, Role, Versions, digest};
use loofah_server::{Config, Server, content, http::router, registry::Session};
use serde_json::{Value, json};
use tower::ServiceExt;

fn open(root: &std::path::Path) -> Arc<Server> {
    Server::open(Config {
        data_dir: root.to_path_buf(),
        public_url: "http://127.0.0.1:8947".into(),
        issuer: "http://127.0.0.1:8948".into(),
        client_id: "test".into(),
        client_secret: None,
        max_upload_bytes: 1024 * 1024,
    })
    .unwrap()
}

async fn token(server: &Server, user_id: &str, token: &str) {
    let mut registry = server.registry.write().await;
    registry.sessions.insert(
        token.into(),
        Session {
            user_id: user_id.into(),
            access_hash: digest(token.as_bytes()),
            refresh_hash: digest(format!("refresh-{token}").as_bytes()),
            access_expires: chrono::Utc::now().timestamp() + 3600,
            expires: chrono::Utc::now().timestamp() + 7200,
        },
    );
    server.save_registry(&registry).await.unwrap();
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

fn meta(id: &str) -> Value {
    json!({"id":id,"title":"Test","created_at":"2026-10-18T10:00:00Z","started_at":null,"ended_at":null,"tags":[],"future_field":{"preserved":true}})
}

#[tokio::test]
async fn concurrent_first_logins_and_restart_keep_one_personal_vault_and_stable_identity() {
    let root = tempfile::tempdir().unwrap();
    let server = open(root.path());
    let (a, b) = tokio::join!(
        server.provision("issuer", "subject", "Name", "old@example.com"),
        server.provision("issuer", "subject", "Name", "old@example.com")
    );
    let a = a.unwrap();
    assert_eq!(a.id, b.unwrap().id);
    let registry = server.registry.read().await;
    assert_eq!(registry.users.len(), 1);
    assert_eq!(registry.vaults.len(), 1);
    let vault_id = registry.vaults.keys().next().unwrap().clone();
    drop(registry);
    drop(server);
    let server = open(root.path());
    let user = server
        .provision("issuer", "subject", "Renamed", "new@example.com")
        .await
        .unwrap();
    assert_eq!(user.id, a.id);
    assert_eq!(
        server.registry.read().await.vaults.keys().next().unwrap(),
        &vault_id
    );
}

#[tokio::test]
async fn permissions_stale_saves_receipts_and_persistent_restore() {
    let root = tempfile::tempdir().unwrap();
    let server = open(root.path());
    let owner = server
        .provision("issuer", "owner", "Owner", "owner@example.com")
        .await
        .unwrap();
    let viewer = server
        .provision("issuer", "viewer", "Viewer", "viewer@example.com")
        .await
        .unwrap();
    token(&server, &owner.id, "owner").await;
    token(&server, &viewer.id, "viewer").await;
    let vault = server.create_vault(&owner, "Team").await.unwrap();
    server
        .update_member(&owner, &vault.id, &viewer.id, Some(Role::Viewer))
        .await
        .unwrap();
    let app = router(server.clone());
    let path = format!("/v1/vaults/{}/invoke", vault.id);
    let create = Call::mutation(
        "session_write_meta",
        json!({"meta":meta("s1")}),
        Versions::new(),
    );
    assert_eq!(
        request(&app, "POST", &path, "viewer", json!(create))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "POST", &path, "owner", json!(create)).await.0,
        StatusCode::OK
    );
    let note = Call::mutation(
        "session_save_note",
        json!({"session_id":"s1","markdown":"first","title":null}),
        Versions::new(),
    );
    let reply = content::invoke(server.clone(), &owner, &vault.id, note.clone())
        .await
        .unwrap();
    assert_eq!(
        content::invoke(server.clone(), &owner, &vault.id, note)
            .await
            .unwrap()
            .versions,
        reply.versions
    );
    let expected = reply.versions;
    let second = Call::mutation(
        "session_save_note",
        json!({"session_id":"s1","markdown":"second","title":null}),
        expected.clone(),
    );
    content::invoke(server.clone(), &owner, &vault.id, second)
        .await
        .unwrap();
    let stale = Call::mutation(
        "session_save_note",
        json!({"session_id":"s1","markdown":"lost","title":null}),
        expected,
    );
    assert_eq!(
        request(&app, "POST", &path, "owner", json!(stale)).await.0,
        StatusCode::PRECONDITION_FAILED
    );
    let read = content::invoke(
        server.clone(),
        &owner,
        &vault.id,
        Call::query("session_get", json!({"session_id":"s1"})),
    )
    .await
    .unwrap();
    assert_eq!(read.data["note_markdown"], "second");
    assert_eq!(read.data["meta"]["future_field"]["preserved"], true);
    content::invoke(
        server.clone(),
        &owner,
        &vault.id,
        Call::mutation("session_delete", json!({"session_id":"s1"}), read.versions),
    )
    .await
    .unwrap();
    drop(app);
    drop(server);
    let server = open(root.path());
    let restored = content::invoke(
        server.clone(),
        &owner,
        &vault.id,
        Call::mutation(
            "session_restore",
            json!({"session_id":"s1"}),
            Versions::new(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(restored.data, true);
    let read = content::invoke(
        server.clone(),
        &owner,
        &vault.id,
        Call::query("session_get", json!({"session_id":"s1"})),
    )
    .await
    .unwrap();
    assert_eq!(read.data["note_markdown"], "second");
    assert!(
        server
            .update_member(&owner, &vault.id, &owner.id, None)
            .await
            .is_err()
    );
    server
        .update_member(&owner, &vault.id, &viewer.id, None)
        .await
        .unwrap();
    let app = router(server);
    assert_eq!(
        request(
            &app,
            "POST",
            &path,
            "viewer",
            json!(Call::query("session_get", json!({"session_id":"s1"})))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn real_http_client_roundtrip_and_vault_isolation() {
    use hypr_vault_access::VaultAccess;
    let root = tempfile::tempdir().unwrap();
    let server = open(root.path());
    let owner = server
        .provision("issuer", "owner", "Owner", "o@example.com")
        .await
        .unwrap();
    token(&server, &owner.id, "owner").await;
    let a = server.create_vault(&owner, "A").await.unwrap();
    let b = server.create_vault(&owner, "B").await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(axum::serve(listener, router(server)).into_future());
    let mut client = hypr_vault_access::client::RemoteVault {
        client: reqwest::Client::new(),
        base_url: format!("http://{address}"),
        vault_id: a.id,
        access_token: "owner".into(),
    };
    client
        .invoke(Call::mutation(
            "session_write_meta",
            json!({"meta":meta("same-id")}),
            Versions::new(),
        ))
        .await
        .unwrap();
    client.vault_id = b.id;
    assert_eq!(
        client
            .invoke(Call::query("session_get", json!({"session_id":"same-id"})))
            .await
            .unwrap()
            .data,
        Value::Null
    );
    task.abort();
}

#[tokio::test]
async fn remote_adapter_reports_denied_malformed_and_disconnected_responses() {
    use hypr_vault_access::{VaultAccess, client::RemoteVault};
    let app = Router::new().route(
        "/v1/vaults/{vault}/invoke",
        axum::routing::post(
            |axum::extract::Path(vault): axum::extract::Path<String>| async move {
                match vault.as_str() {
                    "denied" => (
                        StatusCode::FORBIDDEN,
                        r#"{"code":"forbidden","message":"membership removed"}"#,
                    ),
                    "gateway" => (StatusCode::BAD_GATEWAY, "<html>upstream unavailable</html>"),
                    _ => (StatusCode::OK, "invalid JSON"),
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(axum::serve(listener, app).into_future());
    let mut client = RemoteVault {
        client: reqwest::Client::new(),
        base_url: format!("http://{address}/"),
        vault_id: String::new(),
        access_token: "access".into(),
    };
    for (vault, code) in [
        ("denied", "forbidden"),
        ("gateway", "unavailable"),
        ("malformed", "invalid_data"),
    ] {
        client.vault_id = vault.into();
        let error = client
            .invoke(Call::query("session_get", json!({"session_id":"s1"})))
            .await
            .unwrap_err();
        assert_eq!(error.code, code);
        if vault == "denied" {
            assert_eq!(error.message, "membership removed");
        }
    }
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let error = client
        .invoke(Call::query("session_get", json!({"session_id":"s1"})))
        .await
        .unwrap_err();
    assert_eq!(error.code, "unavailable");
}

#[test]
fn media_ranges_are_bounded() {
    use loofah_server::transfers::range;
    assert_eq!(range(Some("bytes=2-4"), 10).unwrap(), (2, 4, true));
    assert_eq!(range(Some("bytes=-3"), 10).unwrap(), (7, 9, true));
    assert_eq!(range(Some("bytes=8-999"), 10).unwrap(), (8, 9, true));
    for value in ["bytes=11-", "bytes=5-2", "bytes=0-2,4-6", "bytes=-0"] {
        assert!(range(Some(value), 10).is_err());
    }
}

#[tokio::test]
async fn upload_resume_duplicate_chunks_publication_and_media_revocation() {
    let root = tempfile::tempdir().unwrap();
    let server = open(root.path());
    let owner = server
        .provision("issuer", "owner", "Owner", "owner@example.com")
        .await
        .unwrap();
    let viewer = server
        .provision("issuer", "viewer", "Viewer", "viewer@example.com")
        .await
        .unwrap();
    token(&server, &owner.id, "owner").await;
    token(&server, &viewer.id, "viewer").await;
    let vault = server.create_vault(&owner, "Team").await.unwrap();
    server
        .update_member(&owner, &vault.id, &viewer.id, Some(Role::Viewer))
        .await
        .unwrap();
    content::invoke(
        server.clone(),
        &owner,
        &vault.id,
        Call::mutation(
            "session_write_meta",
            json!({"meta":meta("s1")}),
            Versions::new(),
        ),
    )
    .await
    .unwrap();
    let app = router(server.clone());
    let data = b"an uploaded attachment";
    let prefix = format!("/v1/vaults/{}", vault.id);
    let (status,upload)=request(&app,"POST",&format!("{prefix}/uploads"),"owner",json!({"session_id":"s1","filename":"sample.txt","size":data.len(),"sha256":digest(data),"expected":null,"audio":false})).await;
    assert_eq!(status, StatusCode::OK);
    let id = upload["id"].as_str().unwrap();
    let chunk = |offset, body: Vec<u8>| {
        Request::builder()
            .method("PUT")
            .uri(format!("{prefix}/uploads/{id}"))
            .header("authorization", "Bearer owner")
            .header("Upload-Offset", offset)
            .body(Body::from(body))
            .unwrap()
    };
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(chunk(0, data[..8].to_vec()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert!(
        !root
            .path()
            .join(format!(
                "vaults/{}/sessions/s1/attachments/sample.txt",
                vault.id
            ))
            .exists()
    );
    let response = app
        .clone()
        .oneshot(chunk(8, data[8..].to_vec()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("{prefix}/uploads/{id}/finish"),
            "owner",
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("{prefix}/uploads/{id}/finish"),
            "owner",
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let key = "sessions/s1/attachments/sample.txt";
    let (status, ticket) = request(
        &app,
        "POST",
        &format!("{prefix}/sessions/s1/media"),
        "viewer",
        json!({"key":key}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let media_path = url::Url::parse(ticket["url"].as_str().unwrap())
        .unwrap()
        .path()
        .to_string();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&media_path)
                .header("Range", "bytes=3-9")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        data[3..10]
    );
    server
        .update_member(&owner, &vault.id, &viewer.id, None)
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&media_path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let (status, _) = request(
        &app,
        "POST",
        &format!("{prefix}/sessions/s1/media"),
        "owner",
        json!({"key":"sessions/s1/unknown.txt"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

async fn upload_bytes(
    app: &Router,
    prefix: &str,
    filename: &str,
    bytes: &[u8],
    expected: Versions,
) -> (StatusCode, Value) {
    let (status,upload)=request(app,"POST",&format!("{prefix}/uploads"),"owner",json!({"session_id":"s1","filename":filename,"size":bytes.len(),"sha256":digest(bytes),"expected":expected.get(&format!("sessions/s1/{filename}")).cloned().flatten(),"expected_versions":expected,"audio":true})).await;
    if status != StatusCode::OK {
        return (status, upload);
    }
    let path = format!("{prefix}/uploads/{}", upload["id"].as_str().unwrap());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(&path)
                .header("authorization", "Bearer owner")
                .header("Upload-Offset", 0)
                .body(Body::from(bytes.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    request(app, "POST", &format!("{path}/finish"), "owner", json!({})).await
}

#[tokio::test]
async fn delayed_recording_cannot_replace_a_newer_recording_or_extension() {
    let root = tempfile::tempdir().unwrap();
    let server = open(root.path());
    let owner = server
        .provision("issuer", "owner", "Owner", "owner@example.com")
        .await
        .unwrap();
    token(&server, &owner.id, "owner").await;
    let vault = server.create_vault(&owner, "Recordings").await.unwrap();
    content::invoke(
        server.clone(),
        &owner,
        &vault.id,
        Call::mutation(
            "session_write_meta",
            json!({"meta":meta("s1")}),
            Versions::new(),
        ),
    )
    .await
    .unwrap();
    let app = router(server);
    let prefix = format!("/v1/vaults/{}", vault.id);
    let initial = Versions::from([
        ("sessions/s1/audio.mp3".into(), None),
        ("sessions/s1/audio.wav".into(), None),
        ("sessions/s1/audio.ogg".into(), None),
    ]);
    assert_eq!(
        upload_bytes(
            &app,
            &prefix,
            "audio.mp3",
            b"first recording",
            initial.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, ticket) = request(
        &app,
        "POST",
        &format!("{prefix}/sessions/s1/media"),
        "owner",
        json!({"key":"sessions/s1/audio.mp3"}),
    )
    .await;
    let old_media = url::Url::parse(ticket["url"].as_str().unwrap())
        .unwrap()
        .path()
        .to_string();
    let baseline = Versions::from([
        (
            "sessions/s1/audio.mp3".into(),
            Some(digest(b"first recording")),
        ),
        ("sessions/s1/audio.wav".into(), None),
        ("sessions/s1/audio.ogg".into(), None),
    ]);
    assert_eq!(
        upload_bytes(
            &app,
            &prefix,
            "audio.mp3",
            b"newer recording",
            baseline.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        upload_bytes(&app, &prefix, "audio.wav", b"delayed recording", baseline)
            .await
            .0,
        StatusCode::PRECONDITION_FAILED
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&old_media)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let (status, artifacts) = request(
        &app,
        "GET",
        &format!("{prefix}/sessions/s1/artifacts"),
        "owner",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(artifacts[0]["filename"], "audio.mp3");
    assert_eq!(artifacts[0]["version"], digest(b"newer recording"));
    let (status,_)=request(&app,"POST",&format!("{prefix}/uploads"),"owner",json!({"session_id":"s1","filename":"../escape","size":1,"sha256":digest(b"x"),"audio":false})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn event_stream_resets_and_revokes_removed_members() {
    let root = tempfile::tempdir().unwrap();
    let server = open(root.path());
    let owner = server
        .provision("issuer", "owner", "Owner", "o@example.com")
        .await
        .unwrap();
    let member = server
        .provision("issuer", "member", "Member", "m@example.com")
        .await
        .unwrap();
    token(&server, &member.id, "member").await;
    let vault = server.create_vault(&owner, "Events").await.unwrap();
    server
        .update_member(&owner, &vault.id, &member.id, Some(Role::Viewer))
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(axum::serve(listener, router(server.clone())).into_future());
    let mut stream = reqwest::Client::new()
        .get(format!("http://{address}/v1/events"))
        .bearer_auth("member")
        .send()
        .await
        .unwrap();
    assert!(stream.status().is_success());
    let reset = tokio::time::timeout(std::time::Duration::from_secs(3), stream.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&reset).contains("event: reset"));
    server
        .update_member(&owner, &vault.id, &member.id, None)
        .await
        .unwrap();
    let revoked = tokio::time::timeout(std::time::Duration::from_secs(3), stream.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&revoked).contains("event: revoked"));
    server.stopping.send_replace(true);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(3), stream.chunk())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    task.abort();
    let _ = task.await;
}

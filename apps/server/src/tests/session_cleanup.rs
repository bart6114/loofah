use super::*;
use hypr_loofah_client::{RemoteClient, RemoteOperation, RemoteReply};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

async fn cleanup(
    fixture: &Fixture,
    subject: &str,
    vault: &str,
    request_id: Option<&str>,
    input: Value,
) -> (StatusCode, Value) {
    fixture
        .call_token(
            &fixture.token(subject),
            "POST",
            &format!("/api/v1/vaults/{vault}/session-cleanups"),
            Some(input),
            request_id,
        )
        .await
}

#[tokio::test]
async fn cleanup_requires_current_write_access_uuid_and_a_strict_request() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Cleanup"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    let (_, shared) = fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
            Some(json!({"role":"reader","expected_revision":shared["revision"]})),
        )
        .await;
    let request_id = uuid::Uuid::new_v4().to_string();
    let input = json!({"session_id":"missing"});
    for (subject, target, expected) in [
        ("bob", vault, StatusCode::FORBIDDEN),
        ("charlie", vault, StatusCode::NOT_FOUND),
        (
            "bob",
            alice.personal_vault_id.as_str(),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            cleanup(&fixture, subject, target, Some(&request_id), input.clone())
                .await
                .0,
            expected
        );
    }
    for request_id in [None, Some("not-a-uuid")] {
        assert_eq!(
            cleanup(&fixture, "alice", vault, request_id, input.clone())
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    for input in [
        json!({"session_id":"../escape"}),
        json!({"session_id":"missing","empty":true}),
        json!({"session_id":"missing","force":true}),
        json!({"session_id":"missing","path":"sessions/missing"}),
    ] {
        assert_eq!(
            cleanup(&fixture, "alice", vault, Some(&request_id), input)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status, _) = fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
            Some(json!({"role":"editor","expected_revision":shared["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, outcome) = cleanup(&fixture, "bob", vault, Some(&request_id), input).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(outcome, json!({"kind":"not_found"}));
    let (_, capabilities) = fixture
        .call("alice", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("session_cleanup"))
    );
}

#[tokio::test]
async fn cleanup_checks_canonical_content_and_replays_without_removing_restored_data() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let vault = &alice.personal_vault_id;
    let context = fixture.service.vault(vault).await.unwrap();
    let meta = serde_json::from_value(json!({
        "id":"note", "title":"", "created_at":"2026-11-07T10:00:00Z", "tags":[]
    }))
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Keep this note")
        .await
        .unwrap();
    let directory = context.store.vault_base().join("sessions/note");
    let input = json!({"session_id":"note"});
    let retained = uuid::Uuid::new_v4().to_string();
    let (status, outcome) = cleanup(&fixture, "alice", vault, Some(&retained), input.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(outcome, json!({"kind":"not_empty"}));
    std::fs::write(directory.join("notes.md"), "").unwrap();
    assert_eq!(
        cleanup(&fixture, "alice", vault, Some(&retained), input.clone())
            .await
            .1,
        json!({"kind":"not_empty"})
    );
    assert!(directory.exists());
    let deleted = uuid::Uuid::new_v4().to_string();
    let (status, outcome) = cleanup(&fixture, "alice", vault, Some(&deleted), input.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(outcome, json!({"kind":"deleted"}));
    assert!(!directory.exists());
    assert!(context.store.restore_session("note").await.unwrap());
    context
        .store
        .write_note("note", "Restored canonical text")
        .await
        .unwrap();
    std::fs::write(directory.join("unknown.pdf"), b"User owned").unwrap();
    assert_eq!(
        cleanup(&fixture, "alice", vault, Some(&deleted), input)
            .await
            .1,
        json!({"kind":"deleted"})
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("notes.md")).unwrap(),
        "Restored canonical text"
    );
    assert_eq!(
        std::fs::read(directory.join("unknown.pdf")).unwrap(),
        b"User owned"
    );
    let (status, error) = cleanup(
        &fixture,
        "alice",
        vault,
        Some(&deleted),
        json!({"session_id":"other"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["mutation_outcome"], "not_committed");
    assert!(directory.exists());
}

#[tokio::test]
async fn native_cleanup_replays_lost_replies_at_realistic_latency_without_deleting_restored_notes()
{
    for rtt in [50, 150, 500] {
        let fixture = Fixture::new().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let first = Arc::new(AtomicBool::new(true));
        let ready = CancellationToken::new();
        let release = CancellationToken::new();
        let app = fixture.app.clone().layer(axum::middleware::from_fn({
            let ready = ready.clone();
            let release = release.clone();
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let hold = request.method() == axum::http::Method::POST
                    && request.uri().path().ends_with("/session-cleanups")
                    && first.swap(false, Ordering::SeqCst);
                let ready = ready.clone();
                let release = release.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(rtt / 2)).await;
                    let response = next.run(request).await;
                    if hold {
                        assert_eq!(response.status(), StatusCode::OK);
                        ready.cancel();
                        release.cancelled().await;
                    }
                    tokio::time::sleep(Duration::from_millis(rtt / 2)).await;
                    response
                }
            }
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = RemoteClient::discover(&origin, Arc::new(EphemeralCredentials), true)
            .await
            .unwrap();
        let login = client.begin_login().await.unwrap();
        let url = login.authorization_url().to_string();
        let complete = tokio::spawn(login.complete());
        assert!(reqwest::get(url).await.unwrap().status().is_success());
        complete.await.unwrap().unwrap();
        let RemoteReply::Me(me) = client
            .execute(
                RemoteOperation::Provision,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
        else {
            panic!("native provision response")
        };
        let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
        let meta = serde_json::from_value(json!({
            "id":"note", "title":"", "created_at":"2026-11-07T10:00:00Z", "tags":[]
        }))
        .unwrap();
        context.store.create_note_session(&meta, "").await.unwrap();
        let operation = RemoteOperation::DeleteEmptySession {
            vault_id: me.personal_vault_id.clone(),
            input: DeleteEmptySessionRequest {
                session_id: "note".into(),
            },
        };
        let request_id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let pending = tokio::spawn({
            let client = client.clone();
            let operation = operation.clone();
            let request_id = request_id.clone();
            let cancel = cancel.clone();
            async move { client.execute(operation, &request_id, cancel).await }
        });
        tokio::time::timeout(Duration::from_secs(15), ready.cancelled())
            .await
            .unwrap();
        let directory = context.store.vault_base().join("sessions/note");
        assert!(!directory.exists());
        cancel.cancel();
        let error = pending.await.unwrap().unwrap_err();
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
        release.cancel();
        assert!(context.store.restore_session("note").await.unwrap());
        context
            .store
            .write_note("note", "Restored after a lost reply")
            .await
            .unwrap();
        let RemoteReply::SessionCleanup(outcome) = client
            .execute(operation, &request_id, CancellationToken::new())
            .await
            .unwrap()
        else {
            panic!("native cleanup response")
        };
        assert_eq!(outcome, DeleteEmptySessionOutcome::Deleted);
        assert_eq!(
            std::fs::read_to_string(directory.join("notes.md")).unwrap(),
            "Restored after a lost reply"
        );
        fixture.service.shutdown().await.unwrap();
        server.abort();
    }
}

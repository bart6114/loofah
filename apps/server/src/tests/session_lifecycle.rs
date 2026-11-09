use super::*;
use hypr_loofah_client::{RemoteClient, RemoteOperation, RemoteReply};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

async fn create(fixture: &Fixture, vault: &str) -> Arc<hypr_loofah_core::VaultContext> {
    let context = fixture.service.vault(vault).await.unwrap();
    let meta=serde_json::from_value(json!({"id":"note","title":"Original","created_at":"2026-11-08T10:00:00Z","tags":[],"future":{"keep":true}})).unwrap();
    context
        .store
        .create_note_session(&meta, "Original body")
        .await
        .unwrap();
    std::fs::write(
        context.store.vault_base().join("sessions/note/unknown.bin"),
        b"user bytes",
    )
    .unwrap();
    context
}
async fn snapshot(fixture: &Fixture, subject: &str, vault: &str) -> Value {
    let (status, value) = fixture
        .call(
            subject,
            "GET",
            &format!("/api/v1/vaults/{vault}/sessions/note/lifecycle"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    value
}
async fn write(
    fixture: &Fixture,
    subject: &str,
    vault: &str,
    kind: &str,
    request: Option<&str>,
    input: Value,
) -> (StatusCode, Value) {
    fixture
        .call_token(
            &fixture.token(subject),
            "POST",
            &format!("/api/v1/vaults/{vault}/session-{kind}"),
            Some(input),
            request,
        )
        .await
}
#[tokio::test]
async fn checked_session_lifecycle_requires_access_uuid_and_strict_input_without_cross_vault_tokens()
 {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Lifecycle"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
            Some(json!({"role":"reader","expected_revision":shared["revision"]})),
        )
        .await;
    let context = create(&fixture, vault).await;
    let revision = snapshot(&fixture, "bob", vault).await["revision"].clone();
    let input = json!({"session_id":"note","expected_revision":revision});
    let id = uuid::Uuid::new_v4().to_string();
    for (subject, target, status) in [
        ("bob", vault, StatusCode::FORBIDDEN),
        ("charlie", vault, StatusCode::NOT_FOUND),
        (
            "bob",
            alice.personal_vault_id.as_str(),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            write(
                &fixture,
                subject,
                target,
                "deletions",
                Some(&id),
                input.clone()
            )
            .await
            .0,
            status
        );
    }
    for invalid in [None, Some("bad")] {
        assert_eq!(
            write(
                &fixture,
                "alice",
                vault,
                "deletions",
                invalid,
                input.clone()
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    for invalid in [
        json!({"session_id":"note","expected_revision":"bad"}),
        json!({"session_id":"../other","expected_revision":revision}),
        json!({"session_id":"note","expected_revision":revision,"force":true}),
    ] {
        assert_eq!(
            write(&fixture, "alice", vault, "deletions", Some(&id), invalid)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert!(context.store.vault_base().join("sessions/note").exists());
    let (status, deleted) = write(&fixture, "alice", vault, "deletions", Some(&id), input).await;
    assert_eq!(status, StatusCode::OK);
    let token = deleted["trash_token"].as_str().unwrap();
    assert!(is_session_lifecycle_revision(token));
    assert!(!deleted.to_string().contains("path"));
    let restore = json!({"trash_token":token,"expected_revision":revision});
    assert_eq!(
        write(
            &fixture,
            "alice",
            &alice.personal_vault_id,
            "restorations",
            Some(&uuid::Uuid::new_v4().to_string()),
            restore.clone()
        )
        .await
        .1,
        json!({"kind":"not_found"})
    );
    assert!(!context.store.vault_base().join("sessions/note").exists());
    assert_eq!(
        write(
            &fixture,
            "alice",
            vault,
            "restorations",
            Some(&uuid::Uuid::new_v4().to_string()),
            restore
        )
        .await
        .1["kind"],
        "restored"
    );
    assert_eq!(
        std::fs::read(context.store.vault_base().join("sessions/note/unknown.bin")).unwrap(),
        b"user bytes"
    );
    fixture.service.shutdown().await.unwrap();
}
#[tokio::test]
async fn checked_session_lifecycle_cas_and_historical_decisions_never_overwrite_or_recreate() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let vault = &alice.personal_vault_id;
    let context = create(&fixture, vault).await;
    let stale = snapshot(&fixture, "alice", vault).await["revision"].clone();
    context
        .store
        .write_note("note", "Newer draft")
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let (status, conflict) = write(
        &fixture,
        "alice",
        vault,
        "deletions",
        Some(&id),
        json!({"session_id":"note","expected_revision":stale}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["kind"], "conflict");
    let revision = conflict["snapshot"]["revision"].clone();
    let id = uuid::Uuid::new_v4().to_string();
    let input = json!({"session_id":"note","expected_revision":revision});
    let deleted = write(
        &fixture,
        "alice",
        vault,
        "deletions",
        Some(&id),
        input.clone(),
    )
    .await
    .1;
    let token = deleted["trash_token"].clone();
    let restore_id = uuid::Uuid::new_v4().to_string();
    let restore = json!({"trash_token":token,"expected_revision":revision});
    let restored = write(
        &fixture,
        "alice",
        vault,
        "restorations",
        Some(&restore_id),
        restore.clone(),
    )
    .await
    .1;
    assert_eq!(restored["kind"], "restored");
    context
        .store
        .write_note("note", "Restored and edited")
        .await
        .unwrap();
    assert_eq!(
        write(&fixture, "alice", vault, "deletions", Some(&id), input)
            .await
            .1,
        deleted
    );
    assert_eq!(
        std::fs::read_to_string(context.store.vault_base().join("sessions/note/notes.md")).unwrap(),
        "Restored and edited"
    );
    let current = snapshot(&fixture, "alice", vault).await["revision"].clone();
    write(
        &fixture,
        "alice",
        vault,
        "deletions",
        Some(&uuid::Uuid::new_v4().to_string()),
        json!({"session_id":"note","expected_revision":current}),
    )
    .await;
    assert_eq!(
        write(
            &fixture,
            "alice",
            vault,
            "restorations",
            Some(&restore_id),
            restore
        )
        .await
        .1,
        restored
    );
    assert!(!context.store.vault_base().join("sessions/note").exists());
    fixture.service.shutdown().await.unwrap();
}
#[tokio::test]
async fn checked_session_lifecycle_sdk_replays_lost_replies_at_50_150_500ms_without_redeleting() {
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
                    && request.uri().path().ends_with("/session-deletions")
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
            panic!()
        };
        let context = create(&fixture, &me.personal_vault_id).await;
        let RemoteReply::SessionLifecycle(snapshot) = client
            .execute(
                RemoteOperation::SessionLifecycle {
                    vault_id: me.personal_vault_id.clone(),
                    session_id: "note".into(),
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        let operation = RemoteOperation::DeleteSession {
            vault_id: me.personal_vault_id.clone(),
            input: DeleteSessionRequest {
                session_id: "note".into(),
                expected_revision: snapshot.revision.clone(),
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
        assert!(!context.store.vault_base().join("sessions/note").exists());
        cancel.cancel();
        assert_eq!(
            pending.await.unwrap().unwrap_err().mutation_outcome,
            Some(MutationOutcome::Unknown)
        );
        release.cancel();
        let RemoteReply::SessionDelete(deleted) = client
            .execute(operation.clone(), &request_id, CancellationToken::new())
            .await
            .unwrap()
        else {
            panic!()
        };
        let DeleteSessionOutcome::Trashed {
            trash_token,
            revision,
        } = deleted.clone()
        else {
            panic!()
        };
        let restored = client
            .execute(
                RemoteOperation::RestoreSession {
                    vault_id: me.personal_vault_id.clone(),
                    input: RestoreSessionRequest {
                        trash_token,
                        expected_revision: revision,
                    },
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(matches!(
            restored,
            RemoteReply::SessionRestore(RestoreSessionOutcome::Restored { .. })
        ));
        context
            .store
            .write_note("note", "Kept after a lost reply")
            .await
            .unwrap();
        let RemoteReply::SessionDelete(replayed) = client
            .execute(operation, &request_id, CancellationToken::new())
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(replayed, deleted);
        assert_eq!(
            std::fs::read_to_string(context.store.vault_base().join("sessions/note/notes.md"))
                .unwrap(),
            "Kept after a lost reply"
        );
        fixture.service.shutdown().await.unwrap();
        server.abort();
    }
}

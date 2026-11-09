use super::*;
use hypr_loofah_client::{RemoteClient, RemoteOperation, RemoteReply};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

async fn note(context: &hypr_loofah_core::VaultContext, tag: &str) {
    let meta = serde_json::from_value(json!({"id":"same","title":"Keep title","created_at":"2026-11-08T00:00:00Z","tags":[tag],"tag_suggestions":{"items":["import/audio","other"],"dismissed":[]},"future":{"keep":true}})).unwrap();
    context
        .store
        .create_note_session(&meta, "Keep note")
        .await
        .unwrap();
}

#[tokio::test]
async fn complete_tag_surface_is_reader_protected_scoped_and_returns_current_conflicts() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Tags"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
                Some(json!({"role":"reader","expected_revision":shared["revision"]}))
            )
            .await
            .0,
        StatusCode::OK
    );
    let context = fixture.service.vault(vault).await.unwrap();
    note(&context, "shared-tag").await;
    note(
        &fixture
            .service
            .vault(&alice.personal_vault_id)
            .await
            .unwrap(),
        "PRIVATE CANARY",
    )
    .await;
    let path = format!("/api/v1/vaults/{vault}");
    let before = context.read_metadata("same").await.unwrap();
    let action = json!({"name":"import/audio","expected_revision":before.revision});
    let lookup = json!({"query":"","excluded":[],"limit":8});
    let requests = [
        (
            format!("{path}/tags/suggestions/query"),
            lookup.clone(),
            false,
        ),
        (
            format!("{path}/tags/ensure"),
            json!({"name":"registry"}),
            true,
        ),
        (
            format!("{path}/sessions/same/tag-suggestions/accept"),
            action.clone(),
            true,
        ),
        (
            format!("{path}/sessions/same/tag-suggestions/dismiss"),
            action.clone(),
            true,
        ),
    ];
    for (endpoint, input, write) in &requests {
        assert_eq!(
            fixture
                .call("bob", "POST", endpoint, Some(input.clone()))
                .await
                .0,
            if *write {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
        assert_eq!(
            fixture
                .call("charlie", "POST", endpoint, Some(input.clone()))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        let personal = endpoint.replacen(vault, &alice.personal_vault_id, 1);
        assert_eq!(
            fixture
                .call("bob", "POST", &personal, Some(input.clone()))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    let (_, names) = fixture
        .call("bob", "POST", &requests[0].0, Some(lookup))
        .await;
    assert_eq!(names, json!(["shared-tag"]));
    assert_eq!(
        fixture
            .call(
                "alice",
                "POST",
                &requests[1].0,
                Some(json!({"name":" #IMPORT/manual "}))
            )
            .await
            .1,
        json!({"id":"import/manual","name":"import/manual"})
    );
    let (status, saved) = fixture
        .call("alice", "POST", &requests[2].0, Some(action.clone()))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["kind"], "saved");
    assert_eq!(
        saved["snapshot"]["meta"]["tags"],
        json!(["import/audio", "shared-tag"])
    );
    let (status, conflict) = fixture
        .call("alice", "POST", &requests[3].0, Some(action))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["kind"], "conflict");
    assert_eq!(conflict["snapshot"], saved["snapshot"]);
    assert_eq!(
        fixture
            .call(
                "alice",
                "POST",
                &requests[3].0,
                Some(json!({"name":"other","expected_revision":saved["snapshot"]["revision"]}))
            )
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        std::fs::read_to_string(context.store.vault_base().join("sessions/same/notes.md")).unwrap(),
        "Keep note"
    );
    let (_, capabilities) = fixture
        .call("alice", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("tag_editing"))
    );
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn strict_tag_requests_and_corrupt_registries_fail_without_mutating_metadata() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    note(&context, "keep").await;
    let base = format!("/api/v1/vaults/{}", me.personal_vault_id);
    let before = context.read_metadata("same").await.unwrap();
    let original =
        std::fs::read(context.store.vault_base().join("sessions/same/_meta.json")).unwrap();
    for (endpoint, input) in [
        (
            "tags/suggestions/query",
            json!({"query":"","excluded":[],"limit":8,"path":"other"}),
        ),
        ("tags/ensure", json!({"name":" "})),
        ("tags/ensure", json!({"name":"other","force":true})),
        (
            "sessions/same/tag-suggestions/accept",
            json!({"name":"other","expected_revision":""}),
        ),
        (
            "sessions/same/tag-suggestions/dismiss",
            json!({"name":"other","expected_revision":before.revision,"force":true}),
        ),
    ] {
        assert_eq!(
            fixture
                .call("alice", "POST", &format!("{base}/{endpoint}"), Some(input))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status, _) = fixture
        .call(
            "alice",
            "POST",
            &format!("{base}/tags/suggestions/query"),
            Some(json!({"query":"","excluded":[],"limit":201})),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let path = context.store.vault_base().join("tags.json");
    let corrupt = br#"{"future":{"x":1,"x":2},"tags":[]}"#;
    std::fs::write(&path, corrupt).unwrap();
    for (endpoint, input) in [
        ("tags/ensure", json!({"name":"new"})),
        (
            "sessions/same/tag-suggestions/accept",
            json!({"name":"import/audio","expected_revision":before.revision}),
        ),
    ] {
        let (status, error) = fixture
            .call("alice", "POST", &format!("{base}/{endpoint}"), Some(input))
            .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error["mutation_outcome"], "unknown");
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
        assert_eq!(
            std::fs::read(context.store.vault_base().join("sessions/same/_meta.json")).unwrap(),
            original
        );
    }
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_tag_client_lost_replies_require_read_review_and_never_overwrite_newer_metadata() {
    for rtt in [50, 150, 500] {
        let fixture = Fixture::new().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let ready = CancellationToken::new();
        let release = CancellationToken::new();
        let app = fixture.app.clone().layer(axum::middleware::from_fn({
            let calls = calls.clone();
            let ready = ready.clone();
            let release = release.clone();
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let hold = request.method() == axum::http::Method::POST
                    && request.uri().path().ends_with("/tag-suggestions/accept")
                    && calls.fetch_add(1, Ordering::SeqCst) == 0;
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
        let url = login.authorization_url().to_owned();
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
            panic!("provision")
        };
        let vault = me.personal_vault_id;
        let context = fixture.service.vault(&vault).await.unwrap();
        note(&context, "in-use").await;
        let before = context.read_metadata("same").await.unwrap();
        assert!(
            matches!(client.execute(RemoteOperation::EnsureTag{vault_id:vault.clone(),input:EnsureTagRequest{name:"registry".into()}},&uuid::Uuid::new_v4().to_string(),CancellationToken::new()).await.unwrap(),RemoteReply::Tag(tag) if tag.id=="registry")
        );
        assert!(
            matches!(client.execute(RemoteOperation::TagSuggestions{vault_id:vault.clone(),input:TagSuggestionsRequest{query:"".into(),excluded:vec!["in-use".into()],limit:8}},&uuid::Uuid::new_v4().to_string(),CancellationToken::new()).await.unwrap(),RemoteReply::TagSuggestions(names) if names==["registry"])
        );
        let operation = RemoteOperation::AcceptTagSuggestion {
            vault_id: vault.clone(),
            session_id: "same".into(),
            input: TagSuggestionActionRequest {
                name: "import/audio".into(),
                expected_revision: before.revision,
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
        cancel.cancel();
        assert_eq!(
            pending.await.unwrap().unwrap_err().mutation_outcome,
            Some(MutationOutcome::Unknown)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let saved = context.read_metadata("same").await.unwrap();
        let MetadataSaveOutcome::Saved(newer) = context
            .save_metadata(
                "same",
                SaveMetadataRequest {
                    patch: SessionMetaPatch {
                        title: Some("Newer title".into()),
                        ..Default::default()
                    },
                    expected_revision: saved.revision,
                },
            )
            .await
            .unwrap()
        else {
            panic!("metadata save")
        };
        release.cancel();
        let RemoteReply::MetadataSave(MetadataSaveOutcome::Conflict(current)) = client
            .execute(operation, &request_id, CancellationToken::new())
            .await
            .unwrap()
        else {
            panic!("typed conflict")
        };
        assert_eq!(current, newer);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(
            matches!(client.execute(RemoteOperation::DismissTagSuggestion{vault_id:vault,session_id:"same".into(),input:TagSuggestionActionRequest{name:"other".into(),expected_revision:current.revision}},&uuid::Uuid::new_v4().to_string(),CancellationToken::new()).await.unwrap(),RemoteReply::MetadataSave(MetadataSaveOutcome::Saved(snapshot)) if snapshot.meta.title=="Newer title")
        );
        client.disconnect();
        fixture.service.shutdown().await.unwrap();
        server.abort();
    }
}

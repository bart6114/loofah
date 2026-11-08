use super::*;

fn request(snapshot: &DocumentEditorSnapshot, body: &str) -> SaveDocumentEditorRequest {
    SaveDocumentEditorRequest {
        markdown: body.into(),
        title: Some("Edited session title".into()),
        tasks: vec![],
        expected_revision: snapshot.document.revision.clone(),
        expected_metadata_revision: snapshot.metadata.revision.clone(),
        expected_tasks_revision: snapshot.tasks.revision.clone(),
    }
}

#[tokio::test]
async fn document_editor_routes_enforce_roles_targets_and_all_three_revisions() {
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
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            &format!("/api/v1/vaults/{vault}/sessions"),
            Some(json!({"title":"Original","markdown":"Raw note"})),
        )
        .await;
    let session = created["meta"]["id"].as_str().unwrap();
    let context = fixture.service.vault(vault).await.unwrap();
    let directory = context.store.vault_base().join("sessions").join(session);
    let output = uuid::Uuid::new_v4().to_string();
    std::fs::create_dir_all(directory.join("enhanced")).unwrap();
    std::fs::write(
        directory.join("enhanced").join(format!("{output}.md")),
        "---\nkind: template_output\ntitle: Output label\ntemplate_id: template\nfuture: preserve\n---\nTemplate before",
    )
    .unwrap();
    std::fs::write(directory.join("unknown.md"), "Attachment canary").unwrap();
    context.store.refresh_session(session).await.unwrap();
    let root = format!("/api/v1/vaults/{vault}/sessions/{session}");
    for suffix in [
        "summary/editor".to_owned(),
        format!("outputs/{output}/editor"),
    ] {
        let path = format!("{root}/{suffix}");
        let (status, before) = fixture.call("bob", "GET", &path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {before}");
        let before: DocumentEditorSnapshot = serde_json::from_value(before).unwrap();
        let input = request(&before, "Document changed");
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(
            fixture
                .call("bob", "PUT", &path, Some(json.clone()))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fixture.call("charlie", "GET", &path, None).await.0,
            StatusCode::NOT_FOUND
        );
        let private = format!(
            "/api/v1/vaults/{}/sessions/{session}/{suffix}",
            alice.personal_vault_id
        );
        assert_eq!(
            fixture
                .call("bob", "PUT", &private, Some(json.clone()))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        let (status, saved) = fixture.call("alice", "PUT", &path, Some(json)).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        let DocumentEditorSaveOutcome::Saved(saved) = serde_json::from_value(saved).unwrap() else {
            panic!("saved")
        };
        assert_eq!(saved.metadata.meta.title, "Edited session title");
        assert_eq!(
            saved.document.document.as_ref().unwrap().markdown,
            "Document changed"
        );
        for field in [
            "expected_revision",
            "expected_metadata_revision",
            "expected_tasks_revision",
        ] {
            let mut stale = serde_json::to_value(request(&saved, "Must not overwrite")).unwrap();
            stale[field] = "stale".into();
            let (status, conflict) = fixture.call("alice", "PUT", &path, Some(stale)).await;
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(
                serde_json::from_value::<DocumentEditorSaveOutcome>(conflict).unwrap(),
                DocumentEditorSaveOutcome::Conflict(saved.clone())
            );
        }
        let mut unknown = serde_json::to_value(request(&saved, "Rejected")).unwrap();
        unknown["path"] = "/private/path".into();
        assert_eq!(
            fixture.call("alice", "PUT", &path, Some(unknown)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        std::fs::read_to_string(directory.join("notes.md")).unwrap(),
        "Raw note"
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("unknown.md")).unwrap(),
        "Attachment canary"
    );
    assert!(
        std::fs::read_to_string(directory.join("enhanced").join(format!("{output}.md")))
            .unwrap()
            .contains("future: preserve")
    );
    let missing = format!("{root}/outputs/{}/editor", uuid::Uuid::new_v4());
    assert_eq!(
        fixture.call("alice", "GET", &missing, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn document_editor_real_tcp_lost_reply_rereads_then_conflicts_at_realistic_latency() {
    for latency in [50, 150, 500] {
        let fixture = Fixture::new().await;
        let alice = fixture.login("alice").await;
        let vault = alice.personal_vault_id;
        let (_, created) = fixture
            .call(
                "alice",
                "POST",
                &format!("/api/v1/vaults/{vault}/sessions"),
                Some(json!({"title":"Before","markdown":"Raw"})),
            )
            .await;
        let session = created["meta"]["id"].as_str().unwrap();
        let (base, task) = fixture.tcp().await;
        let path = format!("/api/v1/vaults/{vault}/sessions/{session}/summary/editor");
        let (_, before) = fixture.call("alice", "GET", &path, None).await;
        let before: DocumentEditorSnapshot = serde_json::from_value(before).unwrap();
        let input = request(&before, "Committed before reply loss");
        let client = reqwest::Client::new();
        tokio::time::sleep(std::time::Duration::from_millis(latency)).await;
        let response = client
            .put(format!("{base}{path}"))
            .bearer_auth(fixture.token("alice"))
            .header("x-loofah-api-major", "1")
            .header("x-request-id", uuid::Uuid::new_v4().to_string())
            .json(&input)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        // Discard the reply body as if the connection was lost after the commit.
        drop(response);
        tokio::time::sleep(std::time::Duration::from_millis(latency)).await;
        let fresh: DocumentEditorSnapshot = client
            .get(format!("{base}{path}"))
            .bearer_auth(fixture.token("alice"))
            .header("x-loofah-api-major", "1")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(fresh.document.document.unwrap().markdown, input.markdown);
        assert_eq!(
            fixture
                .call(
                    "alice",
                    "PUT",
                    &path,
                    Some(serde_json::to_value(input).unwrap())
                )
                .await
                .0,
            StatusCode::CONFLICT
        );
        fixture.service.stop.cancel();
        task.await.unwrap();
    }
}

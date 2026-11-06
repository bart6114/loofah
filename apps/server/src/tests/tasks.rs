use super::*;

#[tokio::test]
async fn task_replacements_are_revision_checked_reader_protected_and_isolated_from_other_vaults() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Tasks"})),
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
    let base = format!("/api/v1/vaults/{id}/sessions");
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            &base,
            Some(json!({"title":"Note","markdown":"User content"})),
        )
        .await;
    let session = created["meta"]["id"].as_str().unwrap();
    let path = format!("{base}/{session}/tasks");
    let snapshot_path = format!("{path}/snapshot");
    let context = fixture.service.vault(id).await.unwrap();
    let directory = context.store.vault_base().join("sessions").join(session);
    std::fs::write(directory.join("unknown.txt"), "ATTACHMENT CANARY").unwrap();
    std::fs::write(directory.join(".hidden"), "HIDDEN CANARY").unwrap();
    let before_meta = std::fs::read(directory.join("_meta.json")).unwrap();
    let (status, before) = fixture.call("bob", "GET", &snapshot_path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        before,
        serde_json::to_value(context.read_tasks_snapshot(session).await.unwrap()).unwrap()
    );
    assert!(!directory.join("tasks.json").exists());
    let input = json!({"source":{"kind":"raw_note"},"expected_revision":before["revision"],"tasks":[{"id":"task","source_order":0,"status":"done","text":"Task","body":[],"due_at":""}]});
    assert_eq!(
        fixture
            .call("bob", "PUT", &path, Some(input.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for (subject, vault) in [("charlie", id), ("bob", alice.personal_vault_id.as_str())] {
        let other = format!("/api/v1/vaults/{vault}/sessions/{session}/tasks");
        assert_eq!(
            fixture
                .call(subject, "GET", &format!("{other}/snapshot"), None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            fixture
                .call(subject, "PUT", &other, Some(input.clone()))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    let (status, saved) = fixture
        .call("alice", "PUT", &path, Some(input.clone()))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["kind"], "saved");
    assert_eq!(saved["snapshot"]["tasks"][0]["status"], "done");
    let (status, conflict) = fixture.call("alice", "PUT", &path, Some(input)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["snapshot"], saved["snapshot"]);
    let (status, tasks) = fixture.call("bob", "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tasks, saved["snapshot"]["tasks"]);
    let before_tasks = std::fs::read(directory.join("tasks.json")).unwrap();
    let duplicate = json!({"source":{"kind":"raw_note"},"expected_revision":saved["snapshot"]["revision"],"tasks":[{"id":"same","source_order":0,"status":"todo","text":"One","body":[]},{"id":"same","source_order":1,"status":"todo","text":"Two","body":[]}]});
    assert_eq!(
        fixture.call("alice", "PUT", &path, Some(duplicate)).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        std::fs::read(directory.join("tasks.json")).unwrap(),
        before_tasks
    );
    assert_eq!(
        std::fs::read(directory.join("_meta.json")).unwrap(),
        before_meta
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("notes.md")).unwrap(),
        "User content"
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("unknown.txt")).unwrap(),
        "ATTACHMENT CANARY"
    );
    assert_eq!(
        std::fs::read_to_string(directory.join(".hidden")).unwrap(),
        "HIDDEN CANARY"
    );
    fixture.service.shutdown().await.unwrap();
}

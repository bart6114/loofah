use super::*;

#[tokio::test]
async fn compound_editor_writes_check_roles_and_both_revisions_without_touching_other_vaults() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Editor"})),
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
    let base = format!("/api/v1/vaults/{vault}/sessions");
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            &base,
            Some(json!({"title":"Title","markdown":"- [ ] Task"})),
        )
        .await;
    let session = created["meta"]["id"].as_str().unwrap();
    let path = format!("{base}/{session}/note/editor");
    let (status, before) = fixture.call("bob", "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    let before: NoteEditorSnapshot = serde_json::from_value(before).unwrap();
    let id = before.tasks.tasks[0].id.clone();
    let input = SaveNoteEditorRequest {
        markdown: "- [ ] Renamed task".into(),
        title: Some("Renamed".into()),
        expected_revision: before.note.revision,
        expected_tasks_revision: before.tasks.revision,
        tasks: vec![TaskInput {
            id: id.clone(),
            source_order: 0,
            status: "in_progress".into(),
            text: "Renamed task".into(),
            body: json!([{"type":"paragraph","content":[{"type":"text","text":"Renamed task"}]}]),
            due_at: "2026-12-01".into(),
        }],
    };
    let directory = fixture
        .service
        .vault(vault)
        .await
        .unwrap()
        .store
        .vault_base()
        .join("sessions")
        .join(session);
    std::fs::write(directory.join("unknown.md"), "Attachment canary").unwrap();
    std::fs::write(directory.join(".hidden"), "Private canary").unwrap();
    assert_eq!(
        fixture
            .call(
                "bob",
                "PUT",
                &path,
                Some(serde_json::to_value(&input).unwrap())
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for (subject, vault) in [
        ("charlie", vault),
        ("bob", alice.personal_vault_id.as_str()),
    ] {
        let other = format!("/api/v1/vaults/{vault}/sessions/{session}/note/editor");
        assert_eq!(
            fixture
                .call(
                    subject,
                    "PUT",
                    &other,
                    Some(serde_json::to_value(&input).unwrap())
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    let (status, saved) = fixture
        .call(
            "alice",
            "PUT",
            &path,
            Some(serde_json::to_value(&input).unwrap()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let NoteEditorSaveOutcome::Saved(saved) = serde_json::from_value(saved).unwrap() else {
        panic!("expected saved editor")
    };
    assert_eq!(saved.tasks.tasks[0].id, id);
    assert_eq!(saved.tasks.tasks[0].status, "in_progress");
    assert_eq!(saved.tasks.tasks[0].due_at, "2026-12-01");
    let mut task = input.tasks[0].clone();
    task.status = "done".into();
    let change = ReplaceTasksRequest {
        source: TaskSource::RawNote,
        tasks: vec![task],
        expected_revision: saved.tasks.revision.clone(),
    };
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &format!("{base}/{session}/tasks"),
                Some(serde_json::to_value(change).unwrap())
            )
            .await
            .0,
        StatusCode::OK
    );
    let mut stale = input;
    stale.expected_revision = saved.note.revision.clone();
    stale.expected_tasks_revision = saved.tasks.revision;
    stale.markdown = "Never overwrite a stale editor".into();
    let (status, conflict) = fixture
        .call(
            "alice",
            "PUT",
            &path,
            Some(serde_json::to_value(stale).unwrap()),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let NoteEditorSaveOutcome::Conflict(current) = serde_json::from_value(conflict).unwrap() else {
        panic!("expected editor conflict")
    };
    assert_eq!(current.note, saved.note);
    assert_eq!(current.tasks.tasks[0].status, "done");
    assert_eq!(
        std::fs::read_to_string(directory.join("unknown.md")).unwrap(),
        "Attachment canary"
    );
    assert_eq!(
        std::fs::read_to_string(directory.join(".hidden")).unwrap(),
        "Private canary"
    );
    fixture.service.shutdown().await.unwrap();
}

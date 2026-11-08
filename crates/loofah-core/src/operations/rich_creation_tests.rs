use super::*;
use std::time::Duration;

fn fixture_context(root: &Path, limits: hypr_vault_write::ContentLimits) -> Arc<VaultContext> {
    let vault = root.join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    Arc::new(
        VaultContext::new(
            "vault".into(),
            Arc::new(SessionStore::with_content_limits(vault.clone(), limits)),
            crate::startup::StartupState::new(Some(&vault)),
            Arc::new(crate::settings::ConfigState::load_or_default(&vault)),
            Arc::new(crate::search::SearchService::default()),
        )
        .with_operation_storage(root.join("operations")),
    )
}
fn input() -> (CreateSessionRequest, InitialSessionMetadata) {
    (
        CreateSessionRequest {
            title: "CLI title".into(),
            markdown: "# Independent body heading\n\n- [ ] Keep original task".into(),
        },
        InitialSessionMetadata {
            created_at: Some("2024-05-01T11:00:00.123+02:00".into()),
            started_at: Some("2024-05-01T11:01:00+02:00".into()),
            ended_at: Some("2024-05-01T11:30:00+02:00".into()),
            tags: vec!["#Project-X".into(), "Other".into(), "#Project-X".into()],
            author: Some("claude-code".into()),
            skill: Some("meeting-summarizer".into()),
        },
    )
}
async fn create(
    context: &Arc<VaultContext>,
    id: uuid::Uuid,
    request: &CreateSessionRequest,
    metadata: &InitialSessionMetadata,
) -> Result<SessionMeta, ApplicationError> {
    context
        .create_session_with_metadata(
            "local".into(),
            id.to_string(),
            request.clone(),
            metadata.clone(),
        )
        .await
}
fn path(context: &VaultContext, id: uuid::Uuid) -> PathBuf {
    receipt_path(
        &context.operations.as_ref().unwrap().root,
        &creation_identity("local", id).1,
    )
}
fn bytes(path: impl AsRef<Path>) -> Vec<u8> {
    std::fs::read(path).unwrap()
}
fn json(path: impl AsRef<Path>) -> serde_json::Value {
    serde_json::from_slice(&bytes(path)).unwrap()
}

#[tokio::test]
async fn rich_creation_commits_original_metadata_body_tasks_and_preserved_registry_once() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let context = fixture_context(&root, Default::default());
    let id = uuid::Uuid::new_v4();
    let (request, metadata) = input();
    assert_eq!(
        context
            .created_session_id("local", &id.to_string())
            .unwrap(),
        creation_identity("local", id).1
    );
    assert_eq!(
        context
            .created_session_id("local", "invalid")
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert!(!path(&context, id).exists());
    let tags = context.store.vault_base().join("tags.json");
    std::fs::write(&tags,br##"{"tags":[{"id":"project-x","name":"Existing label","future":{"keep":true}}],"future":{"top":[1,2]}}"##).unwrap();
    let meta = create(&context, id, &request, &metadata).await.unwrap();
    assert_eq!(meta.id, creation_identity("local", id).1);
    assert_eq!(meta.title, request.title);
    assert_eq!(meta.created_at, "2024-05-01T09:00:00.123Z");
    assert_eq!(meta.started_at.as_deref(), Some("2024-05-01T09:01:00.000Z"));
    assert_eq!(meta.ended_at.as_deref(), Some("2024-05-01T09:30:00.000Z"));
    assert_eq!(meta.tags, metadata.tags);
    assert_eq!(meta.author, metadata.author);
    assert_eq!(meta.skill, metadata.skill);
    let directory = context.store.vault_base().join("sessions").join(&meta.id);
    assert_eq!(
        bytes(directory.join("notes.md")),
        request.markdown.as_bytes()
    );
    let stored_tags = json(&tags);
    assert_eq!(stored_tags["tags"].as_array().unwrap().len(), 2);
    assert_eq!(stored_tags["future"], serde_json::json!({"top":[1,2]}));
    assert_eq!(
        stored_tags["tags"][0]["future"],
        serde_json::json!({"keep":true})
    );
    assert_eq!(stored_tags["tags"][0]["name"], "Existing label");
    let task_bytes = bytes(directory.join("tasks.json"));
    assert_eq!(
        json(directory.join("tasks.json"))["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let before = [
        bytes(directory.join("_meta.json")),
        bytes(directory.join("notes.md")),
        bytes(&tags),
        bytes(path(&context, id)),
    ];
    assert_eq!(
        create(&context, id, &request, &metadata).await.unwrap(),
        meta
    );
    assert_eq!(
        before,
        [
            bytes(directory.join("_meta.json")),
            bytes(directory.join("notes.md")),
            bytes(&tags),
            bytes(path(&context, id))
        ]
    );
    assert_eq!(bytes(directory.join("tasks.json")), task_bytes);
}

#[tokio::test]
async fn rich_request_replays_reject_each_changed_field_without_touching_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let context = fixture_context(&root, Default::default());
    let id = uuid::Uuid::new_v4();
    let (request, metadata) = input();
    let meta = create(&context, id, &request, &metadata).await.unwrap();
    let directory = context.store.vault_base().join("sessions").join(&meta.id);
    let receipt = path(&context, id);
    let before = [
        bytes(directory.join("_meta.json")),
        bytes(directory.join("notes.md")),
        bytes(&receipt),
        bytes(context.store.vault_base().join("tags.json")),
    ];
    for field in [
        "title", "body", "created", "started", "ended", "tags", "author", "skill",
    ] {
        let mut request = request.clone();
        let mut metadata = metadata.clone();
        match field {
            "title" => request.title.push('x'),
            "body" => request.markdown.push('x'),
            "created" => metadata.created_at = Some("2020-01-01T00:00:00Z".into()),
            "started" => metadata.started_at = None,
            "ended" => metadata.ended_at = None,
            "tags" => metadata.tags.push("new".into()),
            "author" => metadata.author = Some("another".into()),
            "skill" => metadata.skill = Some("another".into()),
            _ => unreachable!(),
        }
        assert_eq!(
            create(&context, id, &request, &metadata)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Conflict,
            "{field}"
        );
        assert_eq!(
            before,
            [
                bytes(directory.join("_meta.json")),
                bytes(directory.join("notes.md")),
                bytes(&receipt),
                bytes(context.store.vault_base().join("tags.json"))
            ]
        );
    }
}

#[tokio::test]
async fn registry_failure_retains_one_pending_decision_and_restart_preserves_current_user_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let context = fixture_context(&root, Default::default());
    let id = uuid::Uuid::new_v4();
    let (request, metadata) = input();
    let (_, session) = creation_identity("local", id);
    std::fs::write(
        context.store.vault_base().join("tags.json"),
        br#"{"tags":"unsupported","future":true}"#,
    )
    .unwrap();
    let error = create(&context, id, &request, &metadata).await.unwrap_err();
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::Persisted));
    assert!(error.message.contains(&session));
    let receipt = path(&context, id);
    assert_eq!(json(&receipt)["files_created"], true);
    assert_eq!(json(&receipt)["completed"], false);
    let exclusive = context.try_session_lifecycle_change(&session).unwrap();
    assert!(
        context
            .ensure_capture_session_lifecycle_idle(&session)
            .await
            .is_err()
    );
    drop(exclusive);
    let directory = context.store.vault_base().join("sessions").join(&session);
    let mut actual = json(directory.join("_meta.json"));
    actual["title"] = serde_json::json!("User title");
    actual["future"] = serde_json::json!({"keep":[true,null]});
    std::fs::write(
        directory.join("_meta.json"),
        serde_json::to_vec(&actual).unwrap(),
    )
    .unwrap();
    std::fs::write(directory.join("notes.md"), b"New user content").unwrap();
    std::fs::write(directory.join("unknown.pdf"), b"user attachment").unwrap();
    std::fs::write(
        context.store.vault_base().join("tags.json"),
        br#"{"tags":[],"future":{"keep":true}}"#,
    )
    .unwrap();
    drop(context);
    let reopened = fixture_context(&root, Default::default());
    let meta = create(&reopened, id, &request, &metadata).await.unwrap();
    assert_eq!(meta.title, request.title);
    assert_eq!(meta.id, session);
    assert_eq!(json(directory.join("_meta.json")), actual);
    assert_eq!(bytes(directory.join("notes.md")), b"New user content");
    assert_eq!(bytes(directory.join("unknown.pdf")), b"user attachment");
    assert_eq!(json(&receipt)["completed"], true);
    let exclusive = reopened.try_session_lifecycle_change(&session).unwrap();
    reopened
        .ensure_capture_session_lifecycle_idle(&session)
        .await
        .unwrap();
    drop(exclusive);
}

#[tokio::test]
async fn completed_rich_decisions_do_not_resurrect_notes_or_registry_on_any_api_replay() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let context = fixture_context(&root, Default::default());
    let id = uuid::Uuid::new_v4();
    let (request, metadata) = input();
    let meta = create(&context, id, &request, &metadata).await.unwrap();
    let directory = context.store.vault_base().join("sessions").join(&meta.id);
    std::fs::write(
        context.store.vault_base().join("tags.json"),
        br#"{"tags":[],"future":true}"#,
    )
    .unwrap();
    let registry = bytes(context.store.vault_base().join("tags.json"));
    assert_eq!(
        create(&context, id, &request, &metadata).await.unwrap(),
        meta
    );
    assert_eq!(
        bytes(context.store.vault_base().join("tags.json")),
        registry
    );
    std::fs::rename(&directory, root.join("user-removed-note")).unwrap();
    assert_eq!(
        create(&context, id, &request, &metadata)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        context
            .create_session("local".into(), id.to_string(), request.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert!(!directory.exists());
    assert_eq!(
        bytes(context.store.vault_base().join("tags.json")),
        registry
    );
}

#[tokio::test]
async fn rich_preflight_rejects_invalid_values_and_all_declared_limits_before_any_intent() {
    for kind in [
        "date",
        "skill",
        "tag",
        "body_limit",
        "title_limit",
        "metadata_limit",
        "receipt_limit",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut limits = hypr_vault_write::ContentLimits::default();
        let (mut request, mut metadata) = input();
        match kind {
            "date" => metadata.created_at = Some("invalid".into()),
            "skill" => metadata.author = None,
            "tag" => metadata.tags = vec!["###".into()],
            "body_limit" => limits.note_write_bytes = 1,
            "title_limit" => limits.title_bytes = 1,
            "metadata_limit" => limits.metadata_bytes = 1,
            "receipt_limit" => limits.recovery_bytes = 1,
            _ => unreachable!(),
        }
        request.markdown.push('\n');
        let context = fixture_context(&root, limits);
        let id = uuid::Uuid::new_v4();
        let (_, session) = creation_identity("local", id);
        assert!(
            create(&context, id, &request, &metadata).await.is_err(),
            "{kind}"
        );
        assert!(!path(&context, id).exists());
        assert!(
            !context
                .store
                .vault_base()
                .join("sessions")
                .join(session)
                .exists()
        );
        assert!(!context.store.vault_base().join("tags.json").exists());
    }
}

#[tokio::test]
async fn rich_default_reader_preserves_version_one_receipt_bytes_and_original_capture_clock() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let context = fixture_context(&root, Default::default());
    for capture in [false, true] {
        let id = uuid::Uuid::new_v4();
        let request = if capture {
            CreateSessionRequest {
                title: String::new(),
                markdown: String::new(),
            }
        } else {
            CreateSessionRequest {
                title: "Legacy".into(),
                markdown: "Legacy body".into(),
            }
        };
        let accepted = if capture {
            context
                .create_capture_session("local".into(), id, 1_794_000_000_123)
                .await
                .unwrap()
        } else {
            context
                .create_session("local".into(), id.to_string(), request.clone())
                .await
                .unwrap()
        };
        let (key, _) = creation_identity("local", id);
        let legacy = context
            .operations
            .as_ref()
            .unwrap()
            .root
            .join(format!("{key}.json"));
        let before = bytes(&legacy);
        let meta = create(&context, id, &request, &InitialSessionMetadata::default())
            .await
            .unwrap();
        assert_eq!(meta, accepted.meta);
        assert_eq!(bytes(&legacy), before);
        assert!(!path(&context, id).exists());
        let mut invalid = InitialSessionMetadata::default();
        invalid.author = Some("new".into());
        assert_eq!(
            create(&context, id, &request, &invalid)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(bytes(&legacy), before);
    }
}

#[tokio::test]
async fn rich_invalid_future_duplicate_and_tampered_receipts_are_preserved_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let context = fixture_context(&root, Default::default());
    let id = uuid::Uuid::new_v4();
    let (request, metadata) = input();
    let meta = create(&context, id, &request, &metadata).await.unwrap();
    let receipt = path(&context, id);
    let original = json(&receipt);
    let directory = context.store.vault_base().join("sessions").join(&meta.id);
    let before = [
        bytes(directory.join("_meta.json")),
        bytes(directory.join("notes.md")),
        bytes(directory.join("tasks.json")),
    ];
    for kind in [
        "future",
        "unknown",
        "duplicate",
        "binding",
        "foreign_id",
        "frozen_unknown",
        "state",
        "truncated",
    ] {
        let mut value = original.clone();
        match kind {
            "future" => value["schema_version"] = serde_json::json!(3),
            "unknown" => value["future"] = serde_json::json!({"keep":true}),
            "binding" => value["operation_key"] = serde_json::json!("0".repeat(64)),
            "foreign_id" => {
                value["frozen_meta"]["id"] = serde_json::json!(uuid::Uuid::new_v4().to_string())
            }
            "frozen_unknown" => value["frozen_meta"]["future"] = serde_json::json!(true),
            "state" => value["files_created"] = serde_json::json!(false),
            _ => {}
        }
        let mut body = serde_json::to_vec(&value).unwrap();
        if kind == "duplicate" {
            body.splice(1..1, b"\"completed\":true,".iter().copied());
        }
        if kind == "truncated" {
            body = b"{broken".to_vec();
        }
        std::fs::write(&receipt, &body).unwrap();
        assert!(
            create(&context, id, &request, &metadata).await.is_err(),
            "{kind}"
        );
        assert_eq!(bytes(&receipt), body);
        assert_eq!(
            before,
            [
                bytes(directory.join("_meta.json")),
                bytes(directory.join("notes.md")),
                bytes(directory.join("tasks.json"))
            ]
        );
    }
}

#[tokio::test]
async fn pending_missing_evidence_requires_review_while_exact_partial_note_can_finish_once() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let context = fixture_context(&root, Default::default());
    let id = uuid::Uuid::new_v4();
    let (request, metadata) = input();
    let meta = create(&context, id, &request, &metadata).await.unwrap();
    let receipt = path(&context, id);
    let mut pending = json(&receipt);
    pending["completed"] = serde_json::json!(false);
    std::fs::write(&receipt, serde_json::to_vec(&pending).unwrap()).unwrap();
    let directory = context.store.vault_base().join("sessions").join(&meta.id);
    std::fs::rename(&directory, root.join("removed-note")).unwrap();
    assert_eq!(
        create(&context, id, &request, &metadata)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert!(!directory.exists());
    pending["files_created"] = serde_json::json!(false);
    std::fs::write(&receipt, serde_json::to_vec(&pending).unwrap()).unwrap();
    assert_eq!(
        create(&context, id, &request, &metadata)
            .await
            .unwrap_err()
            .mutation_outcome,
        Some(MutationOutcome::Unknown)
    );
    assert!(!directory.exists());
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("notes.md"), &request.markdown).unwrap();
    std::fs::write(directory.join("unknown.pdf"), b"keep attachment").unwrap();
    assert_eq!(
        create(&context, id, &request, &metadata).await.unwrap(),
        meta
    );
    assert_eq!(bytes(directory.join("unknown.pdf")), b"keep attachment");
    assert_eq!(
        bytes(directory.join("notes.md")),
        request.markdown.as_bytes()
    );
}

#[tokio::test]
async fn dropped_rich_caller_retains_owned_lifecycle_admission_until_all_declared_fields_commit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let context = fixture_context(&root, Default::default());
    let id = uuid::Uuid::new_v4();
    let (request, metadata) = input();
    let (_, session) = creation_identity("local", id);
    let receipt = path(&context, id);
    let held = context.store.lock_snapshot().await;
    let owned = context.clone();
    let body = request.clone();
    let initial = metadata.clone();
    let caller = tokio::spawn(async move { create(&owned, id, &body, &initial).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !receipt.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(context.try_session_lifecycle_change(&session).is_err());
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(context.try_session_lifecycle_change(&session).is_err());
    drop(held);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read(&receipt)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .is_some_and(|value| value["completed"] == true)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let meta = context.store.read_meta(&session).await.unwrap().unwrap();
    assert_eq!(meta.tags, metadata.tags);
    assert_eq!(meta.author, metadata.author);
    assert_eq!(meta.skill, metadata.skill);
    assert_eq!(
        context.store.read_note(&session).await.unwrap().as_deref(),
        Some(request.markdown.as_str())
    );
}

#[tokio::test]
async fn rich_local_capacity_preserves_large_body_and_unknown_registry_without_remote_limits() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let context = fixture_context(&root, hypr_vault_write::ContentLimits::unlimited());
    let registry = serde_json::json!({"tags":[],"future":"x".repeat(9*1024*1024)});
    let tag_path = context.store.vault_base().join("tags.json");
    std::fs::write(&tag_path, serde_json::to_vec(&registry).unwrap()).unwrap();
    let request = CreateSessionRequest {
        title: "Large local".into(),
        markdown: "x".repeat(4 * 1024 * 1024 + 1),
    };
    let metadata = InitialSessionMetadata {
        tags: vec!["Local".into()],
        ..Default::default()
    };
    let meta = create(&context, uuid::Uuid::new_v4(), &request, &metadata)
        .await
        .unwrap();
    assert_eq!(
        context.store.read_note(&meta.id).await.unwrap().as_deref(),
        Some(request.markdown.as_str())
    );
    assert_eq!(json(&tag_path)["future"], registry["future"]);
    assert_eq!(json(&tag_path)["tags"][0]["id"], "local");
}

#[cfg(unix)]
#[tokio::test]
async fn rich_receipt_links_are_never_followed_or_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let context = fixture_context(&root, Default::default());
    let id = uuid::Uuid::new_v4();
    let (request, metadata) = input();
    create(&context, id, &request, &metadata).await.unwrap();
    let receipt = path(&context, id);
    let saved = bytes(&receipt);
    let backing = root.join("untouched-receipt");
    std::fs::rename(&receipt, &backing).unwrap();
    std::os::unix::fs::symlink(&backing, &receipt).unwrap();
    assert!(create(&context, id, &request, &metadata).await.is_err());
    assert_eq!(bytes(&backing), saved);
    assert!(
        std::fs::symlink_metadata(receipt)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

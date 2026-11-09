use super::*;
use crate::drafts::tests::recovery;

fn document(scope: &str, target: DocumentTarget) -> NoteDraftRecovery {
    let session_id = "owner";
    let (source_type, source_id) = match &target {
        DocumentTarget::Summary => ("session_summary", session_id),
        DocumentTarget::TemplateOutput { document_id } => ("enhanced_note", document_id.as_str()),
    };
    let task = serde_json::from_value(serde_json::json!({
        "id":"stable", "source_type":source_type,"source_id":source_id,"source_order":0,
        "status":"todo","text":"Original task","body":[{"type":"paragraph","content":[{"type":"text","text":"Keep body"}]}],
        "due_at":"2027-01-01","assignee":"owner","created_at":"2026-11-08T00:00:00Z","updated_at":"2026-11-08T00:00:00Z"
    })).unwrap();
    let mut value = recovery(
        scope,
        &coordinate(session_id, &target),
        "Unsaved document body",
    );
    value.baseline.title = "Session title".into();
    value.draft.title = "Unsaved session title".into();
    value.editor_tasks = Some(EditorTaskRecovery {
        baseline: TasksSnapshot {
            tasks: vec![task],
            revision: "task-baseline".into(),
        },
        draft: vec![TaskInput {
            id: "stable".into(),
            source_order: 0,
            status: "in_progress".into(),
            text: "Unsaved task".into(),
            body: serde_json::json!([{"type":"paragraph","content":[{"type":"session","attrs":{"id":"related"}}]}]),
            due_at: "2027-02-01".into(),
        }],
    });
    value.document_editor = Some(DocumentEditorRecovery {
        session_id: session_id.into(), target,
        metadata: MetadataSnapshot { meta:serde_json::from_value(serde_json::json!({"id":session_id,"title":"Session title","created_at":"2026-11-08T00:00:00Z","tags":["keep"]})).unwrap(), revision:"metadata-baseline".into() },
    });
    value
}

#[tokio::test]
async fn compound_document_recovery_survives_restart_and_inventory_without_duplicating_body_or_task_baselines()
 {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("cache");
    let cache = Arc::new(DraftCache::new(directory.clone()).unwrap());
    let summary = document("alice:vault", DocumentTarget::Summary);
    let output = document(
        "alice:vault",
        DocumentTarget::TemplateOutput {
            document_id: "manual-output".into(),
        },
    );
    for value in [&summary, &output] {
        cache
            .put("closed-window".into(), value.clone())
            .await
            .unwrap();
        let bytes =
            std::fs::read(cache.path("closed-window", &value.scope, &value.session_id)).unwrap();
        let stored: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stored["schema_version"], 3);
        let extra = stored["recovery"]["documentEditor"].as_object().unwrap();
        assert_eq!(extra.len(), 3);
        assert!(!extra.contains_key("document"));
        assert!(!extra.contains_key("tasks"));
        assert!(stored["recovery"]["editorTasks"].is_object());
    }
    drop(cache);
    let cache = Arc::new(DraftCache::new(directory).unwrap());
    for value in [&summary, &output] {
        assert_eq!(
            cache
                .get(
                    "closed-window".into(),
                    value.scope.clone(),
                    value.session_id.clone()
                )
                .await
                .unwrap(),
            Some(value.clone())
        );
    }
    let inventory = cache.inventory(summary.scope.clone()).await.unwrap();
    assert_eq!(inventory.len(), 2);
    for entry in inventory {
        let restored = cache
            .recovery(summary.scope.clone(), entry.id)
            .await
            .unwrap()
            .unwrap();
        assert!(restored.document_editor.is_some());
        assert_eq!(restored.baseline.title, "Session title");
        assert_eq!(restored.draft.title, "Unsaved session title");
        assert_eq!(restored.editor_tasks.unwrap().draft[0].due_at, "2027-02-01");
    }
}

#[tokio::test]
async fn legacy_records_keep_their_versions_and_bytes_when_read_by_the_document_capable_cache() {
    let root = tempfile::tempdir().unwrap();
    let cache = Arc::new(DraftCache::new(root.path().join("cache")).unwrap());
    let plain = recovery("alice", "body-only", "Unsaved");
    let mut raw_editor = recovery("alice", "[\"note_editor\",\"owner\"]", "Unsaved");
    raw_editor.editor_tasks = document("alice", DocumentTarget::Summary).editor_tasks;
    for (value, version) in [(plain, 1), (raw_editor, 2)] {
        cache.put("main".into(), value.clone()).await.unwrap();
        let path = cache.path("main", &value.scope, &value.session_id);
        let bytes = std::fs::read(&path).unwrap();
        let stored: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stored["schema_version"], version);
        assert!(stored["recovery"].get("documentEditor").is_none());
        assert_eq!(
            cache
                .get("main".into(), value.scope.clone(), value.session_id.clone())
                .await
                .unwrap(),
            Some(value)
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn document_records_refuse_wrong_editor_downgrade_and_cross_target_baselines_without_touching_saved_bytes()
 {
    let root = tempfile::tempdir().unwrap();
    let cache = Arc::new(DraftCache::new(root.path().join("cache")).unwrap());
    let original = document("alice", DocumentTarget::Summary);
    cache.put("main".into(), original.clone()).await.unwrap();
    let path = cache.path("main", &original.scope, &original.session_id);
    let bytes = std::fs::read(&path).unwrap();
    let mut values = Vec::new();
    let mut value = original.clone();
    value.document_editor = None;
    values.push(value);
    let mut value = original.clone();
    value.editor_tasks = None;
    values.push(value);
    let mut value = original.clone();
    value.document_editor.as_mut().unwrap().session_id = "another-session".into();
    values.push(value);
    let mut value = original.clone();
    value.document_editor.as_mut().unwrap().target = DocumentTarget::TemplateOutput {
        document_id: "another-output".into(),
    };
    values.push(value);
    let mut value = original.clone();
    value.document_editor.as_mut().unwrap().metadata.meta.id = "another-session".into();
    values.push(value);
    let mut value = original.clone();
    value.baseline.title = "Output frontmatter title is not the session title".into();
    values.push(value);
    let mut value = original.clone();
    value.document_editor.as_mut().unwrap().metadata.revision = "".into();
    values.push(value);
    let mut value = original.clone();
    value.editor_tasks.as_mut().unwrap().baseline.tasks[0].source_type = "enhanced_note".into();
    values.push(value);
    let mut value = original.clone();
    value.editor_tasks.as_mut().unwrap().baseline.tasks[0].source_id = "foreign".into();
    values.push(value);
    for value in values {
        assert!(cache.put("main".into(), value).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    assert_eq!(
        cache
            .get(
                "main".into(),
                original.scope.clone(),
                original.session_id.clone()
            )
            .await
            .unwrap(),
        Some(original)
    );
}

#[tokio::test]
async fn future_malformed_and_duplicate_known_keys_are_never_normalized_overwritten_or_removed() {
    let root = tempfile::tempdir().unwrap();
    let cache = Arc::new(DraftCache::new(root.path().join("cache")).unwrap());
    let original = document(
        "alice",
        DocumentTarget::TemplateOutput {
            document_id: "manual-output".into(),
        },
    );
    cache.put("main".into(), original.clone()).await.unwrap();
    let path = cache.path("main", &original.scope, &original.session_id);
    let bytes = std::fs::read_to_string(&path).unwrap();
    let mut bad_schema: serde_json::Value = serde_json::from_str(&bytes).unwrap();
    bad_schema["schema_version"] = serde_json::json!(2);
    let mut no_tasks: serde_json::Value = serde_json::from_str(&bytes).unwrap();
    no_tasks["recovery"]
        .as_object_mut()
        .unwrap()
        .remove("editorTasks");
    let variants = [
        bytes.replacen("\"schema_version\":3", "\"schema_version\":4", 1),
        bytes.replacen(
            "\"schema_version\":3",
            "\"schema_version\":4,\"schema_version\":3",
            1,
        ),
        bytes.replacen(
            "\"documentEditor\":",
            "\"documentEditor\":null,\"documentEditor\":",
            1,
        ),
        bytes.replacen(
            "\"revision\":\"baseline\"",
            "\"revision\":\"foreign\",\"revision\":\"baseline\"",
            1,
        ),
        serde_json::to_string(&bad_schema).unwrap(),
        serde_json::to_string(&no_tasks).unwrap(),
    ];
    for data in variants {
        assert_ne!(data, bytes);
        std::fs::write(&path, &data).unwrap();
        assert!(
            cache
                .get(
                    "main".into(),
                    original.scope.clone(),
                    original.session_id.clone()
                )
                .await
                .is_err()
        );
        assert!(cache.inventory(original.scope.clone()).await.is_err());
        assert!(cache.put("main".into(), original.clone()).await.is_err());
        assert!(
            cache
                .remove(
                    "main".into(),
                    original.scope.clone(),
                    original.session_id.clone()
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), data);
    }
}

#[tokio::test]
async fn bounded_document_recovery_rejects_before_replacing_a_valid_copy() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("cache");
    let cache = Arc::new(DraftCache::new(directory.clone()).unwrap());
    let original = document("alice", DocumentTarget::Summary);
    cache.put("main".into(), original.clone()).await.unwrap();
    let path = cache.path("main", &original.scope, &original.session_id);
    let bytes = std::fs::read(&path).unwrap();
    let mut bounded = DraftCache::new(directory).unwrap();
    bounded.record_limit = bytes.len() + 10;
    let bounded = Arc::new(bounded);
    let mut larger = original.clone();
    larger.draft.markdown = "large".repeat(bytes.len());
    assert_eq!(
        bounded.put("main".into(), larger).await.unwrap_err().code,
        ErrorCode::ResourceLimit
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        bounded
            .get(
                "main".into(),
                original.scope.clone(),
                original.session_id.clone()
            )
            .await
            .unwrap(),
        Some(original)
    );
}

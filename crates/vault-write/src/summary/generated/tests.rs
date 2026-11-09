use super::*;
use crate::TaskInput;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const BEFORE: &str = "# Before\n- [ ] Follow up";
const GENERATED: &str = "# Decisions\n- [ ] Follow up\n- [ ] Send agenda";
const PUBLISHED: &str = "# Keep title\n\n# Decisions\n- [ ] Follow up\n- [ ] Send agenda";

async fn fixture() -> (tempfile::TempDir, tempfile::TempDir, SessionStore) {
    let vault = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let store = SessionStore::new(vault.path().into());
    let meta=serde_json::from_value(json!({"id":"note","title":"Keep title","created_at":"2026-11-07T10:00:00Z","tags":["attached"],"tag_suggestions":{"items":["pending"],"dismissed":["ignored"]},"future":{"preserve":true}})).unwrap();
    store
        .create_note_session(&meta, "Personal notes stay unchanged.")
        .await
        .unwrap();
    store.ensure_summary("note").await.unwrap();
    store
        .update_generated_summary("note", BEFORE, Some(""))
        .await
        .unwrap();
    let original = store.session_tasks("note").await.unwrap().remove(0);
    store
        .replace_tasks(
            "session_summary",
            "note",
            vec![TaskInput {
                id: original.id,
                source_order: 0,
                status: "done".into(),
                text: "Follow up".into(),
                body: json!([]),
                due_at: "2026-12-01".into(),
            }],
        )
        .await
        .unwrap();
    store
        .replace_tasks(
            "session_raw_note",
            "note",
            vec![TaskInput {
                id: "personal-task".into(),
                source_order: 0,
                status: "todo".into(),
                text: "Personal action".into(),
                body: json!([]),
                due_at: String::new(),
            }],
        )
        .await
        .unwrap();
    let dir = vault.path().join("sessions/note");
    std::fs::write(dir.join("foreign.md"), "Attachment canary").unwrap();
    std::fs::write(dir.join(".hidden"), "Hidden canary").unwrap();
    std::fs::write(vault.path().join("tags.json"),json!({"tags":[{"id":"attached","name":"attached","future":{"keep":true}}],"future":{"registry":true}}).to_string()).unwrap();
    (vault, runtime, store)
}

fn scored(name: &str, confidence: f64) -> ScoredTagSuggestion {
    ScoredTagSuggestion {
        name: name.into(),
        confidence,
    }
}
fn suggestions() -> Option<Vec<ScoredTagSuggestion>> {
    Some(vec![
        scored("Project/New", 0.99),
        scored("pending", 0.85),
        scored("ignored", 1.0),
    ])
}
async fn revision(store: &SessionStore) -> String {
    store
        .read_document_snapshot("note", &DocumentTarget::Summary)
        .await
        .unwrap()
        .unwrap()
        .revision
}
fn intent(root: &Path) -> PathBuf {
    root.join(format!("{}.json", crate::sha256(b"note")))
}
async fn save(
    store: &SessionStore,
    root: &Path,
    suggestions: Option<Vec<ScoredTagSuggestion>>,
    auto: bool,
) -> DocumentSaveOutcome {
    store
        .save_generated_summary_with_suggestions_if_revision(
            root,
            "note",
            GENERATED,
            &revision(store).await,
            suggestions,
            auto,
        )
        .await
        .unwrap()
}
fn files(vault: &Path) -> Vec<Vec<u8>> {
    [
        "sessions/note/summary.md",
        "sessions/note/_meta.json",
        "sessions/note/tasks.json",
        "tags.json",
    ]
    .iter()
    .map(|path| std::fs::read(vault.join(path)).unwrap())
    .collect()
}
async fn failed_at(
    store: &SessionStore,
    root: &Path,
    stage: CommitStage,
) -> Result<DocumentSaveOutcome, StoreError> {
    store
        .save_generated_summary_with_checkpoint(
            root,
            "note",
            (GENERATED, &revision(store).await),
            suggestions(),
            true,
            move |current| async move {
                if current == stage {
                    Err(StoreError::Io(
                        "Injected failure after file publication".into(),
                    ))
                } else {
                    Ok(())
                }
            },
        )
        .await
}

#[tokio::test]
async fn checked_generation_applies_confident_tags_preserves_dismissals_and_reconciles_stable_tasks()
 {
    let (vault, runtime, store) = fixture().await;
    let before_tasks = store.session_tasks("note").await.unwrap();
    let original = before_tasks
        .iter()
        .find(|task| task.source_type == "session_summary")
        .unwrap();
    let mut changes = store.take_index_change_receiver().unwrap();
    while changes.try_recv().is_ok() {}
    let outcome = save(&store, runtime.path(), suggestions(), true).await;
    assert!(matches!(outcome, DocumentSaveOutcome::Saved(_)));
    let meta = store.read_meta("note").await.unwrap().unwrap();
    assert_eq!(meta.tags, vec!["attached", "project/new"]);
    assert_eq!(
        meta.tag_suggestions.as_ref().unwrap().items,
        vec!["pending"]
    );
    assert_eq!(
        meta.tag_suggestions.as_ref().unwrap().dismissed,
        vec!["ignored"]
    );
    assert_eq!(meta.title, "Keep title");
    assert_eq!(meta.extra["future"], json!({"preserve":true}));
    let tasks = store.session_tasks("note").await.unwrap();
    let stable = tasks.iter().find(|task| task.text == "Follow up").unwrap();
    assert_eq!(stable.id, original.id);
    assert_eq!(stable.status, "done");
    assert_eq!(stable.due_at, "2026-12-01");
    assert!(tasks.iter().any(|task| task.text == "Send agenda"));
    assert_eq!(
        tasks.iter().find(|task| task.id == "personal-task"),
        before_tasks.iter().find(|task| task.id == "personal-task")
    );
    let registry: Value =
        serde_json::from_slice(&std::fs::read(vault.path().join("tags.json")).unwrap()).unwrap();
    assert_eq!(registry["future"], json!({"registry":true}));
    assert_eq!(registry["tags"][0]["future"], json!({"keep":true}));
    assert_eq!(registry["tags"].as_array().unwrap().len(), 2);
    let mut entities = std::collections::HashSet::new();
    while let Ok(change) = changes.try_recv() {
        entities.insert(format!("{:?}", change.entity));
    }
    for entity in ["Sessions", "Docs", "Tasks", "Tags"] {
        assert!(entities.contains(entity), "{entities:?}");
    }
    assert!(!intent(runtime.path()).exists());
    assert_eq!(
        std::fs::read_to_string(vault.path().join("sessions/note/notes.md")).unwrap(),
        "Personal notes stay unchanged."
    );
    assert_eq!(
        std::fs::read_to_string(vault.path().join("sessions/note/foreign.md")).unwrap(),
        "Attachment canary"
    );
}

#[tokio::test]
async fn thresholds_opt_out_attached_dismissed_and_empty_suggestions_keep_existing_policy() {
    for (input, auto, attached, items) in [
        (
            Some(vec![
                scored("at-threshold", 0.85),
                scored("below", 0.2),
                scored("above", 0.85001),
            ]),
            true,
            vec!["attached", "above"],
            vec!["at-threshold", "below"],
        ),
        (
            Some(vec![scored("new", 0.99)]),
            false,
            vec!["attached"],
            vec!["new"],
        ),
        (
            Some(vec![
                scored("ATTACHED", 1.0),
                scored("IGNORED", 1.0),
                scored("new", 0.4),
            ]),
            true,
            vec!["attached"],
            vec!["new"],
        ),
        (None, true, vec!["attached"], vec!["pending"]),
        (Some(vec![]), true, vec!["attached"], vec![]),
    ] {
        let (_vault, runtime, store) = fixture().await;
        save(&store, runtime.path(), input, auto).await;
        let meta = store.read_meta("note").await.unwrap().unwrap();
        let mut expected: Vec<String> = attached.into_iter().map(str::to_owned).collect();
        if auto && expected.len() > 1 {
            expected.sort();
        }
        assert_eq!(meta.tags, expected);
        let state = meta.tag_suggestions.unwrap();
        assert_eq!(state.items, items);
        assert_eq!(state.dismissed, vec!["ignored"]);
    }
}

#[tokio::test]
async fn a_first_summary_can_be_created_against_its_absent_revision() {
    let (vault, runtime, store) = fixture().await;
    std::fs::remove_file(vault.path().join("sessions/note/summary.md")).unwrap();
    let absent = store
        .read_document_snapshot("note", &DocumentTarget::Summary)
        .await
        .unwrap()
        .unwrap();
    assert!(absent.document.is_none());
    let outcome = store
        .save_generated_summary_with_suggestions_if_revision(
            runtime.path(),
            "note",
            GENERATED,
            &absent.revision,
            suggestions(),
            true,
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome,DocumentSaveOutcome::Saved(snapshot) if snapshot.document.as_ref().unwrap().markdown==PUBLISHED)
    );
    assert!(
        store
            .read_meta("note")
            .await
            .unwrap()
            .unwrap()
            .tags
            .contains(&"project/new".into())
    );
    assert!(!intent(runtime.path()).exists());
}

#[tokio::test]
async fn stale_summary_is_rejected_without_applying_tags_or_replacing_newer_tasks() {
    let (vault, runtime, store) = fixture().await;
    let stale = revision(&store).await;
    store
        .update_generated_summary("note", "# User edit\n- [ ] Manual task", Some(BEFORE))
        .await
        .unwrap();
    let before = files(vault.path());
    let outcome = store
        .save_generated_summary_with_suggestions_if_revision(
            runtime.path(),
            "note",
            GENERATED,
            &stale,
            suggestions(),
            true,
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome,DocumentSaveOutcome::Conflict(snapshot) if snapshot.document.as_ref().unwrap().markdown.contains("User edit"))
    );
    assert_eq!(files(vault.path()), before);
    assert!(!intent(runtime.path()).exists());
}

#[tokio::test]
async fn uncertain_body_publication_updates_the_index_and_invalidates_content_for_recovery() {
    let (_vault, runtime, store) = fixture().await;
    let mut changes = store.take_index_change_receiver().unwrap();
    while changes.try_recv().is_ok() {}
    assert!(
        failed_at(&store, runtime.path(), CommitStage::Body)
            .await
            .is_err()
    );
    assert_eq!(store.summary_get("note").as_deref(), Some(PUBLISHED));
    let mut entities = std::collections::HashSet::new();
    while let Ok(change) = changes.try_recv() {
        entities.insert(format!("{:?}", change.entity));
    }
    for entity in ["Sessions", "Docs", "Tasks", "Tags"] {
        assert!(entities.contains(entity), "{entities:?}");
    }
    store.recover_document_writes(runtime.path()).await.unwrap();
    assert!(!intent(runtime.path()).exists());
}

#[tokio::test]
async fn fault_after_each_file_boundary_recovers_once_without_a_second_generation() {
    for stage in [
        CommitStage::Intent,
        CommitStage::Body,
        CommitStage::Tasks,
        CommitStage::Metadata,
        CommitStage::Registry,
    ] {
        let (vault, runtime, store) = fixture().await;
        let before = files(vault.path());
        assert!(failed_at(&store, runtime.path(), stage).await.is_err());
        let record: Value =
            serde_json::from_slice(&std::fs::read(intent(runtime.path())).unwrap()).unwrap();
        assert_eq!(record["schema_version"], 3);
        assert_eq!(record["target"]["kind"], "generated_summary");
        let published = files(vault.path());
        let paths = [
            "sessions/note/summary.md",
            "sessions/note/_meta.json",
            "sessions/note/tasks.json",
            "tags.json",
        ];
        let modified: Vec<_> = paths
            .iter()
            .map(|path| {
                std::fs::metadata(vault.path().join(path))
                    .unwrap()
                    .modified()
                    .unwrap()
            })
            .collect();
        drop(store);
        let store = SessionStore::new(vault.path().into());
        let actual = store
            .read_recovered_document_snapshot(
                Some(runtime.path()),
                "note",
                &DocumentTarget::Summary,
            )
            .await
            .unwrap()
            .unwrap();
        if stage == CommitStage::Intent {
            assert_eq!(actual.document.unwrap().markdown, BEFORE);
            assert_eq!(files(vault.path()), before);
        } else {
            assert_eq!(actual.document.unwrap().markdown, PUBLISHED);
            let meta = store.read_meta("note").await.unwrap().unwrap();
            assert!(meta.tags.contains(&"project/new".into()));
            assert_eq!(
                store
                    .session_tasks("note")
                    .await
                    .unwrap()
                    .iter()
                    .find(|task| task.text == "Follow up")
                    .unwrap()
                    .status,
                "done"
            );
            let registry = store.list_tags().await.unwrap();
            assert_eq!(
                registry
                    .iter()
                    .filter(|tag| tag.id == "project/new")
                    .count(),
                1
            );
            let mut planned: Vec<crate::TaskItem> =
                serde_json::from_value(record["tasks"].clone()).unwrap();
            crate::tasks::sort_tasks(&mut planned);
            let mut actual = store.session_tasks("note").await.unwrap();
            crate::tasks::sort_tasks(&mut actual);
            assert_eq!(actual, planned);
        }
        if stage == CommitStage::Registry {
            assert_eq!(files(vault.path()), published);
            for (path, modified) in paths.iter().zip(modified) {
                assert_eq!(
                    std::fs::metadata(vault.path().join(path))
                        .unwrap()
                        .modified()
                        .unwrap(),
                    modified
                );
            }
        }
        assert!(!intent(runtime.path()).exists());
        let complete = files(vault.path());
        store.recover_document_writes(runtime.path()).await.unwrap();
        assert_eq!(files(vault.path()), complete);
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/note/.hidden")).unwrap(),
            "Hidden canary"
        );
    }
}

#[tokio::test]
async fn recovery_merges_newer_unrelated_metadata_and_registry_fields() {
    let (vault, runtime, store) = fixture().await;
    assert!(
        failed_at(&store, runtime.path(), CommitStage::Body)
            .await
            .is_err()
    );
    let meta_path = vault.path().join("sessions/note/_meta.json");
    let mut meta: Value = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    meta["title"] = json!("Title changed after interruption");
    meta["future_after"] = json!({"preserve":"latest"});
    std::fs::write(&meta_path, meta.to_string()).unwrap();
    let tags_path = vault.path().join("tags.json");
    let mut tags: Value = serde_json::from_slice(&std::fs::read(&tags_path).unwrap()).unwrap();
    tags["tags"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"other-session","name":"Other session","future":42}));
    tags["future_after"] = json!(true);
    std::fs::write(tags_path, tags.to_string()).unwrap();
    drop(store);
    let store = SessionStore::new(vault.path().into());
    let meta = store
        .read_recovered_metadata_snapshot(Some(runtime.path()), "note")
        .await
        .unwrap()
        .unwrap()
        .meta;
    assert_eq!(meta.title, "Title changed after interruption");
    assert_eq!(meta.extra["future_after"], json!({"preserve":"latest"}));
    assert!(meta.tags.contains(&"project/new".into()));
    let tags: Value =
        serde_json::from_slice(&std::fs::read(vault.path().join("tags.json")).unwrap()).unwrap();
    assert_eq!(tags["tags"][1]["future"], 42);
    assert_eq!(tags["future_after"], true);
    assert_eq!(tags["tags"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn newer_summary_tasks_or_tag_decisions_block_recovery_before_other_files_change() {
    for changed in ["summary", "tasks", "tags", "registry"] {
        let (vault, runtime, store) = fixture().await;
        assert!(
            failed_at(&store, runtime.path(), CommitStage::Body)
                .await
                .is_err()
        );
        let directory = vault.path().join("sessions/note");
        match changed {
            "summary" => {
                std::fs::write(directory.join("summary.md"), "# Newer manual summary").unwrap()
            }
            "tasks" => {
                let path = directory.join("tasks.json");
                let mut value: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                value["tasks"][0]["due_at"] = json!("2027-01-01");
                std::fs::write(path, value.to_string()).unwrap();
            }
            "tags" => {
                let path = directory.join("_meta.json");
                let mut value: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                value["tag_suggestions"]["dismissed"] = json!(["ignored", "project/new"]);
                std::fs::write(path, value.to_string()).unwrap();
            }
            "registry" => {
                std::fs::write(vault.path().join("tags.json"), "{damaged registry").unwrap()
            }
            _ => unreachable!(),
        }
        let before = files(vault.path());
        drop(store);
        let store = SessionStore::new(vault.path().into());
        assert!(
            store
                .read_recovered_document_snapshot(
                    Some(runtime.path()),
                    "note",
                    &DocumentTarget::Summary
                )
                .await
                .is_err(),
            "{changed}"
        );
        assert_eq!(files(vault.path()), before, "{changed}");
        assert!(intent(runtime.path()).exists());
    }
}

#[tokio::test]
async fn corrupt_or_symlinked_registry_never_becomes_an_empty_replacement() {
    let (vault, runtime, store) = fixture().await;
    let path = vault.path().join("tags.json");
    for bytes in ["{broken", "null", r#"{"tags":[{"broken":true}]}"#] {
        std::fs::write(&path, bytes).unwrap();
        let before = files(vault.path());
        assert!(
            store
                .save_generated_summary_with_suggestions_if_revision(
                    runtime.path(),
                    "note",
                    GENERATED,
                    &revision(&store).await,
                    suggestions(),
                    true
                )
                .await
                .is_err()
        );
        assert_eq!(files(vault.path()), before);
        assert!(!intent(runtime.path()).exists());
    }
    #[cfg(unix)]
    {
        let other = tempfile::tempdir().unwrap();
        let target = other.path().join("private.json");
        std::fs::write(&target, r#"{"tags":[]}"#).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(
            store
                .save_generated_summary_with_suggestions_if_revision(
                    runtime.path(),
                    "note",
                    GENERATED,
                    &revision(&store).await,
                    suggestions(),
                    true
                )
                .await
                .is_err()
        );
        assert!(path.is_symlink());
        assert_eq!(std::fs::read_to_string(target).unwrap(), r#"{"tags":[]}"#);
    }
}

#[tokio::test]
async fn unchanged_body_still_recovers_the_admitted_tag_and_task_updates() {
    let (vault, runtime, store) = fixture().await;
    let unchanged = "# Keep title\n\n# Before\n- [ ] Follow up";
    store
        .update_generated_summary("note", unchanged, Some(BEFORE))
        .await
        .unwrap();
    let result = store
        .save_generated_summary_with_checkpoint(
            runtime.path(),
            "note",
            (unchanged, &revision(&store).await),
            suggestions(),
            true,
            |stage| async move {
                if stage == CommitStage::Intent {
                    Err(StoreError::Io("stop".into()))
                } else {
                    Ok(())
                }
            },
        )
        .await;
    assert!(result.is_err());
    drop(store);
    let store = SessionStore::new(vault.path().into());
    store.recover_document_writes(runtime.path()).await.unwrap();
    assert!(
        store
            .read_meta("note")
            .await
            .unwrap()
            .unwrap()
            .tags
            .contains(&"project/new".into())
    );
    assert!(!intent(runtime.path()).exists());
}

#[tokio::test]
async fn recording_refuses_application_and_keeps_uncertain_recovery_until_recording_ends() {
    let (vault, runtime, store) = fixture().await;
    let before = files(vault.path());
    store.note_recording_active("note");
    assert!(
        store
            .save_generated_summary_with_suggestions_if_revision(
                runtime.path(),
                "note",
                GENERATED,
                &revision(&store).await,
                suggestions(),
                true
            )
            .await
            .is_err()
    );
    assert_eq!(files(vault.path()), before);
    store.release_recording_prepare("note").await.unwrap();
    assert!(
        failed_at(&store, runtime.path(), CommitStage::Body)
            .await
            .is_err()
    );
    store.note_recording_active("note");
    let pending = files(vault.path());
    assert!(store.recover_document_writes(runtime.path()).await.is_err());
    assert_eq!(files(vault.path()), pending);
    assert!(intent(runtime.path()).exists());
    store.release_recording_prepare("note").await.unwrap();
    store.recover_document_writes(runtime.path()).await.unwrap();
    assert!(!intent(runtime.path()).exists());
}

#[tokio::test]
async fn cancellation_keeps_the_continuous_write_lock_until_all_admitted_files_finish() {
    let (_vault, runtime, store) = fixture().await;
    let root = runtime.path().to_path_buf();
    let expected = revision(&store).await;
    let pause = Arc::new(tokio::sync::Semaphore::new(0));
    let gate = pause.clone();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let mut entered = Some(entered);
    let writer = store.clone();
    let task = tokio::spawn(async move {
        writer
            .save_generated_summary_with_checkpoint(
                &root,
                "note",
                (GENERATED, &expected),
                suggestions(),
                true,
                move |stage| {
                    let gate = gate.clone();
                    let entered = if stage == CommitStage::Body {
                        entered.take()
                    } else {
                        None
                    };
                    async move {
                        if let Some(entered) = entered {
                            entered.send(()).unwrap();
                            gate.acquire().await.unwrap().forget();
                        }
                        Ok(())
                    }
                },
            )
            .await
    });
    waiting.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(store.write_lock.try_lock().is_err());
    pause.add_permits(1);
    let snapshot = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        store.read_recovered_metadata_snapshot(Some(runtime.path()), "note"),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert!(snapshot.meta.tags.contains(&"project/new".into()));
    assert!(!intent(runtime.path()).exists());
}

#[tokio::test]
async fn legacy_summary_migration_and_filename_collision_preserve_task_identity_and_foreign_bytes()
{
    for collision in [false, true] {
        let (vault, runtime, store) = fixture().await;
        std::fs::remove_file(vault.path().join("sessions/note/summary.md")).unwrap();
        store
            .write_enhanced_doc(&crate::EnhancedDoc {
                id: "old-summary".into(),
                session_id: "note".into(),
                kind: "summary".into(),
                title: "Legacy title".into(),
                template_id: String::new(),
                sort_order: 0,
                markdown: BEFORE.into(),
            })
            .await
            .unwrap();
        if collision {
            std::fs::write(
                vault.path().join("sessions/note/summary.md"),
                "Do not replace this conflicting file",
            )
            .unwrap();
        }
        let old_tasks = store.session_tasks("note").await.unwrap();
        let old = old_tasks
            .iter()
            .find(|task| task.text == "Follow up")
            .unwrap();
        let mut followup = old.clone();
        followup.source_type = "enhanced_note".into();
        followup.source_id = "old-summary".into();
        let tasks_path = vault.path().join("sessions/note/tasks.json");
        let mut tasks: Value =
            serde_json::from_slice(&std::fs::read(&tasks_path).unwrap()).unwrap();
        let target = tasks["tasks"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|task| task["id"] == followup.id)
            .unwrap();
        *target = serde_json::to_value(&followup).unwrap();
        std::fs::write(tasks_path, tasks.to_string()).unwrap();
        save(&store, runtime.path(), suggestions(), true).await;
        assert_eq!(
            store.read_summary("note").await.unwrap().as_deref(),
            Some(PUBLISHED)
        );
        let tasks = store.session_tasks("note").await.unwrap();
        let current = tasks.iter().find(|task| task.text == "Follow up").unwrap();
        assert_eq!(current.id, old.id);
        assert_eq!(current.status, "done");
        assert_eq!(current.due_at, old.due_at);
        if collision {
            assert_eq!(
                std::fs::read_to_string(vault.path().join("sessions/note/summary.md")).unwrap(),
                "Do not replace this conflicting file"
            );
        } else {
            assert_eq!(
                std::fs::read_to_string(vault.path().join("sessions/note/summary.md")).unwrap(),
                PUBLISHED
            );
            assert!(
                !vault
                    .path()
                    .join("sessions/note/enhanced/old-summary.md")
                    .exists()
            );
        }
    }
}

#[tokio::test]
async fn interrupted_legacy_collision_recovers_each_boundary_without_replacing_foreign_bytes() {
    for stage in [
        CommitStage::Intent,
        CommitStage::Body,
        CommitStage::Tasks,
        CommitStage::Metadata,
        CommitStage::Registry,
    ] {
        let (vault, runtime, store) = fixture().await;
        let canonical = vault.path().join("sessions/note/summary.md");
        std::fs::remove_file(&canonical).unwrap();
        store
            .write_enhanced_doc(&crate::EnhancedDoc {
                id: "legacy-summary".into(),
                session_id: "note".into(),
                kind: "summary".into(),
                title: "Legacy title".into(),
                template_id: String::new(),
                sort_order: 0,
                markdown: BEFORE.into(),
            })
            .await
            .unwrap();
        std::fs::write(&canonical, "Conflicting user file").unwrap();
        assert!(failed_at(&store, runtime.path(), stage).await.is_err());
        drop(store);
        let store = SessionStore::new(vault.path().into());
        store.recover_document_writes(runtime.path()).await.unwrap();
        assert_eq!(
            store.read_summary("note").await.unwrap().as_deref(),
            Some(if stage == CommitStage::Intent {
                BEFORE
            } else {
                PUBLISHED
            })
        );
        assert_eq!(
            std::fs::read_to_string(&canonical).unwrap(),
            "Conflicting user file"
        );
        assert!(!intent(runtime.path()).exists());
    }
}

#[tokio::test]
async fn future_intents_are_retained_and_deleted_sessions_are_never_recreated() {
    let (vault, runtime, store) = fixture().await;
    assert!(
        failed_at(&store, runtime.path(), CommitStage::Body)
            .await
            .is_err()
    );
    let record_path = intent(runtime.path());
    let mut value: Value = serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    value["schema_version"] = json!(4);
    std::fs::write(&record_path, value.to_string()).unwrap();
    let before = files(vault.path());
    assert!(store.recover_document_writes(runtime.path()).await.is_err());
    assert_eq!(files(vault.path()), before);
    assert!(record_path.exists());
    value["schema_version"] = json!(3);
    std::fs::write(&record_path, value.to_string()).unwrap();
    std::fs::remove_dir_all(vault.path().join("sessions/note")).unwrap();
    store.recover_document_writes(runtime.path()).await.unwrap();
    assert!(!vault.path().join("sessions/note").exists());
    assert!(!record_path.exists());
    assert!(
        !store
            .list_tags()
            .await
            .unwrap()
            .iter()
            .any(|tag| tag.id == "project/new")
    );
}

#[tokio::test]
async fn killed_processes_leave_recoverable_intents_at_each_file_boundary() {
    for stage in [
        CommitStage::Intent,
        CommitStage::Body,
        CommitStage::Tasks,
        CommitStage::Metadata,
        CommitStage::Registry,
    ] {
        let (vault, runtime, store) = fixture().await;
        drop(store);
        let executable = std::env::current_exe().unwrap();
        let vault_path = vault.path().to_path_buf();
        let runtime_path = runtime.path().to_path_buf();
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new(executable)
                .args([
                    "--ignored",
                    "--exact",
                    "summary::generated::tests::generated_summary_child",
                    "--nocapture",
                ])
                .env("LOOFAH_GENERATED_SUMMARY_CHILD_VAULT", vault_path)
                .env("LOOFAH_GENERATED_SUMMARY_CHILD_RUNTIME", runtime_path)
                .env("LOOFAH_GENERATED_SUMMARY_CHILD_STAGE", format!("{stage:?}"))
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(
            output.status.code(),
            Some(77),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(intent(runtime.path()).exists());
        let store = SessionStore::new(vault.path().into());
        store.recover_document_writes(runtime.path()).await.unwrap();
        assert_eq!(
            store.read_summary("note").await.unwrap().as_deref(),
            Some(if stage == CommitStage::Intent {
                BEFORE
            } else {
                PUBLISHED
            })
        );
        assert_eq!(
            store
                .read_meta("note")
                .await
                .unwrap()
                .unwrap()
                .tags
                .contains(&"project/new".into()),
            stage != CommitStage::Intent
        );
        assert!(!intent(runtime.path()).exists());
    }
}

#[tokio::test]
#[ignore = "subprocess fixture"]
async fn generated_summary_child() {
    let Some(vault) = std::env::var_os("LOOFAH_GENERATED_SUMMARY_CHILD_VAULT") else {
        return;
    };
    let runtime =
        PathBuf::from(std::env::var_os("LOOFAH_GENERATED_SUMMARY_CHILD_RUNTIME").unwrap());
    let stage = std::env::var("LOOFAH_GENERATED_SUMMARY_CHILD_STAGE").unwrap();
    let store = SessionStore::new(vault.into());
    store
        .save_generated_summary_with_checkpoint(
            &runtime,
            "note",
            (GENERATED, &revision(&store).await),
            suggestions(),
            true,
            move |current| {
                let expected = stage.clone();
                async move {
                    if format!("{current:?}") == expected {
                        std::process::exit(77);
                    }
                    Ok(())
                }
            },
        )
        .await
        .unwrap();
    panic!("subprocess did not exit at its boundary");
}

#[tokio::test]
async fn generated_summary_stamps_existing_title_with_legacy_markdown_semantics() {
    for (title, markdown, expected) in [
        ("Keep title", GENERATED, PUBLISHED),
        (
            " Keep title ",
            "  # Keep title\n\nAlready headed",
            "  # Keep title\n\nAlready headed",
        ),
        ("", GENERATED, GENERATED),
        ("  ", GENERATED, GENERATED),
        (
            "**Title**\n- [ ] Heading action",
            "# Provider heading\nBody",
            "# **Title**\n- [ ] Heading action\n\n# Provider heading\nBody",
        ),
    ] {
        let (vault, root, store) = fixture().await;
        store
            .update_meta(
                "note",
                crate::SessionMetaPatch {
                    title: Some(title.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let outcome = store
            .save_generated_summary_with_suggestions_if_revision(
                root.path(),
                "note",
                markdown,
                &revision(&store).await,
                None,
                false,
            )
            .await
            .unwrap();
        let DocumentSaveOutcome::Saved(snapshot) = outcome else {
            panic!("expected saved generated document")
        };
        assert_eq!(snapshot.document.unwrap().markdown, expected);
        assert_eq!(store.read_meta("note").await.unwrap().unwrap().title, title);
        if title.contains("Heading action") {
            assert!(
                store
                    .session_tasks("note")
                    .await
                    .unwrap()
                    .iter()
                    .any(|task| task.text == "Heading action")
            );
        }
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/note/summary.md")).unwrap(),
            expected
        );
        assert!(!intent(root.path()).exists());
    }
}

#[tokio::test]
async fn generated_summary_checks_prepared_title_before_any_publication() {
    for (before_title, after_title) in [
        ("Keep title", "Renamed"),
        ("", "New title"),
        ("Keep title", ""),
    ] {
        let (vault, root, store) = fixture().await;
        store
            .update_meta(
                "note",
                crate::SessionMetaPatch {
                    title: Some(before_title.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let expected_revision = revision(&store).await;
        let before = files(vault.path());
        let updater = store.clone();
        let outcome = store
            .save_generated_summary_with_checkpoint(
                root.path(),
                "note",
                (GENERATED, &expected_revision),
                None,
                false,
                move |stage| {
                    let updater = updater.clone();
                    async move {
                        if stage == CommitStage::Prepared {
                            updater
                                .update_meta(
                                    "note",
                                    crate::SessionMetaPatch {
                                        title: Some(after_title.into()),
                                        ..Default::default()
                                    },
                                )
                                .await?;
                        }
                        Ok(())
                    }
                },
            )
            .await
            .unwrap();
        assert!(
            matches!(outcome, DocumentSaveOutcome::Conflict(snapshot) if snapshot.revision == expected_revision)
        );
        let after = files(vault.path());
        for index in [0, 2, 3] {
            assert_eq!(before[index], after[index]);
        }
        assert_eq!(
            store.read_meta("note").await.unwrap().unwrap().title,
            after_title
        );
        assert!(!intent(root.path()).exists());
    }
}

#[tokio::test]
async fn generated_summary_accepts_unrelated_metadata_edits_after_preparation() {
    let (_vault, root, store) = fixture().await;
    let expected_revision = revision(&store).await;
    let updater = store.clone();
    let outcome = store
        .save_generated_summary_with_checkpoint(
            root.path(),
            "note",
            (GENERATED, &expected_revision),
            None,
            false,
            move |stage| {
                let updater = updater.clone();
                async move {
                    if stage == CommitStage::Prepared {
                        updater
                            .update_meta(
                                "note",
                                crate::SessionMetaPatch {
                                    tags: Some(vec!["manual".into()]),
                                    ..Default::default()
                                },
                            )
                            .await?;
                    }
                    Ok(())
                }
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome, DocumentSaveOutcome::Saved(snapshot) if snapshot.document.as_ref().unwrap().markdown == PUBLISHED)
    );
    assert_eq!(
        store.read_meta("note").await.unwrap().unwrap().tags,
        ["manual"]
    );
}

#[tokio::test]
async fn generated_summary_quota_includes_stamped_title_before_intent() {
    let (vault, root, mut store) = fixture().await;
    store.content_limits.note_write_bytes = GENERATED.len();
    let before = files(vault.path());
    let markdown = GENERATED;
    let outcome = store
        .save_generated_summary_with_suggestions_if_revision(
            root.path(),
            "note",
            markdown,
            &revision(&store).await,
            None,
            false,
        )
        .await;
    assert!(matches!(outcome, Err(StoreError::ResourceLimit(_))));
    assert_eq!(files(vault.path()), before);
    assert!(!intent(root.path()).exists());
    store.content_limits.note_write_bytes = PUBLISHED.len();
    assert!(
        matches!(store.save_generated_summary_with_suggestions_if_revision(root.path(), "note", GENERATED, &revision(&store).await, None, false).await.unwrap(), DocumentSaveOutcome::Saved(snapshot) if snapshot.document.as_ref().unwrap().markdown == PUBLISHED)
    );
}

#[tokio::test]
async fn generated_summary_rename_conflict_does_not_migrate_legacy_body() {
    let (vault, root, store) = fixture().await;
    let canonical = vault.path().join("sessions/note/summary.md");
    std::fs::remove_file(&canonical).unwrap();
    store
        .write_enhanced_doc(&crate::EnhancedDoc {
            id: "old-summary".into(),
            session_id: "note".into(),
            kind: "summary".into(),
            title: "Legacy title".into(),
            template_id: String::new(),
            sort_order: 0,
            markdown: BEFORE.into(),
        })
        .await
        .unwrap();
    let legacy = vault.path().join("sessions/note/enhanced/old-summary.md");
    let before = std::fs::read(&legacy).unwrap();
    let tasks = std::fs::read(vault.path().join("sessions/note/tasks.json")).unwrap();
    let updater = store.clone();
    let outcome = store
        .save_generated_summary_with_checkpoint(
            root.path(),
            "note",
            (GENERATED, &revision(&store).await),
            suggestions(),
            true,
            move |stage| {
                let updater = updater.clone();
                async move {
                    if stage == CommitStage::Prepared {
                        updater
                            .update_meta(
                                "note",
                                crate::SessionMetaPatch {
                                    title: Some("Renamed".into()),
                                    ..Default::default()
                                },
                            )
                            .await?;
                    }
                    Ok(())
                }
            },
        )
        .await
        .unwrap();
    assert!(matches!(outcome, DocumentSaveOutcome::Conflict(_)));
    assert!(!canonical.exists());
    assert_eq!(std::fs::read(legacy).unwrap(), before);
    assert_eq!(
        std::fs::read(vault.path().join("sessions/note/tasks.json")).unwrap(),
        tasks
    );
    assert!(!intent(root.path()).exists());
}

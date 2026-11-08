use super::*;

async fn fixture() -> (tempfile::TempDir, JobJournal, String) {
    setup(true).await
}

async fn setup(confirm: bool) -> (tempfile::TempDir, JobJournal, String) {
    let root = tempfile::tempdir().unwrap();
    let journal = JobJournal::open(root.path().join("jobs"), EventHub::new("vault".into(), 64))
        .await
        .unwrap();
    let job = journal
        .submit(
            "member".into(),
            uuid::Uuid::new_v4().to_string(),
            SummaryJobRequest {
                session_id: "note".into(),
                target: DocumentTarget::Summary,
                expected_revision: "original".into(),
                title_policy: TitlePolicy::GenerateIfEmpty,
            },
        )
        .await
        .unwrap();
    journal.claim(job.id.clone()).await.unwrap();
    journal
        .retain_result(
            job.id.clone(),
            GeneratedDocument {
                markdown: "# Saved body".into(),
                suggested_tags: None,
            },
        )
        .await
        .unwrap();
    journal.begin_apply(job.id.clone()).await.unwrap();
    if confirm {
        journal
            .mark_body_applied(job.id.clone(), "saved".into())
            .await
            .unwrap();
    }
    (root, journal, job.id)
}

#[tokio::test]
async fn an_uncertain_body_confirmation_write_is_reconciled_from_durable_bytes_without_title_replay()
 {
    let (root, journal, id) = setup(false).await;
    journal
        .run(|state, _| {
            state.fail_after_persist = true;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        journal
            .mark_body_applied(id.clone(), "saved".into())
            .await
            .is_err()
    );
    assert!(
        journal
            .advance_title(id.clone(), TitleStage::Generating {})
            .await
            .is_err()
    );
    drop(journal);
    let journal = JobJournal::open(root.path().join("jobs"), EventHub::new("vault".into(), 64))
        .await
        .unwrap();
    assert_eq!(
        journal.get(id.clone()).await.unwrap().unwrap().state,
        ProcessingJobState::Succeeded
    );
    assert_eq!(
        public(&journal, &id).await.status,
        ProcessingTitleStatus::Interrupted
    );
    assert!(journal.queued().await.unwrap().is_empty());
}

fn title_path(root: &Path, id: &str) -> PathBuf {
    root.join("jobs/titles").join(format!("{id}.json"))
}

async fn public(journal: &JobJournal, id: &str) -> ProcessingTitleResult {
    journal
        .owned_result("member".into(), id.into())
        .await
        .unwrap()
        .unwrap()
        .title
        .unwrap()
}

#[tokio::test]
async fn confirmed_body_is_historical_success_and_its_generated_result_remains_immutable() {
    let (root, journal, id) = fixture().await;
    let body = std::fs::read(root.path().join("jobs/results").join(format!("{id}.json"))).unwrap();
    journal
        .advance_title(id.clone(), TitleStage::Generating {})
        .await
        .unwrap();
    journal
        .advance_title(
            id.clone(),
            TitleStage::Candidate {
                candidate: "Candidate".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        journal
            .progress(id.clone(), ProcessingPhase::Generating)
            .await
            .is_err()
    );
    journal.cancel(id.clone()).await.unwrap();
    assert_eq!(
        journal
            .advance_title(
                id.clone(),
                TitleStage::Applying {
                    candidate: "Candidate".into()
                }
            )
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Cancelled
    );
    journal
        .complete(id.clone(), JobCompletion::Cancelled)
        .await
        .unwrap();
    let job = journal.get(id.clone()).await.unwrap().unwrap();
    assert_eq!(job.state, ProcessingJobState::Succeeded);
    assert_eq!(job.failure, None);
    assert_eq!(
        public(&journal, &id).await.status,
        ProcessingTitleStatus::Interrupted
    );
    assert_eq!(
        public(&journal, &id).await.candidate.as_deref(),
        Some("Candidate")
    );
    assert_eq!(
        std::fs::read(root.path().join("jobs/results").join(format!("{id}.json"))).unwrap(),
        body
    );
    assert!(
        journal
            .retain_result(
                id,
                GeneratedDocument {
                    markdown: "# Replacement".into(),
                    suggested_tags: None
                }
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn restart_completes_confirmed_bodies_without_replaying_any_incomplete_title_stage() {
    for state in 0..5 {
        let (root, journal, id) = fixture().await;
        if state > 0 {
            journal
                .advance_title(id.clone(), TitleStage::Generating {})
                .await
                .unwrap();
        }
        if state > 1 {
            journal
                .advance_title(
                    id.clone(),
                    TitleStage::Candidate {
                        candidate: "Candidate".into(),
                    },
                )
                .await
                .unwrap();
        }
        if state > 2 {
            journal
                .advance_title(
                    id.clone(),
                    TitleStage::Applying {
                        candidate: "Candidate".into(),
                    },
                )
                .await
                .unwrap();
        }
        if state > 3 {
            journal
                .advance_title(
                    id.clone(),
                    TitleStage::finished(ProcessingTitleResult {
                        status: ProcessingTitleStatus::Applied,
                        candidate: Some("Candidate".into()),
                        failure: None,
                    }),
                )
                .await
                .unwrap();
        }
        drop(journal);
        let journal = JobJournal::open(root.path().join("jobs"), EventHub::new("vault".into(), 64))
            .await
            .unwrap();
        assert_eq!(
            journal.get(id.clone()).await.unwrap().unwrap().state,
            ProcessingJobState::Succeeded
        );
        let result = public(&journal, &id).await;
        assert_eq!(
            result.status,
            if state == 4 {
                ProcessingTitleStatus::Applied
            } else if state == 3 {
                ProcessingTitleStatus::Unconfirmed
            } else {
                ProcessingTitleStatus::Interrupted
            }
        );
        assert_eq!(result.candidate.is_some(), state >= 2);
        assert!(journal.queued().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn title_stage_transitions_are_one_use_and_never_discard_a_retained_candidate() {
    let (_, journal, id) = fixture().await;
    assert!(
        journal
            .advance_title(
                id.clone(),
                TitleStage::Applying {
                    candidate: "Candidate".into()
                }
            )
            .await
            .is_err()
    );
    assert!(
        journal
            .advance_title(
                id.clone(),
                TitleStage::finished(ProcessingTitleResult {
                    status: ProcessingTitleStatus::Applied,
                    candidate: Some("Candidate".into()),
                    failure: None
                })
            )
            .await
            .is_err()
    );
    journal
        .advance_title(id.clone(), TitleStage::Generating {})
        .await
        .unwrap();
    journal
        .advance_title(
            id.clone(),
            TitleStage::Candidate {
                candidate: "Candidate".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        journal
            .advance_title(
                id.clone(),
                TitleStage::Applying {
                    candidate: "Different".into()
                }
            )
            .await
            .is_err()
    );
    assert!(
        journal
            .advance_title(
                id.clone(),
                TitleStage::finished(ProcessingTitleResult {
                    status: ProcessingTitleStatus::Cancelled,
                    candidate: None,
                    failure: None
                })
            )
            .await
            .is_err()
    );
    let finished = TitleStage::finished(ProcessingTitleResult {
        status: ProcessingTitleStatus::Conflict,
        candidate: Some("Candidate".into()),
        failure: None,
    });
    journal
        .advance_title(id.clone(), finished.clone())
        .await
        .unwrap();
    journal.advance_title(id.clone(), finished).await.unwrap();
    assert!(
        journal
            .advance_title(id, TitleStage::Generating {})
            .await
            .is_err()
    );
}

#[tokio::test]
async fn strict_future_foreign_duplicate_malformed_and_oversized_title_records_are_preserved() {
    let (root, journal, id) = fixture().await;
    journal
        .advance_title(id.clone(), TitleStage::Generating {})
        .await
        .unwrap();
    let path = title_path(root.path(), &id);
    let original = std::fs::read(&path).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    let mut cases = vec![];
    for (field, changed) in [
        ("schema_version", serde_json::json!(2)),
        ("vault_id", serde_json::json!("foreign")),
        (
            "job_id",
            serde_json::json!(uuid::Uuid::new_v4().to_string()),
        ),
        ("body_revision", serde_json::json!("different")),
        ("future", serde_json::json!(true)),
    ] {
        let mut value = value.clone();
        value[field] = changed;
        cases.push(serde_json::to_vec(&value).unwrap());
    }
    let mut changed = value.clone();
    changed["stage"]["extra"] = serde_json::json!(true);
    cases.push(serde_json::to_vec(&changed).unwrap());
    cases.push(
        String::from_utf8(original.clone())
            .unwrap()
            .replacen('{', "{\"schema_version\":1,", 1)
            .into_bytes(),
    );
    cases.push(vec![b'x'; MAX_TITLE_RECORD_BYTES as usize + 1]);
    for bytes in cases {
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(
            public(&journal, &id).await.status,
            ProcessingTitleStatus::Unconfirmed
        );
        assert!(
            journal
                .advance_title(
                    id.clone(),
                    TitleStage::Candidate {
                        candidate: "Candidate".into()
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    journal
        .complete(id.clone(), JobCompletion::Succeeded)
        .await
        .unwrap();
    assert_eq!(
        journal.get(id).await.unwrap().unwrap().state,
        ProcessingJobState::Succeeded
    );
}

#[tokio::test]
async fn candidate_and_actual_serialized_budgets_are_recoverable_and_do_not_poison_body_success() {
    let (root, journal, id) = fixture().await;
    journal
        .advance_title(id.clone(), TitleStage::Generating {})
        .await
        .unwrap();
    for candidate in [
        "x".repeat(4097),
        "Bad\nTitle".into(),
        "<EMPTY>".into(),
        "   ".into(),
    ] {
        assert!(
            journal
                .advance_title(id.clone(), TitleStage::Candidate { candidate })
                .await
                .is_err()
        );
    }
    let candidate = "\\".repeat(4096);
    journal
        .advance_title(
            id.clone(),
            TitleStage::Candidate {
                candidate: candidate.clone(),
            },
        )
        .await
        .unwrap();
    let bytes = std::fs::read(title_path(root.path(), &id)).unwrap();
    assert!(bytes.len() > 8192 && bytes.len() < MAX_TITLE_RECORD_BYTES as usize);
    drop(journal);
    let journal = JobJournal::open(root.path().join("jobs"), EventHub::new("vault".into(), 64))
        .await
        .unwrap();
    assert_eq!(public(&journal, &id).await.candidate, Some(candidate));
    assert_eq!(
        journal.get(id).await.unwrap().unwrap().state,
        ProcessingJobState::Succeeded
    );
}

#[tokio::test]
async fn old_preserve_layout_and_request_ids_remain_unchanged_and_policy_changes_conflict() {
    let root = tempfile::tempdir().unwrap();
    let journal = JobJournal::open(root.path().join("jobs"), EventHub::new("vault".into(), 64))
        .await
        .unwrap();
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut request: SummaryJobRequest = serde_json::from_str(
        r#"{"session_id":"note","target":{"kind":"summary"},"expected_revision":"revision"}"#,
    )
    .unwrap();
    let job = journal
        .submit("member".into(), request_id.clone(), request.clone())
        .await
        .unwrap();
    let path = root
        .path()
        .join("jobs")
        .join(format!("{}.json", key("member", &request_id)));
    let bytes = std::fs::read(&path).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert!(value.get("body_applied").is_none());
    assert!(value["request"].get("title_policy").is_none());
    request.title_policy = TitlePolicy::GenerateIfEmpty;
    assert_eq!(
        journal
            .submit("member".into(), request_id.clone(), request)
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Conflict
    );
    drop(journal);
    let journal = JobJournal::open(root.path().join("jobs"), EventHub::new("vault".into(), 64))
        .await
        .unwrap();
    assert_eq!(
        journal.get(job.id).await.unwrap().unwrap().state,
        ProcessingJobState::Queued
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

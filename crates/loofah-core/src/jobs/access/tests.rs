use super::*;
use std::collections::BTreeSet;

async fn fixture() -> (tempfile::TempDir, JobJournal) {
    let root = tempfile::tempdir().unwrap();
    let journal = JobJournal::open(root.path().join("jobs"), EventHub::new("vault".into(), 256))
        .await
        .unwrap();
    (root, journal)
}
async fn submit(journal: &JobJournal, owner: &str, session: &str) -> ProcessingJobSnapshot {
    journal
        .submit(
            owner.into(),
            uuid::Uuid::new_v4().to_string(),
            SummaryJobRequest {
                only_if_empty: false,
                title_policy: Default::default(),
                session_id: session.into(),
                target: DocumentTarget::Summary,
                expected_revision: "revision".into(),
            },
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn private_job_lookup_cancellation_and_results_do_not_disclose_other_owners() {
    let (_root, journal) = fixture().await;
    let job = submit(&journal, "alice", "note").await;
    for id in [job.id.clone(), uuid::Uuid::new_v4().to_string()] {
        assert_eq!(
            journal
                .owned_job("bob".into(), id.clone())
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            journal
                .cancel_owned("bob".into(), id.clone())
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            journal
                .owned_result("bob".into(), id)
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
    }
    assert_eq!(
        journal
            .owned_job("alice".into(), job.id.clone())
            .await
            .unwrap()
            .state,
        ProcessingJobState::Queued
    );
    assert!(
        journal
            .owned_result("alice".into(), job.id.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        journal
            .cancel_owned("alice".into(), job.id.clone())
            .await
            .unwrap()
            .state,
        ProcessingJobState::Cancelled
    );
    assert_eq!(
        journal
            .cancel_owned("alice".into(), job.id)
            .await
            .unwrap()
            .state,
        ProcessingJobState::Cancelled
    );
}

#[tokio::test]
async fn private_results_keep_their_vault_job_and_target_binding_after_cancel_and_restart() {
    let (root, journal) = fixture().await;
    let job = submit(&journal, "alice", "note").await;
    journal.claim(job.id.clone()).await.unwrap();
    journal
        .cancel_owned("alice".into(), job.id.clone())
        .await
        .unwrap();
    journal
        .retain_result(
            job.id.clone(),
            GeneratedDocument {
                markdown: "# Recovered result".into(),
                suggested_tags: Some(vec![hypr_vault_write::ScoredTagSuggestion {
                    name: "release".into(),
                    confidence: 0.95,
                }]),
            },
        )
        .await
        .unwrap();
    journal
        .complete(job.id.clone(), JobCompletion::Cancelled)
        .await
        .unwrap();
    drop(journal);
    let journal = JobJournal::open(root.path().join("jobs"), EventHub::new("vault".into(), 256))
        .await
        .unwrap();
    let result = journal
        .owned_result("alice".into(), job.id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.vault_id, "vault");
    assert_eq!(result.job_id, job.id);
    assert_eq!(result.session_id, "note");
    assert_eq!(result.target, DocumentTarget::Summary);
    assert_eq!(result.markdown, "# Recovered result");
    assert_eq!(result.suggested_tags.unwrap()[0].confidence, 0.95);
    assert_eq!(
        journal
            .owned_result("bob".into(), job.id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn pagination_filters_ownership_session_and_activity_before_applying_the_limit() {
    let (_root, journal) = fixture().await;
    let mut expected = BTreeSet::new();
    for i in 0..6 {
        let a = submit(
            &journal,
            "alice",
            if i % 2 == 0 { "first" } else { "second" },
        )
        .await;
        expected.insert(a.id);
        submit(&journal, "bob", "first").await;
    }
    let mut seen = BTreeSet::new();
    let mut cursor = None;
    loop {
        let page = journal
            .owned_page(
                "alice".into(),
                ListProcessingJobsInput {
                    limit: Some(1),
                    after: cursor,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(seen.insert(page.items[0].id.clone()));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, expected);
    let page = journal
        .owned_page(
            "alice".into(),
            ListProcessingJobsInput {
                session_id: Some("first".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 3);
    journal
        .cancel_owned("alice".into(), page.items[0].id.clone())
        .await
        .unwrap();
    let active = journal
        .owned_page(
            "alice".into(),
            ListProcessingJobsInput {
                session_id: Some("first".into()),
                active_only: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(active.items.len(), 2);
    assert!(
        active
            .items
            .iter()
            .all(|job| job.session_id == "first" && !job.state.is_terminal())
    );
    let foreign = submit(&journal, "bob", "first").await;
    assert_eq!(
        journal
            .owned_page(
                "alice".into(),
                ListProcessingJobsInput {
                    after: Some(foreign.id),
                    ..Default::default()
                }
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn invalid_owner_page_sizes_and_cursors_fail_before_accessing_records() {
    let (_root, journal) = fixture().await;
    for limit in [0, 201] {
        assert_eq!(
            journal
                .owned_page(
                    "alice".into(),
                    ListProcessingJobsInput {
                        limit: Some(limit),
                        ..Default::default()
                    }
                )
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }
    assert!(
        journal
            .owned_page(" ".into(), Default::default())
            .await
            .is_err()
    );
    assert!(
        journal
            .owned_page(
                "alice".into(),
                ListProcessingJobsInput {
                    after: Some("invalid".into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert!(
        journal
            .owned_page(
                "alice".into(),
                ListProcessingJobsInput {
                    session_id: Some("../outside".into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
}

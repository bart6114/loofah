use super::*;

async fn fixture() -> (tempfile::TempDir, JobJournal) {
    let root = tempfile::tempdir().unwrap();
    let journal = JobJournal::open(root.path().join("jobs"), EventHub::new("vault".into(), 8))
        .await
        .unwrap();
    (root, journal)
}
async fn claimed(journal: &JobJournal, namespace: &str) -> ProcessingJobSnapshot {
    let job = journal
        .submit(
            namespace.into(),
            uuid::Uuid::new_v4().to_string(),
            SummaryJobRequest {
                session_id: "note".into(),
                target: DocumentTarget::Summary,
                expected_revision: "original-revision".into(),
                title_policy: TitlePolicy::Preserve,
                only_if_empty: false,
            },
        )
        .await
        .unwrap();
    journal.claim(job.id.clone()).await.unwrap().unwrap();
    job
}
fn snapshot(text: &str) -> ProcessingPreviewSnapshot {
    ProcessingPreviewSnapshot {
        cursor: format!("{}:1", uuid::Uuid::new_v4()),
        markdown: text.into(),
        truncated: false,
    }
}
#[test]
fn preview_observer_coalesces_tiny_chunks_bounds_utf8_and_resets_attempts() {
    let (observer, mut updates) = GenerationPreview::channel();
    observer.reset();
    let first = updates.borrow_and_update().clone().unwrap();
    for _ in 0..10_000 {
        observer.update("partial", false);
    }
    assert!(!updates.has_changed().unwrap());
    observer.0.state.lock().unwrap().emitted -= INTERVAL;
    observer.publish_pending(false);
    assert_eq!(
        updates.borrow_and_update().as_ref().unwrap().markdown,
        "partial"
    );
    observer.update(&"🌱".repeat(MAX_PROCESSING_PREVIEW_BYTES), true);
    let bounded = updates.borrow_and_update().clone().unwrap();
    assert_eq!(bounded.markdown.len(), MAX_PROCESSING_PREVIEW_BYTES);
    assert!(bounded.truncated);
    assert_ne!(bounded.cursor, first.cursor);
    observer.reset();
    let reset = updates.borrow_and_update().clone().unwrap();
    assert!(reset.markdown.is_empty());
    assert_ne!(
        reset.cursor.split_once(':').unwrap().0,
        first.cursor.split_once(':').unwrap().0
    );
}
#[tokio::test]
async fn preview_is_private_retained_across_restart_and_never_requeues_inference() {
    let (root, journal) = fixture().await;
    let job = claimed(&journal, "member-a").await;
    let output = snapshot("# Partial\nNot saved");
    journal
        .checkpoint_preview(job.id.clone(), output.clone())
        .await
        .unwrap();
    assert_eq!(
        journal
            .owned_preview("member-b".into(), job.id.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert!(journal.result(job.id.clone()).await.unwrap().is_none());
    drop(journal);
    let journal = JobJournal::open(root.path().join("jobs"), EventHub::new("vault".into(), 8))
        .await
        .unwrap();
    let retained = journal
        .owned_preview("member-a".into(), job.id.clone())
        .await
        .unwrap();
    assert_eq!(retained.job.state, ProcessingJobState::Interrupted);
    assert_eq!(retained.preview, Some(output));
    assert!(journal.queued().await.unwrap().is_empty());
    assert!(journal.result(job.id.clone()).await.unwrap().is_none());
    let path = root
        .path()
        .join("jobs/previews")
        .join(format!("{}.json", job.id));
    let mut wrong: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    wrong["request"]["expected_revision"] = "new-revision".into();
    std::fs::write(&path, serde_json::to_vec(&wrong).unwrap()).unwrap();
    assert!(
        journal
            .owned_preview("member-a".into(), job.id)
            .await
            .unwrap()
            .preview
            .is_none()
    );
}
#[tokio::test]
async fn sidecar_retention_and_encoded_size_are_bounded_without_touching_job_results() {
    let (root, journal) = fixture().await;
    let mut first = None;
    for index in 0..=RETAINED {
        let job = claimed(&journal, "member").await;
        if index == 0 {
            first = Some(job.id.clone());
        }
        journal
            .checkpoint_preview(
                job.id.clone(),
                snapshot(&"\u{0001}".repeat(MAX_PROCESSING_PREVIEW_BYTES)),
            )
            .await
            .unwrap();
        journal.cancel(job.id.clone()).await.unwrap();
        journal
            .complete(job.id, JobCompletion::Cancelled)
            .await
            .unwrap();
    }
    let entries: Vec<_> = std::fs::read_dir(root.path().join("jobs/previews"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect();
    assert_eq!(entries.len(), RETAINED);
    assert!(
        entries
            .iter()
            .all(|entry| entry.metadata().unwrap().len() <= FILE_BYTES as u64)
    );
    let old = journal
        .owned_preview("member".into(), first.unwrap())
        .await
        .unwrap();
    assert_eq!(old.job.state, ProcessingJobState::Cancelled);
    assert!(old.preview.is_none());
    assert!(!root.path().join("jobs/results").exists());
}
#[test]
fn preview_debug_never_includes_text_or_target_identifiers() {
    assert!(!format!("{:?}", snapshot("PRIVATE-CONTENT-CANARY")).contains("CANARY"));
}

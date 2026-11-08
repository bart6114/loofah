use super::*;
use crate::{jobs::*, search::SearchService, settings::ConfigState, startup::StartupState};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

async fn context(root: &std::path::Path) -> Arc<VaultContext> {
    let vault = root.join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let hypr_storage::vault::LeaseAttempt::Acquired(lease) =
        hypr_storage::vault::VaultLease::try_acquire_existing(&vault).unwrap()
    else {
        panic!("lease")
    };
    let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
    let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Note","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
    store
        .create_note_session(&meta, "Original note")
        .await
        .unwrap();
    Arc::new(
        VaultContext::new(
            "vault".into(),
            store,
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.join("operations"))
        .with_vault_lease(lease)
        .unwrap(),
    )
}
fn declaration() -> ManualMediaSummaryDeclaration {
    ManualMediaSummaryDeclaration {
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id: "note".into(),
        fresh_session: false,
        expected_created_at: None,
        kind: ManualMediaKind::Subtitles,
        summary: Some(CaptureProcessingSummary {
            target: DocumentTarget::Summary,
            title_policy: TitlePolicy::Preserve,
        }),
        only_if_empty: false,
    }
}
#[derive(Default)]
struct Summary {
    calls: AtomicUsize,
    entered: Notify,
    release: Notify,
}
impl JobGenerator for Summary {
    fn generate(
        &self,
        work: JobWork,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<GeneratedDocument, ProcessingJobFailure>> {
        Box::pin(async move {
            assert_eq!(work.principal, OWNER);
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            Ok(GeneratedDocument {
                markdown: "Generated original summary".into(),
                suggested_tags: None,
            })
        })
    }
}
struct Auth;
impl JobAuthorizer for Auth {
    fn authorize<'a>(
        &'a self,
        work: &'a JobWork,
    ) -> JobFuture<'a, Result<Box<dyn Send>, ProcessingJobFailure>> {
        Box::pin(async move {
            assert_eq!(work.principal, OWNER);
            Ok(Box::new(()) as Box<dyn Send>)
        })
    }
}
async fn publish(context: &Arc<VaultContext>, request: &str, word_count: usize) {
    use hypr_vault_write::transcript_publication::*;
    let Some(SubtitleSourcePreparation::Ready(source)) = context
        .store
        .prepare_subtitle_source("note", ledger::hash(b"original SRT bytes"))
        .await
        .unwrap()
    else {
        panic!("source")
    };
    let words=(0..word_count).map(|index|serde_json::json!({"id":uuid::Uuid::new_v4().to_string(),"text":"This sentence supplies enough normalized Unicode characters for automatic summary eligibility.","start_ms":index*1000,"end_ms":index*1000+900,"channel":2})).collect::<Vec<_>>();
    let transcript=serde_json::from_value(serde_json::json!({"id":uuid::Uuid::new_v4().to_string(),"session_id":"note","user_id":uuid::Uuid::nil().to_string(),"created_at":"2026-11-08T10:00:00Z","started_at":1000,"memo_md":"","words":words,"speaker_hints":[]})).unwrap();
    let outcome = context
        .store
        .prepare_subtitle_publication(
            &context
                .operations
                .as_ref()
                .unwrap()
                .root
                .join("local-subtitles/publications"),
            TranscriptPublicationIdentity {
                vault_id: context.events.vault_id(),
                namespace: "local-subtitles".into(),
                request_id: uuid(request).unwrap(),
            },
            source,
            transcript,
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        TranscriptPublicationOutcome::Published { .. }
    ));
}
async fn terminal(context: &Arc<VaultContext>, request: &str) -> ManualMediaSummaryStatus {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let status = context.manual_media_summary_status(request).await.unwrap();
            if status
                .job
                .as_ref()
                .is_some_and(|job| job.state.is_terminal())
            {
                return status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn summary_requires_original_publication_and_replays_one_child_without_audio() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let context = context(&path).await;
    let summary = Arc::new(Summary::default());
    let worker = context
        .start_processing(summary.clone(), Arc::new(Auth))
        .await
        .unwrap();
    let declaration = declaration();
    let original = context
        .declare_manual_media_summary(declaration.clone())
        .await
        .unwrap();
    assert!(
        context
            .complete_manual_media_summary(declaration.request_id.clone())
            .await
            .is_err()
    );
    assert_eq!(summary.calls.load(Ordering::SeqCst), 0);
    publish(&context, &declaration.request_id, 5).await;
    let queued = context
        .complete_manual_media_summary(declaration.request_id.clone())
        .await
        .unwrap();
    assert_eq!(queued.child_request_id, original.child_request_id);
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        summary.entered.notified(),
    )
    .await
    .unwrap();
    assert!(context.try_session_lifecycle_change("note").is_err());
    summary.release.notify_one();
    let done = terminal(&context, &declaration.request_id).await;
    assert_eq!(
        done.job.as_ref().unwrap().state,
        ProcessingJobState::Succeeded
    );
    std::fs::remove_file(path.join("vault/sessions/note/transcript.json")).unwrap();
    // The child can commit before the parent checkpoints its returned job ID.
    let mut lost_parent = journal::load(&context, &declaration.request_id)
        .await
        .unwrap()
        .unwrap();
    lost_parent.status.state = ManualMediaSummaryState::SourceBound;
    lost_parent.status.job = None;
    journal::persist(&context, &lost_parent, false)
        .await
        .unwrap();
    let replay = context
        .complete_manual_media_summary(declaration.request_id.clone())
        .await
        .unwrap();
    assert_eq!(replay.job.unwrap().id, done.job.unwrap().id);
    assert_eq!(summary.calls.load(Ordering::SeqCst), 1);
    assert!(!path.join("vault/sessions/note/transcript.json").exists());
    worker.shutdown().await.unwrap();
}
#[tokio::test]
async fn changed_raw_transcript_during_summary_blocks_publication_and_never_reruns() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let context = context(&path).await;
    let summary = Arc::new(Summary::default());
    let worker = context
        .start_processing(summary.clone(), Arc::new(Auth))
        .await
        .unwrap();
    let declaration = declaration();
    context
        .declare_manual_media_summary(declaration.clone())
        .await
        .unwrap();
    publish(&context, &declaration.request_id, 5).await;
    context
        .complete_manual_media_summary(declaration.request_id.clone())
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        summary.entered.notified(),
    )
    .await
    .unwrap();
    let file = path.join("vault/sessions/note/transcript.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    value["future_user_edit"] = true.into();
    std::fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
    summary.release.notify_one();
    let status = terminal(&context, &declaration.request_id).await;
    assert_eq!(status.job.unwrap().state, ProcessingJobState::Conflict);
    assert!(
        context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .is_none()
    );
    context
        .complete_manual_media_summary(declaration.request_id.clone())
        .await
        .unwrap();
    assert_eq!(summary.calls.load(Ordering::SeqCst), 1);
    worker.shutdown().await.unwrap();
}
#[tokio::test]
async fn frozen_summary_target_and_declaration_refuse_rebinding_before_inference() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let context = context(&path).await;
    let summary = Arc::new(Summary::default());
    let worker = context
        .start_processing(summary.clone(), Arc::new(Auth))
        .await
        .unwrap();
    let declaration = declaration();
    context
        .declare_manual_media_summary(declaration.clone())
        .await
        .unwrap();
    let mut changed = declaration.clone();
    changed.only_if_empty = true;
    assert_eq!(
        context
            .declare_manual_media_summary(changed)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    publish(&context, &declaration.request_id, 5).await;
    std::fs::write(
        path.join("vault/sessions/note/summary.md"),
        "User-written summary",
    )
    .unwrap();
    assert_eq!(
        context
            .complete_manual_media_summary(declaration.request_id.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(summary.calls.load(Ordering::SeqCst), 0);
    worker.shutdown().await.unwrap();
}
#[tokio::test]
async fn unqualified_or_disabled_automatic_summary_is_a_durable_skip() {
    for disabled in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        let context = context(&path).await;
        let summary = Arc::new(Summary::default());
        let worker = context
            .start_processing(summary.clone(), Arc::new(Auth))
            .await
            .unwrap();
        let mut declaration = declaration();
        if disabled {
            declaration.summary = None;
        }
        context
            .declare_manual_media_summary(declaration.clone())
            .await
            .unwrap();
        if !disabled {
            publish(&context, &declaration.request_id, 1).await;
        }
        let status = context
            .complete_manual_media_summary(declaration.request_id.clone())
            .await
            .unwrap();
        assert_eq!(status.state, ManualMediaSummaryState::Skipped);
        assert!(status.job.is_none());
        assert_eq!(summary.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            context
                .complete_manual_media_summary(declaration.request_id.clone())
                .await
                .unwrap()
                .state,
            ManualMediaSummaryState::Skipped
        );
        worker.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn manual_summary_status_never_initializes_parent_storage() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let context = context(&path).await;
    let operation = context
        .operations
        .as_ref()
        .unwrap()
        .root
        .join("manual-media-summary");
    assert!(!operation.exists());
    assert_eq!(
        context
            .manual_media_summary_status(&uuid::Uuid::new_v4().to_string())
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert!(!operation.exists());
}

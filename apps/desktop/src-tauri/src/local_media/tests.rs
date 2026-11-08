use super::*;
use tauri::Manager;
async fn context(root: &std::path::Path, id: &str) -> Arc<VaultContext> {
    let vault = root.join(id);
    std::fs::create_dir(&vault).unwrap();
    let hypr_storage::vault::LeaseAttempt::Acquired(lease) =
        hypr_storage::vault::VaultLease::try_acquire_existing(&vault).unwrap()
    else {
        panic!("owned fixture")
    };
    Arc::new(
        VaultContext::new(
            "same-id".into(),
            Arc::new(hypr_vault_write::SessionStore::new(vault.clone())),
            hypr_loofah_core::startup::StartupState::new(Some(&vault)),
            Arc::new(hypr_loofah_core::settings::ConfigState::load_or_default(
                &vault,
            )),
            Arc::new(hypr_loofah_core::search::SearchService::default()),
        )
        .with_operation_storage(root.join(format!("{id}-operations")))
        .with_vault_lease(lease)
        .unwrap(),
    )
}
#[tokio::test]
async fn media_inventory_requires_initialized_original_context_and_never_opens_remote_local_state()
{
    let app = tauri::test::mock_builder()
        .manage(LocalMediaState::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    assert!(!commands::application_local_media_available(
        app.handle().clone()
    ));
    assert!(
        commands::application_local_media_list(app.handle().clone(), None)
            .await
            .is_err()
    );
    assert!(app.state::<LocalMediaState>().service.get().is_none());
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let first = context(root.path(), "first").await;
    let second = context(root.path(), "second").await;
    let state = LocalMediaState::default();
    state.initialize(first.clone()).unwrap();
    assert!(state.service_for(&second).is_err());
    assert!(state.initialize(second).is_err());
    assert!(
        state
            .service_for(&first)
            .unwrap()
            .list(None)
            .await
            .unwrap()
            .is_empty()
    );
    state.request_shutdown();
    assert!(state.service_for(&first).is_err());
    assert!(state.drain(std::time::Duration::from_secs(1)).await);
}
#[tokio::test]
async fn cancel_does_not_claim_completion_or_release_original_task_before_worker_join() {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let original = context(root.path(), "owner").await;
    let state = LocalMediaState::default();
    state.initialize(original.clone()).unwrap();
    let service = state.service_for(&original).unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let task = Arc::new(Task {
        fingerprint: "original".into(),
        status: Mutex::new(LocalMediaStatus {
            request_id: request.clone(),
            session_id: "note".into(),
            kind: LocalMediaKind::Transcribe,
            state: MediaState::Running,
            transcript_id: None,
            word_count: None,
            error: None,
            imported: None,
        }),
        admitted: CancellationToken::new(),
        done: CancellationToken::new(),
        cancel: service.stop.child_token(),
        safe: std::sync::atomic::AtomicBool::new(false),
    });
    service
        .tasks
        .lock()
        .unwrap()
        .insert(request.clone(), task.clone());
    assert_eq!(
        service.cancel(&request).await.unwrap().state,
        MediaState::Running
    );
    assert!(task.cancel.is_cancelled());
    assert!(!task.done.is_cancelled());
    assert!(!state.drain(std::time::Duration::from_millis(1)).await);
    task.safe.store(true, std::sync::atomic::Ordering::Release);
    task.done.cancel();
    assert!(state.drain(std::time::Duration::from_secs(1)).await);
}
struct HeldMedia {
    host: Arc<NativeMediaHost>,
    entered: CancellationToken,
    release: Arc<tokio::sync::Semaphore>,
    released: Arc<std::sync::atomic::AtomicBool>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
impl LocalMediaHost for HeldMedia {
    fn cancellation(&self, request: &str) -> CancellationToken {
        self.host.cancellation(request)
    }
    fn normalize(
        &self,
        _: LocalCliArtifact,
        _: String,
        _: PathBuf,
        _: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<LocalCliArtifact> {
        unreachable!()
    }
    fn transcribe(
        &self,
        _: String,
        source: LocalCliArtifact,
        _: String,
        session: String,
        _: hypr_vault_read::SessionAudio,
        _: AppConfig,
        _: PathBuf,
        _: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<LocalMediaTranscription> {
        let entered = self.entered.clone();
        let release = self.release.clone();
        let released = self.released.clone();
        let calls = self.calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            entered.cancel();
            let _permit = release.acquire().await.unwrap();
            drop(source);
            released.store(true, std::sync::atomic::Ordering::Release);
            Ok(serde_json::from_value::<TranscriptWithData>(serde_json::json!({"id":uuid::Uuid::new_v4().to_string(),"user_id":"00000000-0000-0000-0000-000000000000","session_id":session,"created_at":"2026-11-08T10:00:00Z","started_at":0,"ended_at":null,"memo_md":"","words":[{"id":"word","text":"Retained","start_ms":0,"end_ms":100,"channel":0}],"speaker_hints":[]})).unwrap().into())
        })
    }
}
#[tokio::test]
async fn native_media_cancel_retains_actual_source_until_join_and_recovers_output_without_replaying_model()
 {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let vault = root.path().join("vault");
    std::fs::create_dir(&vault).unwrap();
    let hypr_storage::vault::LeaseAttempt::Acquired(lease) =
        hypr_storage::vault::VaultLease::try_acquire_existing(&vault).unwrap()
    else {
        panic!("owned fixture")
    };
    let runtime = hypr_loofah_core::VaultRuntime::open_leased(
        "local:test".into(),
        lease.clone(),
        &root.path().join("cache"),
        &root.path().join("operations"),
        hypr_vault_write::ContentLimits::unlimited(),
    )
    .await
    .unwrap();
    runtime.context.startup.wait_until_ready().await.unwrap();
    runtime
        .context
        .enable_standalone_local_session_lifecycle(&lease)
        .unwrap();
    let context = runtime.context.clone();
    let cli = Arc::new(LocalCli::new(context.clone()).unwrap());
    let output = cli
        .execute(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::CreateSession {
                title: "Original".into(),
                markdown: None,
                metadata: Default::default(),
            },
            None,
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    let LocalCliReply::SessionCreated { meta } = output.reply else {
        panic!("created session")
    };
    std::fs::write(
        vault.join("sessions").join(&meta.id).join("audio.mp3"),
        b"original source",
    )
    .unwrap();
    let stop = CancellationToken::new();
    let host = Arc::new(NativeMediaHost {
        context: context.clone(),
        stop: stop.clone(),
        requests: Default::default(),
    });
    let entered = CancellationToken::new();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cli = Arc::new(
        LocalCli::new(context.clone())
            .unwrap()
            .with_media_host(Arc::new(HeldMedia {
                host: host.clone(),
                entered: entered.clone(),
                release: release.clone(),
                released: released.clone(),
                calls: calls.clone(),
            })),
    );
    let expected = cli.media_source(&meta.id).await.unwrap();
    let service = Arc::new(Service {
        context: context.clone(),
        cli,
        host,
        tasks: Default::default(),
        stop,
    });
    let request = uuid::Uuid::new_v4().to_string();
    let admitted = service
        .submit(
            request.clone(),
            LocalCliOperation::TranscribeChecked {
                session_id: meta.id.clone(),
                expected,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(admitted.state, MediaState::Running);
    entered.cancelled().await;
    assert_eq!(
        service.cancel(&request).await.unwrap().state,
        MediaState::Running
    );
    assert!(!released.load(std::sync::atomic::Ordering::Acquire));
    assert!(
        !vault
            .join("sessions")
            .join(&meta.id)
            .join("transcript.json")
            .exists()
    );
    // The held source pins only its original session, leaving unrelated notes responsive.
    service
        .cli
        .execute(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::CreateSession {
                title: "Other".into(),
                markdown: None,
                metadata: Default::default(),
            },
            None,
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    let done = service
        .tasks
        .lock()
        .unwrap()
        .get(&request)
        .unwrap()
        .done
        .clone();
    release.add_permits(1);
    done.cancelled().await;
    assert!(released.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(
        service.status(&request).await.unwrap().state,
        MediaState::Review
    );
    let recovered = service.recover(request.clone()).await.unwrap();
    assert_eq!(recovered.state, MediaState::Succeeded);
    assert_eq!(recovered.word_count, Some(1));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        vault
            .join("sessions")
            .join(&meta.id)
            .join("transcript.json")
            .exists()
    );
    drop(service);
    drop(context);
    runtime.shutdown().await.unwrap();
}

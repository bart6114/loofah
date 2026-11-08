use super::*;
use hypr_loofah_core::{
    VaultRuntime,
    local_cli::{
        LocalCli, LocalCliArtifact, LocalMediaFuture, LocalMediaHost, LocalMediaTranscription,
    },
    settings::AppConfig,
};
use hypr_loofah_protocol::LocalCliProgress;

struct PanicMedia {
    release: Arc<std::sync::Mutex<std::sync::mpsc::Receiver<()>>>,
    finished: Arc<std::sync::atomic::AtomicBool>,
}
impl LocalMediaHost for PanicMedia {
    fn normalize(
        &self,
        source: LocalCliArtifact,
        _: String,
        _: PathBuf,
        _: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<LocalCliArtifact> {
        let release = self.release.clone();
        let finished = self.finished.clone();
        Box::pin(async move {
            let (started, entered) = tokio::sync::oneshot::channel();
            tokio::task::spawn_blocking(move || {
                let _source = source;
                let _ = started.send(());
                let _ = release.lock().unwrap().recv();
                finished.store(true, Ordering::SeqCst);
            });
            entered.await.unwrap();
            panic!("media supervisor lost with admitted I/O still active");
        })
    }
    fn transcribe(
        &self,
        _: String,
        _: LocalCliArtifact,
        _: String,
        _: String,
        _: hypr_vault_read::SessionAudio,
        _: AppConfig,
        _: PathBuf,
        _: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<LocalMediaTranscription> {
        unreachable!()
    }
}

#[tokio::test]
async fn core_media_supervisor_panic_retains_owner_while_actual_io_outlives_handler() {
    let fixture = Fixture::new();
    let lease = fixture.lease();
    let original = Arc::downgrade(&lease.pin());
    let runtime = VaultRuntime::open_leased(
        "local-panic".into(),
        lease.clone(),
        &fixture._root.path().join("cache"),
        &fixture._root.path().join("operations"),
        Default::default(),
    )
    .await
    .unwrap();
    runtime.context.startup.wait_until_ready().await.unwrap();
    runtime
        .context
        .search
        .wait_until_initialized()
        .await
        .unwrap();
    let (release, receiver) = std::sync::mpsc::channel();
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cli = Arc::new(
        LocalCli::new(runtime.context.clone())
            .unwrap()
            .with_media_host(Arc::new(PanicMedia {
                release: Arc::new(std::sync::Mutex::new(receiver)),
                finished: finished.clone(),
            })),
    );
    let server = OwnerServer::bind(
        lease,
        &fixture.endpoints,
        &fixture.temporary,
        Arc::new(CoreOwner::new(cli, fixture.temporary.clone())),
    )
    .await
    .unwrap();
    let mut source = tempfile::tempfile().unwrap();
    std::io::Write::write_all(&mut source, b"input audio").unwrap();
    std::io::Seek::rewind(&mut source).unwrap();
    let blob = Blob::copy_from(source, &fixture.temporary).await.unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let result = fixture
        .client()
        .await
        .execute(
            request.clone(),
            LocalCliOperation::Import {
                filename: "input.mp3".into(),
                title: None,
                into: None,
                transcribe: false,
                metadata: Default::default(),
            },
            Some(blob),
        )
        .await;
    assert!(matches!(result, Err(Error::Unknown { request_id }) if request_id == request));
    assert!(!finished.load(Ordering::SeqCst));
    assert!(matches!(
        server.shutdown().await,
        Err(Error::ApplicationWorkerLost)
    ));
    assert!(original.upgrade().is_some());
    assert!(matches!(
        VaultLease::try_acquire_existing(&fixture.vault).unwrap(),
        LeaseAttempt::Busy
    ));
    release.send(()).unwrap();
    until(|| finished.load(Ordering::SeqCst)).await;
    runtime.shutdown().await.unwrap();
    assert!(matches!(
        VaultLease::try_acquire_existing(&fixture.vault).unwrap(),
        LeaseAttempt::Busy
    ));
}

use super::*;
use std::sync::{Condvar, atomic::AtomicUsize};
use std::time::Duration;

#[derive(Default)]
struct Gate {
    entered: AtomicBool,
    released: Mutex<bool>,
    wake: Condvar,
}

impl Gate {
    fn wait(&self) {
        self.entered.store(true, Ordering::Release);
        let mut released = self.released.lock().unwrap();
        while !*released {
            let (value, timeout) = self
                .wake
                .wait_timeout(released, Duration::from_secs(10))
                .unwrap();
            released = value;
            assert!(!timeout.timed_out(), "test gate was not released");
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }

    async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.entered.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }
}

struct FakeEngine {
    native: Option<Arc<Gate>>,
    release: Option<Arc<Gate>>,
    events: usize,
    panic: bool,
    calls: Arc<AtomicUsize>,
}

impl Drop for FakeEngine {
    fn drop(&mut self) {
        if let Some(gate) = &self.release {
            gate.wait();
        }
    }
}

impl Engine for FakeEngine {
    fn batch(
        &self,
        _source: &TrustedBatchSource,
        _params: &ListenParams,
        _cancelled: Arc<AtomicBool>,
        emit: &mut dyn FnMut(BatchSseMessage) -> Result<(), crate::Error>,
    ) -> Result<batch::Response, crate::Error> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        if let Some(gate) = &self.native {
            gate.wait();
        }
        assert!(!self.panic, "fake native panic");
        for _ in 0..self.events {
            emit(BatchSseMessage::Error {
                error: "fake progress".into(),
                detail: String::new(),
            })?;
        }
        Ok(response())
    }
}

fn response() -> batch::Response {
    batch::Response {
        metadata: serde_json::json!({"fake": true}),
        results: batch::Results {
            channels: Vec::new(),
        },
    }
}

fn fake(
    native: Option<Arc<Gate>>,
    release: Option<Arc<Gate>>,
    events: usize,
    panic: bool,
) -> FakeEngine {
    FakeEngine {
        native,
        release,
        events,
        panic,
        calls: Arc::new(AtomicUsize::new(0)),
    }
}

async fn ready(engine: FakeEngine) -> WhisperResources {
    let host = WhisperResources::start(Limits::default(), move || Ok(Arc::new(engine))).unwrap();
    phase(&mut host.observer(), ResourcePhase::Ready).await;
    host
}

async fn phase(observer: &mut ResourceObserver, expected: ResourcePhase) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while observer.snapshot().phase != expected {
            assert!(
                observer.changed().await.is_some(),
                "resource owner disappeared"
            );
        }
    })
    .await
    .unwrap();
}

async fn finished(observer: &mut BatchObserver) -> Arc<Result<batch::Response, ResourceError>> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let BatchSnapshot::Finished(result) = observer.snapshot() {
                return result;
            }
            assert!(
                observer.changed().await.is_some(),
                "batch owner disappeared"
            );
        }
    })
    .await
    .unwrap()
}

fn source(owner: impl Send + 'static) -> TrustedBatchSource {
    TrustedBatchSource::new(
        std::env::temp_dir().join("trusted-existing-source.wav"),
        owner,
    )
    .unwrap()
}

struct SourceOwner {
    released: Arc<AtomicBool>,
    gate: Option<Arc<Gate>>,
}

impl Drop for SourceOwner {
    fn drop(&mut self) {
        if let Some(gate) = &self.gate {
            gate.wait();
        }
        self.released.store(true, Ordering::Release);
    }
}

#[tokio::test]
async fn idle_batch_works_and_completion_is_replayable_after_exact_owner_release() {
    let host = ready(fake(None, None, 0, false)).await;
    let released = Arc::new(AtomicBool::new(false));
    let batch = host
        .start_batch(
            host.try_batch().unwrap(),
            source(SourceOwner {
                released: released.clone(),
                gate: None,
            }),
            ListenParams::default(),
        )
        .unwrap();
    let mut observer = batch.observer();
    assert!(finished(&mut observer).await.is_ok());
    assert!(released.load(Ordering::Acquire));
    assert_eq!(host.observer().snapshot().batch_groups, 0);
    assert!(finished(&mut observer.clone()).await.is_ok());
    phase(&mut host.shutdown(), ResourcePhase::Stopped).await;
}

#[tokio::test]
async fn live_and_batch_exclude_each_other_by_default_without_waiters() {
    let host = ready(fake(None, None, 0, false)).await;
    let live = host.try_live(2).unwrap();
    assert!(matches!(host.try_batch(), Err(ResourceError::Busy)));
    assert!(matches!(host.try_live(1), Err(ResourceError::Busy)));
    assert_eq!(host.observer().snapshot().live_channels, 2);
    drop(live);
    let batch = host.try_batch().unwrap();
    for _ in 0..1024 {
        assert!(matches!(host.try_live(2), Err(ResourceError::Busy)));
    }
    assert_eq!(host.inner.lock().cancellations.len(), 1);
    drop(batch);
    assert!(matches!(host.try_live(0), Err(ResourceError::Invalid(_))));
    assert!(matches!(host.try_live(3), Err(ResourceError::Invalid(_))));
    phase(&mut host.shutdown(), ResourcePhase::Stopped).await;
}

#[tokio::test]
async fn cancellation_and_drop_retain_source_and_capacity_until_native_returns() {
    let native = Arc::new(Gate::default());
    let host = ready(fake(Some(native.clone()), None, 0, false)).await;
    let released = Arc::new(AtomicBool::new(false));
    let batch = host
        .start_batch(
            host.try_batch().unwrap(),
            source(SourceOwner {
                released: released.clone(),
                gate: None,
            }),
            ListenParams::default(),
        )
        .unwrap();
    let mut observer = batch.observer();
    native.entered().await;
    drop(batch);
    assert!(matches!(observer.snapshot(), BatchSnapshot::Running));
    assert!(!released.load(Ordering::Acquire));
    assert!(matches!(host.try_live(2), Err(ResourceError::Busy)));
    native.release();
    assert!(matches!(
        finished(&mut observer).await.as_ref(),
        Err(ResourceError::Cancelled)
    ));
    assert!(released.load(Ordering::Acquire));
    assert!(host.try_live(2).is_ok());
    phase(&mut host.shutdown(), ResourcePhase::Stopped).await;
}

#[tokio::test]
async fn source_destructor_is_inside_actual_completion_and_capacity_barrier() {
    let drop_gate = Arc::new(Gate::default());
    let host = ready(fake(None, None, 0, false)).await;
    let released = Arc::new(AtomicBool::new(false));
    let batch = host
        .start_batch(
            host.try_batch().unwrap(),
            source(SourceOwner {
                released: released.clone(),
                gate: Some(drop_gate.clone()),
            }),
            ListenParams::default(),
        )
        .unwrap();
    let mut observer = batch.observer();
    drop_gate.entered().await;
    assert!(matches!(observer.snapshot(), BatchSnapshot::Running));
    assert!(matches!(host.try_live(1), Err(ResourceError::Busy)));
    drop_gate.release();
    assert!(finished(&mut observer).await.is_ok());
    assert!(released.load(Ordering::Acquire));
    phase(&mut host.shutdown(), ResourcePhase::Stopped).await;
}

#[tokio::test]
async fn shutdown_deadline_is_observation_only_and_waits_for_native_and_model_drop() {
    let native = Arc::new(Gate::default());
    let model_drop = Arc::new(Gate::default());
    let host = ready(fake(
        Some(native.clone()),
        Some(model_drop.clone()),
        0,
        false,
    ))
    .await;
    let batch = host
        .start_batch(
            host.try_batch().unwrap(),
            source(()),
            ListenParams::default(),
        )
        .unwrap();
    let mut completion = batch.observer();
    native.entered().await;
    let mut stopped = host.shutdown();
    assert!(matches!(host.try_live(2), Err(ResourceError::Stopping)));
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            phase(&mut stopped, ResourcePhase::Stopped)
        )
        .await
        .is_err()
    );
    assert!(matches!(completion.snapshot(), BatchSnapshot::Running));
    native.release();
    assert!(matches!(
        finished(&mut completion).await.as_ref(),
        Err(ResourceError::Cancelled)
    ));
    model_drop.entered().await;
    assert_eq!(stopped.snapshot().phase, ResourcePhase::Stopping);
    drop(host);
    model_drop.release();
    phase(&mut stopped, ResourcePhase::Stopped).await;
}

#[tokio::test]
async fn full_unread_progress_queue_never_blocks_native_completion_or_loses_terminal_result() {
    let host = ready(fake(None, None, 100, false)).await;
    let batch = host
        .start_batch(
            host.try_batch().unwrap(),
            source(()),
            ListenParams::default(),
        )
        .unwrap();
    let mut observer = batch.observer();
    let terminal = finished(&mut observer).await;
    assert_eq!(terminal.as_ref().as_ref().unwrap(), &response());
    assert_eq!(batch.events.len(), Limits::default().progress_capacity);
    assert!(host.try_live(2).is_ok());
    phase(&mut host.shutdown(), ResourcePhase::Stopped).await;
}

#[tokio::test]
async fn closed_progress_queue_does_not_cancel_authoritative_result() {
    let native = Arc::new(Gate::default());
    let host = ready(fake(Some(native.clone()), None, 100, false)).await;
    let mut batch = host
        .start_batch(
            host.try_batch().unwrap(),
            source(()),
            ListenParams::default(),
        )
        .unwrap();
    native.entered().await;
    batch.events.close();
    native.release();
    assert_eq!(
        finished(&mut batch.observer())
            .await
            .as_ref()
            .as_ref()
            .unwrap(),
        &response()
    );
    phase(&mut host.shutdown(), ResourcePhase::Stopped).await;
}

#[tokio::test]
async fn native_panic_releases_source_and_reports_actual_failed_completion() {
    let host = ready(fake(None, None, 0, true)).await;
    let released = Arc::new(AtomicBool::new(false));
    let batch = host
        .start_batch(
            host.try_batch().unwrap(),
            source(SourceOwner {
                released: released.clone(),
                gate: None,
            }),
            ListenParams::default(),
        )
        .unwrap();
    assert!(matches!(
        finished(&mut batch.observer()).await.as_ref(),
        Err(ResourceError::WorkerPanicked)
    ));
    assert!(released.load(Ordering::Acquire));
    assert!(host.try_batch().is_ok());
    phase(&mut host.shutdown(), ResourcePhase::Stopped).await;
}

#[tokio::test]
async fn startup_is_single_owned_work_and_shutdown_survives_caller_drop() {
    let load = Arc::new(Gate::default());
    let worker_gate = load.clone();
    let host = WhisperResources::start(Limits::default(), move || {
        worker_gate.wait();
        Ok(Arc::new(fake(None, None, 0, false)))
    })
    .unwrap();
    let mut observer = host.observer();
    load.entered().await;
    for _ in 0..1024 {
        assert!(matches!(host.try_batch(), Err(ResourceError::Loading)));
    }
    drop(host);
    assert_eq!(observer.snapshot().phase, ResourcePhase::Stopping);
    load.release();
    phase(&mut observer, ResourcePhase::Stopped).await;
}

#[tokio::test]
async fn failed_and_panicked_startup_never_become_ready() {
    for panic in [false, true] {
        let host = WhisperResources::start(Limits::default(), move || {
            assert!(!panic, "fake load panic");
            Err(ResourceError::ModelChanged)
        })
        .unwrap();
        let mut observer = host.observer();
        let expected = if panic {
            ResourceError::WorkerPanicked
        } else {
            ResourceError::ModelChanged
        };
        phase(&mut observer, ResourcePhase::Failed(expected)).await;
        assert!(matches!(host.try_batch(), Err(ResourceError::Unavailable)));
        phase(&mut host.shutdown(), ResourcePhase::Stopped).await;
    }
}

#[tokio::test]
async fn live_reservation_is_cancelled_but_not_falsely_drained_by_shutdown() {
    let host = ready(fake(None, None, 0, false)).await;
    let live = host.try_live(2).unwrap();
    let mut stopped = host.shutdown();
    assert!(live.is_cancelled());
    assert_eq!(stopped.snapshot().phase, ResourcePhase::Stopping);
    drop(live);
    phase(&mut stopped, ResourcePhase::Stopped).await;
}

#[tokio::test]
async fn foreign_admission_cannot_run_on_another_model_owner() {
    let first = ready(fake(None, None, 0, false)).await;
    let second = ready(fake(None, None, 0, false)).await;
    let released = Arc::new(AtomicBool::new(false));
    assert!(matches!(
        second.start_batch(
            first.try_batch().unwrap(),
            source(SourceOwner {
                released: released.clone(),
                gate: None,
            }),
            ListenParams::default()
        ),
        Err(ResourceError::Invalid("admission owner"))
    ));
    assert!(released.load(Ordering::Acquire));
    assert!(first.try_batch().is_ok());
    phase(&mut first.shutdown(), ResourcePhase::Stopped).await;
    phase(&mut second.shutdown(), ResourcePhase::Stopped).await;
}

#[tokio::test]
async fn explicit_operator_capacity_is_bounded_and_does_not_share_admissions() {
    let host = WhisperResources::start(
        Limits {
            workload_groups: 2,
            ..Limits::default()
        },
        || Ok(Arc::new(fake(None, None, 0, false))),
    )
    .unwrap();
    phase(&mut host.observer(), ResourcePhase::Ready).await;
    let batch = host.try_batch().unwrap();
    let live = host.try_live(2).unwrap();
    assert!(matches!(host.try_batch(), Err(ResourceError::Busy)));
    assert!(matches!(host.try_live(1), Err(ResourceError::Busy)));
    assert_eq!(
        host.observer().snapshot(),
        ResourceSnapshot {
            phase: ResourcePhase::Ready,
            batch_groups: 1,
            live_groups: 1,
            live_channels: 2,
        }
    );
    drop(batch);
    let replacement = host.try_batch().unwrap();
    drop(replacement);
    drop(live);
    phase(&mut host.shutdown(), ResourcePhase::Stopped).await;
}

#[tokio::test]
async fn stopping_before_dispatch_never_runs_native_and_releases_unsent_source() {
    let engine = fake(None, None, 0, false);
    let calls = engine.calls.clone();
    let host = ready(engine).await;
    let admission = host.try_batch().unwrap();
    let mut stopped = host.shutdown();
    let released = Arc::new(AtomicBool::new(false));
    assert!(matches!(
        host.start_batch(
            admission,
            source(SourceOwner {
                released: released.clone(),
                gate: None,
            }),
            ListenParams::default()
        ),
        Err(ResourceError::Stopping)
    ));
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert!(released.load(Ordering::Acquire));
    phase(&mut stopped, ResourcePhase::Stopped).await;
}

fn model_spec(path: PathBuf, contents: &[u8]) -> ModelSpec {
    ModelSpec {
        absolute_path: path,
        sha256: digest_hex(Sha256::digest(contents).as_ref()),
        size_bytes: contents.len() as u64,
    }
}

#[test]
fn model_identity_is_checked_before_and_after_loading_without_unbounded_reads() {
    assert_eq!(
        digest_hex(Sha256::digest(b"abc").as_ref()),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("model.bin");
    std::fs::write(&path, b"original").unwrap();
    let spec = model_spec(path.clone(), b"original");
    assert!(verify_model(&spec).is_ok());
    let called = AtomicBool::new(false);
    let result = load_verified(&spec, || {
        called.store(true, Ordering::Release);
        std::fs::write(&path, b"replaced").unwrap();
        Ok(())
    });
    assert!(called.load(Ordering::Acquire));
    assert!(matches!(result, Err(ResourceError::ModelChanged)));
    assert!(matches!(
        load_verified::<()>(&spec, || panic!("must reject before load")),
        Err(ResourceError::ModelChanged)
    ));
    std::fs::write(&path, b"original-extra").unwrap();
    assert!(matches!(
        verify_model(&spec),
        Err(ResourceError::ModelChanged)
    ));
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(matches!(
        verify_model(&spec),
        Err(ResourceError::ModelChanged)
    ));
}

#[test]
fn invalid_configuration_and_relative_source_paths_are_rejected() {
    let valid = model_spec(std::env::temp_dir().join("model.bin"), b"model");
    for groups in [0, 9, usize::MAX] {
        assert!(
            validate(
                &valid,
                Limits {
                    workload_groups: groups,
                    ..Limits::default()
                }
            )
            .is_err()
        );
    }
    for progress in [0, 65, usize::MAX] {
        assert!(
            validate(
                &valid,
                Limits {
                    progress_capacity: progress,
                    ..Limits::default()
                }
            )
            .is_err()
        );
    }
    let mut invalid = valid.clone();
    invalid.sha256 = "A".repeat(64);
    assert!(validate(&invalid, Limits::default()).is_err());
    invalid = valid.clone();
    invalid.size_bytes = 0;
    assert!(validate(&invalid, Limits::default()).is_err());
    invalid = valid;
    invalid.absolute_path = "relative.bin".into();
    assert!(validate(&invalid, Limits::default()).is_err());
    assert!(TrustedBatchSource::new("relative.wav".into(), ()).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_model_leaf_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("model.bin");
    let link = directory.path().join("link.bin");
    std::fs::write(&path, b"original").unwrap();
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(matches!(
        verify_model(&model_spec(link, b"original")),
        Err(ResourceError::ModelChanged)
    ));
}

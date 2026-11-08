use super::*;
use std::{sync::atomic::AtomicUsize, time::Duration};
use tokio::sync::{oneshot, watch};

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct FakeDriver {
    status: watch::Receiver<BatchSnapshot>,
    cancellations: Arc<AtomicUsize>,
    panic_once: AtomicBool,
    final_on_close: Option<BatchSnapshot>,
    final_snapshot: Option<BatchSnapshot>,
}
impl BatchDriver for FakeDriver {
    fn snapshot(&self) -> BatchSnapshot {
        assert!(
            !self.panic_once.swap(false, Ordering::AcqRel),
            "driver fixture panic"
        );
        self.final_snapshot
            .clone()
            .unwrap_or_else(|| self.status.borrow().clone())
    }
    fn changed(&mut self) -> TranscriptionFuture<'_, Option<BatchSnapshot>> {
        if let Some(final_snapshot) = self.final_on_close.take() {
            self.final_snapshot = Some(final_snapshot);
            return Box::pin(async { None });
        }
        Box::pin(async move {
            self.status.changed().await.ok()?;
            Some(self.snapshot())
        })
    }
    fn cancel(&mut self) {
        self.cancellations.fetch_add(1, Ordering::AcqRel);
    }
}

fn response() -> batch::Response {
    batch::Response {
        metadata: serde_json::json!({}),
        results: batch::Results { channels: vec![] },
    }
}
fn finished() -> BatchSnapshot {
    BatchSnapshot::Finished(Arc::new(Ok(response())))
}
fn transcript() -> TranscriptWithData {
    TranscriptWithData {
        id: "transcript".into(),
        user_id: "authenticated-member".into(),
        session_id: "note".into(),
        created_at: "2026-11-08T07:00:00Z".into(),
        started_at: 1000.0,
        ended_at: None,
        memo_md: String::new(),
        words: vec![],
        speaker_hints: vec![],
    }
}
fn owner() -> (SourceOwner, Arc<AtomicBool>) {
    let dropped = Arc::new(AtomicBool::new(false));
    (
        Arc::new(Mutex::new(Box::new(Dropped(dropped.clone())))),
        dropped,
    )
}
fn driver(status: watch::Receiver<BatchSnapshot>) -> FakeDriver {
    FakeDriver {
        status,
        cancellations: Arc::new(AtomicUsize::new(0)),
        panic_once: AtomicBool::new(false),
        final_on_close: None,
        final_snapshot: None,
    }
}
async fn until(mut check: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !check() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cancel_retains_real_owners_until_native_finished_before_completing() {
    let (owner, source_dropped) = owner();
    let native_source = owner.clone();
    let model_dropped = Arc::new(AtomicBool::new(false));
    let admission_dropped = Arc::new(AtomicBool::new(false));
    let model = Dropped(model_dropped.clone());
    let admission = Dropped(admission_dropped.clone());
    let (status, receiver) = watch::channel(BatchSnapshot::Running);
    let driver = driver(receiver);
    let cancellations = driver.cancellations.clone();
    let (release, wait) = oneshot::channel();
    let native = tokio::spawn(async move {
        wait.await.unwrap();
        drop(native_source);
        drop(model);
        drop(admission);
        status.send_replace(finished());
    });
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    let operation = tokio::spawn(complete(
        driver,
        owner,
        Arc::new(AtomicBool::new(false)),
        child,
        |_| Ok(transcript()),
    ));
    cancel.cancel();
    until(|| cancellations.load(Ordering::Acquire) > 0).await;
    assert!(!source_dropped.load(Ordering::Acquire));
    assert!(!model_dropped.load(Ordering::Acquire));
    assert!(!admission_dropped.load(Ordering::Acquire));
    assert!(!operation.is_finished());
    release.send(()).unwrap();
    native.await.unwrap();
    assert_eq!(
        operation.await.unwrap().unwrap_err(),
        TranscriptionFailure::Interrupted
    );
    assert!(source_dropped.load(Ordering::Acquire));
    assert!(model_dropped.load(Ordering::Acquire));
    assert!(admission_dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn dropped_waiter_only_cancels_while_owned_task_retains_source_until_finish() {
    let (owner, source_dropped) = owner();
    let native_source = owner.clone();
    let (status, receiver) = watch::channel(BatchSnapshot::Running);
    let driver = driver(receiver);
    let cancellations = driver.cancellations.clone();
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let (done, completed) = oneshot::channel();
    let owned = tokio::spawn(async move {
        let result = complete(
            driver,
            owner,
            Arc::new(AtomicBool::new(false)),
            task_cancel,
            |_| Ok(transcript()),
        )
        .await;
        let _ = done.send(result);
    });
    let (entered, waiting) = oneshot::channel();
    let waiter = tokio::spawn(async move {
        let _cancel = CancelOnDrop(cancel);
        let _ = entered.send(());
        owned.await
    });
    waiting.await.unwrap();
    waiter.abort();
    let _ = waiter.await;
    until(|| cancellations.load(Ordering::Acquire) > 0).await;
    assert!(!source_dropped.load(Ordering::Acquire));
    drop(native_source);
    status.send_replace(finished());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), completed)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err(),
        TranscriptionFailure::Interrupted
    );
    assert!(source_dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn driver_panic_requests_cancel_but_cannot_release_source_before_actual_finish() {
    let (owner, source_dropped) = owner();
    let native_source = owner.clone();
    let (status, receiver) = watch::channel(BatchSnapshot::Running);
    let driver = driver(receiver);
    driver.panic_once.store(true, Ordering::Release);
    let cancellations = driver.cancellations.clone();
    let operation = tokio::spawn(complete(
        driver,
        owner,
        Arc::new(AtomicBool::new(false)),
        CancellationToken::new(),
        |_| Ok(transcript()),
    ));
    until(|| cancellations.load(Ordering::Acquire) > 0).await;
    assert!(!operation.is_finished());
    assert!(!source_dropped.load(Ordering::Acquire));
    drop(native_source);
    status.send_replace(finished());
    assert_eq!(
        operation.await.unwrap().unwrap_err(),
        TranscriptionFailure::EngineFailed
    );
    assert!(source_dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn mapping_panic_is_reported_after_native_drain_and_releases_source_before_done() {
    let (owner, source_dropped) = owner();
    let (status, receiver) = watch::channel(finished());
    drop(status);
    let result = complete(
        driver(receiver),
        owner,
        Arc::new(AtomicBool::new(false)),
        CancellationToken::new(),
        |_| panic!("mapping fixture panic"),
    )
    .await;
    assert_eq!(result.unwrap_err(), TranscriptionFailure::EngineFailed);
    assert!(source_dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn closed_notifier_still_observes_its_last_persistent_finished_receipt() {
    let (owner, source_dropped) = owner();
    let (status, receiver) = watch::channel(BatchSnapshot::Running);
    drop(status);
    let mut driver = driver(receiver);
    driver.final_on_close = Some(finished());
    let result = complete(
        driver,
        owner,
        Arc::new(AtomicBool::new(false)),
        CancellationToken::new(),
        |_| Ok(transcript()),
    )
    .await
    .unwrap();
    assert_eq!(result.user_id, "authenticated-member");
    assert!(source_dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn missing_completion_receipt_retains_owner_and_disables_further_host_admission() {
    let (owner, source_dropped) = owner();
    let (status, receiver) = watch::channel(BatchSnapshot::Running);
    drop(status);
    let temporary = tempfile::tempdir().unwrap();
    let host = CpuTranscriptionHost::start(
        "cpu-base".into(),
        ModelSpec {
            absolute_path: temporary.path().join("missing-model.bin"),
            sha256: "a".repeat(64),
            size_bytes: 1,
        },
        Limits::default(),
        SpeechAdmission::new(1, 0, 16).unwrap(),
    )
    .unwrap();
    let observed = host.failed.clone();
    let operation = tokio::spawn(complete(
        driver(receiver),
        owner,
        host.failed.clone(),
        CancellationToken::new(),
        |_| Ok(transcript()),
    ));
    until(|| observed.load(Ordering::Acquire)).await;
    assert!(!operation.is_finished());
    assert!(!source_dropped.load(Ordering::Acquire));
    assert!(matches!(
        host.prepare(&request(), CancellationToken::new()).await,
        Err(TranscriptionFailure::EngineUnavailable)
    ));
    // No native work exists in this fake; abort only cleans up this irrecoverable fixture.
    operation.abort();
    let _ = operation.await;
    assert!(source_dropped.load(Ordering::Acquire));
    let mut stopped = host.shutdown();
    tokio::time::timeout(Duration::from_secs(5), async {
        while stopped.snapshot().phase
            != hypr_transcribe_whisper_local::resources::ResourcePhase::Stopped
        {
            assert!(stopped.changed().await.is_some());
        }
    })
    .await
    .unwrap();
}

fn request() -> TranscriptionJobRequest {
    serde_json::from_value(serde_json::json!({"session_id":"note","expected_recording_sha256":"a".repeat(64),"expected_transcript":"absent","model":"cpu-base","languages":["en-GB","nl"],"keywords":["Loofah","café project"]})).unwrap()
}
#[test]
fn operator_model_and_supported_languages_are_checked_without_resolving_client_paths() {
    let mut request = request();
    let params = parameters("cpu-base", &request).unwrap();
    assert_eq!(params.keywords, request.keywords);
    assert_eq!(params.languages[0].bcp47_code(), "en-GB");
    assert!(parameters("other-model", &request).is_err());
    request.languages = vec!["la".into()];
    assert!(parameters("cpu-base", &request).is_ok());
    request.languages = vec!["zu".into()];
    assert!(parameters("cpu-base", &request).is_err());
    request.languages = vec!["not-a-language".into()];
    assert!(parameters("cpu-base", &request).is_err());
    request.languages.clear();
    assert!(
        parameters("cpu-base", &request)
            .unwrap()
            .languages
            .is_empty()
    );
}
#[test]
fn failure_projection_never_exposes_operator_paths_or_raw_native_details() {
    assert_eq!(
        resource_failure(ResourceError::Busy),
        TranscriptionFailure::ResourceLimit
    );
    assert_eq!(
        resource_failure(ResourceError::Loading),
        TranscriptionFailure::EngineUnavailable
    );
    assert_eq!(
        resource_failure(ResourceError::Cancelled),
        TranscriptionFailure::Interrupted
    );
    assert_eq!(
        resource_failure(ResourceError::Failed("private operator source path".into())),
        TranscriptionFailure::EngineFailed
    );
}

#[tokio::test]
async fn cancellation_during_blocking_mapping_waits_for_actual_mapping_before_owner_release() {
    let (owner, source_dropped) = owner();
    let (status, receiver) = watch::channel(finished());
    drop(status);
    let cancel = CancellationToken::new();
    let request_cancel = cancel.clone();
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let mapping_gate = gate.clone();
    let (entered, mapping) = oneshot::channel();
    let (done, completed) = oneshot::channel();
    let owned = tokio::spawn(async move {
        let result = complete(
            driver(receiver),
            owner,
            Arc::new(AtomicBool::new(false)),
            request_cancel,
            move |_| {
                let _ = entered.send(());
                let (lock, wake) = &*mapping_gate;
                let mut released = lock.lock().unwrap();
                while !*released {
                    let (next, timeout) = wake
                        .wait_timeout(released, Duration::from_secs(10))
                        .unwrap();
                    released = next;
                    assert!(!timeout.timed_out());
                }
                Ok(transcript())
            },
        )
        .await;
        let _ = done.send(result);
    });
    tokio::time::timeout(Duration::from_secs(5), mapping)
        .await
        .unwrap()
        .unwrap();
    cancel.cancel();
    assert!(!source_dropped.load(Ordering::Acquire));
    assert!(!owned.is_finished());
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), completed)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err(),
        TranscriptionFailure::Interrupted
    );
    owned.await.unwrap();
    assert!(source_dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn scratch_destruction_precedes_core_release_on_success_cancel_and_both_panic_paths() {
    struct CoreReceipt {
        scratch: Arc<AtomicBool>,
        ordered: Arc<AtomicBool>,
        released: Arc<AtomicBool>,
    }
    impl Drop for CoreReceipt {
        fn drop(&mut self) {
            self.ordered
                .store(self.scratch.load(Ordering::Acquire), Ordering::Release);
            self.released.store(true, Ordering::Release);
        }
    }
    for outcome in 0..4 {
        let scratch_dropped = Arc::new(AtomicBool::new(false));
        let ordered = Arc::new(AtomicBool::new(false));
        let released = Arc::new(AtomicBool::new(false));
        let scratch = Dropped(scratch_dropped.clone());
        let core: Box<dyn Send> = Box::new(CoreReceipt {
            scratch: scratch_dropped.clone(),
            ordered: ordered.clone(),
            released: released.clone(),
        });
        let source = Arc::new(Mutex::new(core));
        let (status, receiver) = watch::channel(finished());
        drop(status);
        let driver = driver(receiver);
        driver.panic_once.store(outcome == 2, Ordering::Release);
        let cancel = CancellationToken::new();
        if outcome == 1 {
            cancel.cancel();
        }
        let result = complete(
            driver,
            source,
            Arc::new(AtomicBool::new(false)),
            cancel,
            move |_| {
                let _scratch = &scratch;
                assert!(outcome != 3, "mapping scratch fixture panic");
                Ok(transcript())
            },
        )
        .await;
        assert_eq!(result.is_ok(), outcome == 0);
        assert!(released.load(Ordering::Acquire));
        assert!(
            ordered.load(Ordering::Acquire),
            "Core receipt preceded scratch destruction in case {outcome}"
        );
    }
}

#[tokio::test]
async fn queued_batch_waits_past_native_source_drop_until_original_admission_retires() {
    use std::task::Poll;
    let admission = SpeechAdmission::new(1, 0, 2).unwrap();
    let capacity = Arc::new(admission.batch(&CancellationToken::new()).await.unwrap());
    let native_source_capacity = capacity.clone();
    let native_slots = Arc::new(tokio::sync::Semaphore::new(1));
    let native_admission = native_slots.clone().try_acquire_owned().unwrap();
    let (source_released, source_release) = oneshot::channel();
    let (retire, retirement) = oneshot::channel();
    let (status, receiver) = watch::channel(BatchSnapshot::Running);
    let (owner, _) = owner();
    let operation = tokio::spawn(complete(
        AdmittedBatch {
            driver: driver(receiver),
            _capacity: capacity,
        },
        owner,
        Arc::new(AtomicBool::new(false)),
        CancellationToken::new(),
        |_| Ok(transcript()),
    ));
    let native = tokio::spawn(async move {
        drop(native_source_capacity);
        source_released.send(()).unwrap();
        retirement.await.unwrap();
        drop(native_admission);
        status.send_replace(finished());
    });
    source_release.await.unwrap();
    let cancel = CancellationToken::new();
    let mut next = Box::pin(admission.batch(&cancel));
    assert!(std::future::poll_fn(|cx| Poll::Ready(next.as_mut().poll(cx).is_pending())).await);
    assert!(native_slots.clone().try_acquire_owned().is_err());
    retire.send(()).unwrap();
    native.await.unwrap();
    operation.await.unwrap().unwrap();
    let next_capacity = next.await.unwrap();
    let next_native = native_slots
        .try_acquire_owned()
        .expect("scheduler handoff must follow physical native admission release");
    drop(next_native);
    drop(next_capacity);
}

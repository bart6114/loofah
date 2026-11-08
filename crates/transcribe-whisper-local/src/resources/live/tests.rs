use super::*;
use std::sync::Condvar;

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
            let (next, timeout) = self
                .wake
                .wait_timeout(released, Duration::from_secs(15))
                .unwrap();
            released = next;
            assert!(!timeout.timed_out(), "fake native gate was not released");
        }
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
    async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !self.entered.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[derive(Clone)]
struct Plan {
    init: Option<Arc<Gate>>,
    exit: Option<Arc<Gate>>,
    consume: Option<Arc<Gate>>,
    start_error: bool,
    init_error: bool,
    panic: bool,
    metadata_panic: bool,
    premature_finish: bool,
    burst: usize,
    observed: Arc<Mutex<Vec<(usize, usize, usize, Vec<u32>)>>>,
    params: Arc<Mutex<Vec<ListenParams>>>,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            init: None,
            exit: None,
            consume: None,
            start_error: false,
            init_error: false,
            panic: false,
            metadata_panic: false,
            premature_finish: false,
            burst: 0,
            observed: Default::default(),
            params: Default::default(),
        }
    }
}

struct FakeEngine(Plan);

impl Engine for FakeEngine {
    fn metadata(&self) -> owhisper_interface::stream::Metadata {
        assert!(!self.0.metadata_panic, "fake supervisor callback panic");
        Default::default()
    }
    fn batch(
        &self,
        _: &TrustedBatchSource,
        _: &ListenParams,
        _: Arc<AtomicBool>,
        _: &mut dyn FnMut(BatchSseMessage) -> Result<(), crate::Error>,
    ) -> Result<batch::Response, crate::Error> {
        Err(crate::Error::protocol("fake live engine has no batch path"))
    }
    fn live(
        &self,
        params: ListenParams,
        cancelled: Arc<AtomicBool>,
    ) -> Result<OwnedWorker, ResourceError> {
        if self.0.start_error {
            return Err(ResourceError::Unavailable);
        }
        self.0.params.lock().unwrap().push(params);
        let plan = self.0.clone();
        let (jobs, mut input) = mpsc::channel(32);
        let (out, output) = mpsc::channel(16);
        let (ready, initialized) = tokio::sync::oneshot::channel();
        let (done, completion) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            if let Some(gate) = &plan.init {
                gate.wait();
            }
            if plan.init_error {
                let _ = ready.send(Err("fake initialization rejected".into()));
            } else {
                let _ = ready.send(Ok(()));
                if let Some(gate) = &plan.consume {
                    gate.wait();
                }
                assert!(!plan.panic, "fake live thread panic");
                if plan.premature_finish {
                    let _ = out.blocking_send(Output::Finished {
                        duration: 0.0,
                        finalized: true,
                    });
                } else {
                    while let Some(job) = input.blocking_recv() {
                        if cancelled.load(Ordering::Acquire) {
                            break;
                        }
                        match job {
                            Job::Audio(work) => {
                                plan.observed.lock().unwrap().push((
                                    work.channel,
                                    work.chunk.sample_start,
                                    work.chunk.sample_end,
                                    work.chunk
                                        .samples
                                        .iter()
                                        .map(|sample| sample.to_bits())
                                        .collect(),
                                ));
                                if out
                                    .blocking_send(Output::Segment(
                                        work.channel,
                                        segment(work.chunk.sample_start, work.chunk.sample_end),
                                        false,
                                    ))
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Job::Finish {
                                duration,
                                finalized,
                            } => {
                                for _ in 0..plan.burst {
                                    if out
                                        .blocking_send(Output::Segment(0, segment(0, 1600), true))
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                                let _ = out.blocking_send(Output::Finished {
                                    duration,
                                    finalized,
                                });
                                break;
                            }
                            Job::Deadline | Job::Evidence(_) => {}
                        }
                    }
                }
            }
            let _ = done.send(());
            if let Some(gate) = &plan.exit {
                gate.wait();
            }
        });
        Ok(OwnedWorker {
            jobs,
            output,
            initialized,
            completion,
            thread,
        })
    }
}

fn segment(start: usize, end: usize) -> crate::service::Segment {
    crate::service::Segment {
        text: "live words".into(),
        start: start as f64 / 16000.0,
        duration: (end - start) as f64 / 16000.0,
        confidence: 0.9,
        language: Some("en".into()),
    }
}

fn spec(channels: LiveChannels, generation: u8) -> LiveSpec {
    LiveSpec {
        generation: [generation; 16],
        start_frame: 0,
        channels,
        params: ListenParams {
            channels: channels.count() as u8,
            languages: vec!["en".parse().unwrap()],
            ..Default::default()
        },
    }
}

async fn host(plan: Plan, groups: usize) -> WhisperResources {
    let host = WhisperResources::start(
        Limits {
            workload_groups: groups,
            ..Limits::default()
        },
        move || Ok(Arc::new(FakeEngine(plan))),
    )
    .unwrap();
    let mut status = host.observer();
    tokio::time::timeout(Duration::from_secs(10), async {
        while status.snapshot().phase != ResourcePhase::Ready {
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    host
}

async fn open(live: &OwnedLive) {
    let mut observer = live.observer();
    tokio::time::timeout(Duration::from_secs(10), async {
        while observer.snapshot().status != LiveStatus::Open {
            assert!(
                !matches!(observer.snapshot().status, LiveStatus::Interrupted(_)),
                "{:?}",
                observer.snapshot()
            );
            observer.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

async fn receipt(observer: &mut LiveCompletionObserver) -> Arc<LiveDrainReceipt> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let LiveCompletion::Finished(value) = observer.snapshot() {
                return value;
            }
            observer
                .changed()
                .await
                .expect("completion owner disappeared");
        }
    })
    .await
    .unwrap()
}

async fn stop(host: &WhisperResources) {
    let mut status = host.shutdown();
    tokio::time::timeout(Duration::from_secs(10), async {
        while status.snapshot().phase != ResourcePhase::Stopped {
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

async fn collect(live: &mut OwnedLive) -> (Vec<LiveCaption>, Arc<LiveDrainReceipt>) {
    collect_with_deadline(live, Duration::from_secs(10)).await
}

async fn collect_with_deadline(
    live: &mut OwnedLive,
    deadline: Duration,
) -> (Vec<LiveCaption>, Arc<LiveDrainReceipt>) {
    let mut captions = Vec::new();
    let drained = tokio::time::timeout(deadline, async {
        while let Some(caption) = live.results.recv().await {
            captions.push(caption);
        }
    })
    .await;
    assert!(
        drained.is_ok(),
        "live drain exceeded {deadline:?}; received={} snapshot={:?} completion={:?}",
        captions.len(),
        live.observer().snapshot(),
        live.completion().snapshot(),
    );
    let receipt = receipt(&mut live.completion()).await;
    (captions, receipt)
}

fn pcm(samples: &[f32]) -> Arc<[f32]> {
    Arc::from(samples)
}

fn ingress_fixture(channels: LiveChannels) -> (OwnedLive, mpsc::Receiver<Frame>) {
    let spec = spec(channels, 1);
    let (status, _) = watch::channel(Arc::new(LiveSnapshot {
        generation: spec.generation,
        start_frame: 0,
        admitted_end_frame: 0,
        input_consumed_end_frame: 0,
        completed_through_frame: None,
        status: LiveStatus::Open,
        first_sequence: 0,
        next_sequence: 0,
        captions: Vec::new(),
    }));
    let mut state = Ingress::new(0);
    state.phase = Phase::Open;
    let shared = Arc::new(Shared {
        budgets: ingress::budgets(channels.count()),
        spec,
        state: Mutex::new(state),
        status,
        native_cancelled: Arc::new(AtomicBool::new(false)),
        native_proof: Arc::new(AtomicU8::new(0)),
        cancelled: CancellationToken::new(),
        control: Notify::new(),
    });
    let (frames, receive) = mpsc::channel(ingress::INPUT_BLOCKS);
    let (_, result) = mpsc::channel(16);
    let (_, completed) = watch::channel(LiveCompletion::Pending);
    (
        OwnedLive {
            results: result,
            frames,
            shared,
            completion: LiveCompletionObserver(completed),
        },
        receive,
    )
}

#[test]
fn atomic_stereo_credits_and_queue_busy_never_advance_cursor_or_consume_input() {
    let (live, mut receive) = ingress_fixture(LiveChannels::MicSystem);
    let samples = pcm(&[0.12345679, -0.2345679]);
    let held = live.shared.budgets[1]
        .clone()
        .try_acquire_many_owned(crate::service::live_pipeline::AUDIO_BUDGET as u32)
        .unwrap();
    assert_eq!(
        live.try_accept_frame(0, &samples),
        Err(LiveIngressError::Busy)
    );
    assert_eq!(live.observer().snapshot().admitted_end_frame, 0);
    assert!(receive.try_recv().is_err());
    assert_eq!(
        live.shared.budgets[0].available_permits(),
        crate::service::live_pipeline::AUDIO_BUDGET
    );
    drop(held);
    for first in 0..ingress::INPUT_BLOCKS as u64 {
        live.try_accept_frame(first, &samples).unwrap();
    }
    assert_eq!(
        live.try_accept_frame(8, &samples),
        Err(LiveIngressError::Busy)
    );
    assert_eq!(live.observer().snapshot().admitted_end_frame, 8);
    let frame = receive.try_recv().unwrap();
    assert_eq!(
        frame
            .permits
            .iter()
            .map(|p| p.num_permits())
            .collect::<Vec<_>>(),
        [1, 1]
    );
    assert_eq!(
        frame
            .samples
            .iter()
            .map(|s| s.to_bits())
            .collect::<Vec<_>>(),
        samples.iter().map(|s| s.to_bits()).collect::<Vec<_>>()
    );
    drop(frame);
    live.try_accept_frame(8, &samples).unwrap();
}

#[test]
fn exact_replay_is_bitwise_bounded_and_gaps_overlaps_or_changes_are_explicit() {
    let (live, mut receive) = ingress_fixture(LiveChannels::Microphone);
    let original = pcm(&[0.12345679, -0.0]);
    live.try_accept_frame(0, &original).unwrap();
    drop(receive.try_recv().unwrap());
    assert_eq!(
        live.try_accept_frame(0, &original),
        Ok(FrameReceipt::Duplicate {
            admitted_end_frame: 2
        })
    );
    assert_eq!(
        live.try_accept_frame(0, &pcm(&[0.1234436, -0.0])),
        Err(LiveIngressError::ChangedReplay)
    );
    assert_eq!(
        live.try_accept_frame(0, &pcm(&[0.12345679, 0.0])),
        Err(LiveIngressError::ChangedReplay)
    );
    assert_eq!(
        live.try_accept_frame(1, &original),
        Err(LiveIngressError::Overlap)
    );
    assert_eq!(
        live.try_accept_frame(3, &original),
        Err(LiveIngressError::Gap { expected_frame: 2 })
    );
    for first in 2..70 {
        live.try_accept_frame(first, &pcm(&[0.0])).unwrap();
        drop(receive.try_recv().unwrap());
    }
    assert_eq!(
        live.try_accept_frame(0, &original),
        Err(LiveIngressError::UnknownReplay)
    );
    assert_eq!(live.observer().snapshot().admitted_end_frame, 70);
}

#[test]
fn invalid_pcm_and_offsets_preserve_ingress_state() {
    let (live, _) = ingress_fixture(LiveChannels::MicSystem);
    for samples in [
        pcm(&[]),
        pcm(&[0.0]),
        pcm(&[f32::NAN, 0.0]),
        pcm(&[0.0, f32::INFINITY]),
        Arc::from(vec![
            0.0;
            2 * (crate::service::live_pipeline::MAX_MESSAGE_SAMPLES
                + 1)
        ]),
    ] {
        assert_eq!(
            live.try_accept_frame(0, &samples),
            Err(LiveIngressError::Invalid)
        );
    }
    assert_eq!(
        live.try_accept_frame(u64::MAX, &pcm(&[0.0, 0.0])),
        Err(LiveIngressError::Invalid)
    );
    assert_eq!(live.observer().snapshot().admitted_end_frame, 0);
}

#[tokio::test]
async fn finish_drains_partial_tail_without_inventing_a_processed_prefix() {
    let host = host(Plan::default(), 1).await;
    let mut live = host
        .start_live(
            host.try_live(2).unwrap(),
            spec(LiveChannels::MicSystem, 1),
            (),
        )
        .unwrap();
    open(&live).await;
    live.try_accept_frame(0, &Arc::from(vec![0.0; 2 * 529]))
        .unwrap();
    assert_eq!(live.observer().snapshot().completed_through_frame, None);
    assert_eq!(
        live.finish(528),
        Err(LiveIngressError::Gap {
            expected_frame: 529
        })
    );
    live.finish(529).unwrap();
    live.finish(529).unwrap();
    assert_eq!(
        live.try_accept_frame(529, &pcm(&[0.0, 0.0])),
        Err(LiveIngressError::Closed)
    );
    let (captions, receipt) = collect(&mut live).await;
    assert!(captions.is_empty());
    assert_eq!(receipt.failure, None);
    assert_eq!(receipt.native, NativeDisposition::Joined);
    assert!(receipt.owners_released);
    assert_eq!(
        (receipt.admitted_end_frame, receipt.input_consumed_end_frame),
        (529, 529)
    );
    assert_eq!(
        live.observer().snapshot().completed_through_frame,
        Some(529)
    );
    stop(&host).await;
}

#[tokio::test]
async fn independent_instances_keep_roles_generations_and_provider_parameters_separate() {
    let plan = Plan {
        burst: 1,
        ..Default::default()
    };
    let params = plan.params.clone();
    let host = host(plan, 2).await;
    let mut a_spec = spec(LiveChannels::Microphone, 1);
    a_spec.params.keywords = vec!["only-alpha".into()];
    let mut b_spec = spec(LiveChannels::SystemAudio, 2);
    b_spec.params.keywords = vec!["only-beta".into()];
    let mut a = host
        .start_live(host.try_live(1).unwrap(), a_spec, ())
        .unwrap();
    let mut b = host
        .start_live(host.try_live(1).unwrap(), b_spec, ())
        .unwrap();
    open(&a).await;
    open(&b).await;
    a.try_accept_frame(0, &Arc::from(vec![0.0; 1600])).unwrap();
    b.try_accept_frame(0, &Arc::from(vec![0.0; 1600])).unwrap();
    a.finish(1600).unwrap();
    let (a_words, a_done) = collect(&mut a).await;
    assert_eq!(a_words[0].physical_channel, 0);
    assert_eq!(a_words[0].role, ChannelRole::Microphone);
    assert_eq!(a_done.generation, [1; 16]);
    assert_eq!(a_done.failure, None);
    assert_eq!(b.observer().snapshot().status, LiveStatus::Open);
    b.finish(1600).unwrap();
    let (b_words, b_done) = collect(&mut b).await;
    assert_eq!(b_words[0].physical_channel, 0);
    assert_eq!(b_words[0].role, ChannelRole::SystemAudio);
    assert_eq!(b_done.generation, [2; 16]);
    assert_eq!(b_done.failure, None);
    let mut keywords: Vec<_> = params
        .lock()
        .unwrap()
        .iter()
        .map(|p| p.keywords[0].clone())
        .collect();
    keywords.sort();
    assert_eq!(keywords, ["only-alpha", "only-beta"]);
    stop(&host).await;
}

#[tokio::test]
async fn unread_or_closed_authoritative_output_interrupts_instead_of_dropping_words() {
    for closed in [false, true] {
        let host = host(
            Plan {
                burst: 100,
                ..Default::default()
            },
            1,
        )
        .await;
        let mut live = host
            .start_live(
                host.try_live(1).unwrap(),
                spec(LiveChannels::MixedMono, 1),
                (),
            )
            .unwrap();
        open(&live).await;
        if closed {
            live.results.close();
        }
        live.try_accept_frame(0, &Arc::from(vec![0.0; 1600]))
            .unwrap();
        live.finish(1600).unwrap();
        let done = receipt(&mut live.completion()).await;
        assert_eq!(done.failure, Some(LiveFailure::OutputLost));
        assert_eq!(done.native, NativeDisposition::Joined);
        assert!(done.owners_released);
        assert!(done.caption_count <= RESULT_CAPACITY as u64);
        assert_eq!(live.observer().snapshot().completed_through_frame, None);
        stop(&host).await;
    }
}

#[tokio::test]
async fn slow_ui_gets_bounded_window_reset_while_core_collects_every_caption() {
    let host = host(
        Plan {
            burst: 180,
            ..Default::default()
        },
        1,
    )
    .await;
    let mut live = host
        .start_live(
            host.try_live(1).unwrap(),
            spec(LiveChannels::MixedMono, 1),
            (),
        )
        .unwrap();
    open(&live).await;
    let observer = live.observer();
    live.try_accept_frame(0, &Arc::from(vec![0.0; 1600]))
        .unwrap();
    live.finish(1600).unwrap();
    let (words, done) = collect(&mut live).await;
    assert_eq!(words.len(), 180);
    assert_eq!(done.caption_count, 180);
    assert_eq!(done.failure, None);
    let snapshot = observer.snapshot();
    assert_eq!(snapshot.captions.len(), 128);
    assert_eq!(snapshot.first_sequence, 52);
    assert!(snapshot.requires_reset([1; 16], 0));
    assert!(snapshot.requires_reset([2; 16], 100));
    assert!(!snapshot.requires_reset([1; 16], 52));
    stop(&host).await;
}

struct SourceOwner {
    released: Arc<AtomicBool>,
    drop_gate: Option<Arc<Gate>>,
}
impl Drop for SourceOwner {
    fn drop(&mut self) {
        if let Some(gate) = &self.drop_gate {
            gate.wait();
        }
        self.released.store(true, Ordering::Release);
    }
}

#[tokio::test]
async fn oneshot_completion_never_substitutes_for_thread_join_or_owner_release() {
    let exit = Arc::new(Gate::default());
    let owner_drop = Arc::new(Gate::default());
    let host = host(
        Plan {
            exit: Some(exit.clone()),
            ..Default::default()
        },
        1,
    )
    .await;
    let released = Arc::new(AtomicBool::new(false));
    let live = host
        .start_live(
            host.try_live(1).unwrap(),
            spec(LiveChannels::Microphone, 1),
            SourceOwner {
                released: released.clone(),
                drop_gate: Some(owner_drop.clone()),
            },
        )
        .unwrap();
    open(&live).await;
    live.finish(0).unwrap();
    let mut completion = live.completion();
    exit.entered().await;
    assert!(matches!(completion.snapshot(), LiveCompletion::Pending));
    assert!(!released.load(Ordering::Acquire));
    assert!(matches!(host.try_batch(), Err(ResourceError::Busy)));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), completion.changed())
            .await
            .is_err()
    );
    exit.release();
    owner_drop.entered().await;
    assert!(matches!(completion.snapshot(), LiveCompletion::Pending));
    assert!(matches!(host.try_batch(), Err(ResourceError::Busy)));
    owner_drop.release();
    let done = receipt(&mut completion).await;
    assert_eq!(done.failure, None);
    assert!(released.load(Ordering::Acquire));
    assert_eq!(done.native, NativeDisposition::Joined);
    assert!(done.owners_released);
    stop(&host).await;
}

#[tokio::test]
async fn caller_drop_during_initialization_retains_owners_and_never_reopens() {
    let init = Arc::new(Gate::default());
    let host = host(
        Plan {
            init: Some(init.clone()),
            ..Default::default()
        },
        1,
    )
    .await;
    let released = Arc::new(AtomicBool::new(false));
    let live = host
        .start_live(
            host.try_live(1).unwrap(),
            spec(LiveChannels::Microphone, 1),
            SourceOwner {
                released: released.clone(),
                drop_gate: None,
            },
        )
        .unwrap();
    init.entered().await;
    let observer = live.observer();
    let mut completion = live.completion();
    drop(live);
    assert_eq!(
        observer.snapshot().status,
        LiveStatus::Interrupted(LiveFailure::Cancelled)
    );
    assert!(!released.load(Ordering::Acquire));
    assert!(matches!(host.try_live(1), Err(ResourceError::Busy)));
    init.release();
    let done = receipt(&mut completion).await;
    assert_eq!(done.failure, Some(LiveFailure::Cancelled));
    assert!(released.load(Ordering::Acquire));
    assert_eq!(done.native, NativeDisposition::Joined);
    assert!(matches!(
        observer.snapshot().status,
        LiveStatus::Interrupted(_)
    ));
    stop(&host).await;
}

#[tokio::test]
async fn finish_during_preparation_is_bounded_and_handles_zero_audio() {
    let init = Arc::new(Gate::default());
    let host = host(
        Plan {
            init: Some(init.clone()),
            ..Default::default()
        },
        1,
    )
    .await;
    let mut live = host
        .start_live(
            host.try_live(1).unwrap(),
            spec(LiveChannels::Microphone, 1),
            (),
        )
        .unwrap();
    init.entered().await;
    assert_eq!(
        live.try_accept_frame(0, &pcm(&[0.0])),
        Err(LiveIngressError::Busy)
    );
    live.finish(0).unwrap();
    init.release();
    let (_, done) = collect(&mut live).await;
    assert_eq!(done.failure, None);
    assert_eq!(done.admitted_end_frame, 0);
    assert_eq!(done.caption_count, 0);
    stop(&host).await;
}

#[tokio::test]
async fn startup_rejection_and_native_panic_have_explicit_owned_completion() {
    for plan in [
        Plan {
            start_error: true,
            ..Default::default()
        },
        Plan {
            init_error: true,
            ..Default::default()
        },
        Plan {
            panic: true,
            ..Default::default()
        },
    ] {
        let never_started = plan.start_error;
        let host = host(plan, 1).await;
        let released = Arc::new(AtomicBool::new(false));
        let live = host
            .start_live(
                host.try_live(1).unwrap(),
                spec(LiveChannels::Microphone, 1),
                SourceOwner {
                    released: released.clone(),
                    drop_gate: None,
                },
            )
            .unwrap();
        let done = receipt(&mut live.completion()).await;
        assert!(done.failure.is_some());
        assert!(done.owners_released);
        assert!(released.load(Ordering::Acquire));
        assert_eq!(
            done.native,
            if never_started {
                NativeDisposition::NeverStarted
            } else {
                NativeDisposition::Joined
            }
        );
        assert_eq!(live.observer().snapshot().completed_through_frame, None);
        stop(&host).await;
    }
}

#[tokio::test]
async fn host_shutdown_during_initialization_waits_for_actual_native_release() {
    let init = Arc::new(Gate::default());
    let host = host(
        Plan {
            init: Some(init.clone()),
            ..Default::default()
        },
        1,
    )
    .await;
    let live = host
        .start_live(
            host.try_live(1).unwrap(),
            spec(LiveChannels::Microphone, 1),
            (),
        )
        .unwrap();
    init.entered().await;
    let status = host.shutdown();
    let mut completion = live.completion();
    assert_eq!(status.snapshot().phase, ResourcePhase::Stopping);
    assert!(matches!(completion.snapshot(), LiveCompletion::Pending));
    init.release();
    let done = receipt(&mut completion).await;
    assert_eq!(done.native, NativeDisposition::Joined);
    assert_eq!(done.failure, Some(LiveFailure::Cancelled));
    stop(&host).await;
}

#[tokio::test]
async fn cancelled_full_sample_budget_releases_only_after_native_join() {
    let consume = Arc::new(Gate::default());
    let host = host(
        Plan {
            consume: Some(consume.clone()),
            ..Default::default()
        },
        1,
    )
    .await;
    let live = host
        .start_live(
            host.try_live(2).unwrap(),
            spec(LiveChannels::MicSystem, 1),
            (),
        )
        .unwrap();
    open(&live).await;
    consume.entered().await;
    let block: Arc<[f32]> = vec![0.1; 320000].into();
    // Reserve the complete remaining budget to emulate retained inference/VAD work.
    let held = live.shared.budgets[0]
        .clone()
        .acquire_many_owned(480000)
        .await
        .unwrap();
    assert_eq!(
        live.try_accept_frame(0, &block),
        Err(LiveIngressError::Busy)
    );
    assert_eq!(live.observer().snapshot().admitted_end_frame, 0);
    live.cancel();
    let mut completion = live.completion();
    assert!(matches!(completion.snapshot(), LiveCompletion::Pending));
    assert!(matches!(host.try_batch(), Err(ResourceError::Busy)));
    drop(held);
    consume.release();
    let done = receipt(&mut completion).await;
    assert_eq!(done.failure, Some(LiveFailure::Cancelled));
    assert_eq!(done.native, NativeDisposition::Joined);
    stop(&host).await;
}

#[tokio::test]
async fn invalid_embedded_vad_configuration_joins_already_spawned_worker() {
    let exit = Arc::new(Gate::default());
    let host = host(
        Plan {
            exit: Some(exit.clone()),
            ..Default::default()
        },
        1,
    )
    .await;
    let released = Arc::new(AtomicBool::new(false));
    let mut bad = spec(LiveChannels::Microphone, 1);
    bad.params.custom_query = Some(std::collections::HashMap::from([(
        "redemption_time_ms".into(),
        "0".into(),
    )]));
    let live = host
        .start_live(
            host.try_live(1).unwrap(),
            bad,
            SourceOwner {
                released: released.clone(),
                drop_gate: None,
            },
        )
        .unwrap();
    let mut completion = live.completion();
    exit.entered().await;
    assert!(matches!(completion.snapshot(), LiveCompletion::Pending));
    assert!(!released.load(Ordering::Acquire));
    assert!(matches!(host.try_batch(), Err(ResourceError::Busy)));
    exit.release();
    let done = receipt(&mut completion).await;
    assert!(matches!(done.failure, Some(LiveFailure::Initialization(_))));
    assert_eq!(done.native, NativeDisposition::Joined);
    assert!(released.load(Ordering::Acquire));
    stop(&host).await;
}

#[tokio::test]
async fn extracted_pipeline_preserves_legacy_asymmetric_channel_lengths_and_duration() {
    let (tx, mut rx) = mpsc::channel(32);
    let (started, _) = tokio::sync::oneshot::channel();
    let observations = Arc::new(Mutex::new(EvidenceWindow {
        channels: vec![Vec::new(); 2],
        end: 0,
        cutoff: 0,
    }));
    let mut pipeline =
        Pipeline::new(tx, 2, Duration::from_millis(400), started, observations, 0).unwrap();
    let budget = Arc::new(tokio::sync::Semaphore::new(546));
    let mut permit = budget.clone().acquire_many_owned(546).await.unwrap();
    let permits = vec![permit.split(529).unwrap(), permit.split(17).unwrap()];
    pipeline
        .push(vec![vec![0.0; 529], vec![0.0; 17]], permits)
        .await
        .unwrap();
    assert_eq!(pipeline.consumed_frames(), 529);
    pipeline.finish(true).await.unwrap();
    assert!(
        matches!(rx.recv().await, Some(Job::Finish {duration,finalized:true}) if duration==529.0/16000.0)
    );
    assert_eq!(budget.available_permits(), 546);
}

#[tokio::test]
async fn cancellation_after_startup_deadline_still_wakes_idle_input() {
    let (live, frames) = ingress_fixture(LiveChannels::Microphone);
    let (jobs, mut receive) = mpsc::channel(32);
    let (started, _) = tokio::sync::oneshot::channel();
    let observations = Arc::new(Mutex::new(EvidenceWindow {
        channels: vec![Vec::new()],
        end: 0,
        cutoff: 0,
    }));
    let pipeline = Pipeline::new(
        jobs.clone(),
        1,
        Duration::from_millis(400),
        started,
        observations.clone(),
        0,
    )
    .unwrap();
    let (trigger, speech) = tokio::sync::oneshot::channel();
    let input = tokio::spawn(producer(
        live.shared.clone(),
        frames,
        pipeline,
        speech,
        observations,
        jobs,
        Duration::from_millis(1),
    ));
    trigger.send(()).unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), receive.recv())
            .await
            .unwrap(),
        Some(Job::Deadline)
    ));
    live.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), input)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn premature_finished_message_cannot_hang_open_input_or_claim_complete() {
    let host = host(
        Plan {
            premature_finish: true,
            ..Default::default()
        },
        1,
    )
    .await;
    let live = host
        .start_live(
            host.try_live(1).unwrap(),
            spec(LiveChannels::Microphone, 1),
            (),
        )
        .unwrap();
    let done = receipt(&mut live.completion()).await;
    assert_eq!(done.failure, Some(LiveFailure::InvalidDrain));
    assert_eq!(done.native, NativeDisposition::Joined);
    assert!(done.owners_released);
    stop(&host).await;
}

#[tokio::test]
async fn supervisor_callback_panic_cancels_and_joins_before_owners_release() {
    let host = host(
        Plan {
            metadata_panic: true,
            ..Default::default()
        },
        1,
    )
    .await;
    let released = Arc::new(AtomicBool::new(false));
    let live = host
        .start_live(
            host.try_live(1).unwrap(),
            spec(LiveChannels::Microphone, 1),
            SourceOwner {
                released: released.clone(),
                drop_gate: None,
            },
        )
        .unwrap();
    let done = receipt(&mut live.completion()).await;
    assert_eq!(done.failure, Some(LiveFailure::WorkerPanicked));
    assert_eq!(done.native, NativeDisposition::Joined);
    assert!(done.owners_released);
    assert!(released.load(Ordering::Acquire));
    stop(&host).await;
}

#[tokio::test]
async fn retaining_finished_handles_does_not_grow_or_remove_other_admission_registrations() {
    let host = host(Plan::default(), 2).await;
    let active = host.try_live(1).unwrap();
    let mut retired = Vec::new();
    for generation in 1..=4 {
        let live = host
            .start_live(
                host.try_live(1).unwrap(),
                spec(LiveChannels::Microphone, generation),
                (),
            )
            .unwrap();
        open(&live).await;
        live.finish(0).unwrap();
        assert_eq!(receipt(&mut live.completion()).await.failure, None);
        retired.push(live);
        assert_eq!(host.inner.lock().cancellations.len(), 1);
        assert!(!active.is_cancelled());
    }
    host.shutdown();
    assert!(active.is_cancelled());
    drop(active);
    stop(&host).await;
    assert_eq!(retired.len(), 4);
}

#[tokio::test]
async fn invalid_specs_and_foreign_admissions_never_start_native_work() {
    let h = host(Plan::default(), 2).await;
    let other = host(Plan::default(), 1).await;
    assert!(matches!(
        h.start_live(
            other.try_live(1).unwrap(),
            spec(LiveChannels::Microphone, 1),
            ()
        ),
        Err(ResourceError::Invalid(_))
    ));
    for mut bad in [
        spec(LiveChannels::Microphone, 0),
        spec(LiveChannels::MicSystem, 1),
        spec(LiveChannels::Microphone, 1),
    ] {
        if bad.generation != [0; 16] && bad.channels != LiveChannels::MicSystem {
            bad.params.sample_rate = 48000;
        }
        assert!(h.start_live(h.try_live(1).unwrap(), bad, ()).is_err());
    }
    stop(&h).await;
    stop(&other).await;
}

#[test]
fn role_mapping_preserves_native_mono_semantics_without_faking_canonical_layout() {
    assert_eq!(
        LiveChannels::Microphone.role(0),
        Some(ChannelRole::Microphone)
    );
    assert_eq!(
        LiveChannels::SystemAudio.role(0),
        Some(ChannelRole::SystemAudio)
    );
    assert_eq!(
        LiveChannels::MicSystem.role(1),
        Some(ChannelRole::SystemAudio)
    );
    assert_eq!(LiveChannels::MixedMono.role(0), Some(ChannelRole::Mixed));
    assert_eq!(LiveChannels::Microphone.role(1), None);
}

#[tokio::test]
async fn f32_pipeline_keeps_exact_bits_channel_offsets_and_absolute_window_timing() {
    let plan = Plan::default();
    let observed = plan.observed.clone();
    let host = host(plan, 1).await;
    let mut configuration = spec(LiveChannels::MicSystem, 1);
    configuration.start_frame = 900;
    let mut live = host
        .start_live(host.try_live(2).unwrap(), configuration, ())
        .unwrap();
    open(&live).await;
    let mut mono: Vec<f32> = hypr_data::english_1::AUDIO
        .chunks_exact(2)
        .take(16000 * 8)
        .map(|p| i16::from_le_bytes([p[0], p[1]]) as f32 / 32768.0)
        .collect();
    for sample in mono.iter_mut().step_by(503) {
        if sample.is_normal() {
            *sample = f32::from_bits(sample.to_bits() + 1);
        }
    }
    let stereo: Arc<[f32]> = mono
        .iter()
        .flat_map(|sample| [*sample, -*sample / 3.0])
        .collect::<Vec<_>>()
        .into();
    live.try_accept_frame(900, &stereo).unwrap();
    live.finish(900 + mono.len() as u64).unwrap();
    let (captions, done) = collect(&mut live).await;
    assert_eq!(done.failure, None);
    assert!(!captions.is_empty());
    let records = observed.lock().unwrap();
    assert!(!records.is_empty());
    for (channel, start, end, actual) in records.iter() {
        let original: Vec<_> = mono[start - 900..end - 900]
            .iter()
            .map(|sample| {
                if *channel == 0 {
                    *sample
                } else {
                    -*sample / 3.0
                }
            })
            .map(f32::to_bits)
            .collect();
        assert_eq!(actual, &original);
    }
    assert!(
        captions
            .iter()
            .any(|caption| caption.role == ChannelRole::Microphone)
    );
    assert!(
        captions
            .iter()
            .any(|caption| caption.role == ChannelRole::SystemAudio)
    );
    drop(records);
    stop(&host).await;
}

#[tokio::test]
#[ignore = "requires the existing provisioned LOOFAH_WHISPER_MODEL; typed live CPU integration"]
async fn real_cpu_two_typed_sessions_emit_before_finish_and_drain_independently() {
    use sha2::{Digest, Sha256};
    let path = std::fs::canonicalize(std::env::var("LOOFAH_WHISPER_MODEL").unwrap()).unwrap();
    let mut file = File::open(&path).unwrap();
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let size = file.read(&mut buffer).unwrap();
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    let host = WhisperResources::start_cpu(
        ModelSpec {
            size_bytes: file.metadata().unwrap().len(),
            absolute_path: path,
            sha256: digest_hex(hash.finalize().as_ref()),
        },
        Limits {
            workload_groups: 2,
            ..Limits::default()
        },
    )
    .unwrap();
    let mut status = host.observer();
    tokio::time::timeout(Duration::from_secs(180), async {
        while status.snapshot().phase != ResourcePhase::Ready {
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let mut a = host
        .start_live(
            host.try_live(1).unwrap(),
            spec(LiveChannels::Microphone, 1),
            (),
        )
        .unwrap();
    let mut b = host
        .start_live(
            host.try_live(1).unwrap(),
            spec(LiveChannels::SystemAudio, 2),
            (),
        )
        .unwrap();
    open(&a).await;
    open(&b).await;
    let samples: Vec<f32> = hypr_data::english_1::AUDIO
        .chunks_exact(2)
        .map(|p| i16::from_le_bytes([p[0], p[1]]) as f32 / 32768.0)
        .collect();
    let block: Arc<[f32]> = samples
        .iter()
        .copied()
        .cycle()
        .take(16000 * 10)
        .collect::<Vec<_>>()
        .into();
    for first in [0, 160000] {
        a.try_accept_frame(first, &block).unwrap();
        b.try_accept_frame(first, &block).unwrap();
    }
    let (a_first, b_first) = tokio::time::timeout(Duration::from_secs(180), async {
        tokio::join!(a.results.recv(), b.results.recv())
    })
    .await
    .unwrap();
    assert_eq!(a_first.unwrap().role, ChannelRole::Microphone);
    assert_eq!(b_first.unwrap().role, ChannelRole::SystemAudio);
    assert_eq!(a.observer().snapshot().status, LiveStatus::Open);
    assert_eq!(b.observer().snapshot().status, LiveStatus::Open);
    a.finish(320000).unwrap();
    b.finish(320000).unwrap();
    // CPU tail inference and native joins are real work, unlike the gated fake fixtures.
    let ((_, a_done), (_, b_done)) = tokio::join!(
        collect_with_deadline(&mut a, Duration::from_secs(180)),
        collect_with_deadline(&mut b, Duration::from_secs(180)),
    );
    assert_eq!(a_done.failure, None);
    assert_eq!(b_done.failure, None);
    assert_eq!(a_done.native, NativeDisposition::Joined);
    assert_eq!(b_done.native, NativeDisposition::Joined);
    stop(&host).await;
}

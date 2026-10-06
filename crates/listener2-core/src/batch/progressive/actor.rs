use std::sync::{Arc, Mutex};
use std::time::Duration;

use owhisper_client::StreamingBatchStream;
use owhisper_interface::batch_stream::BatchStreamEvent;
use ractor::{Actor, ActorName, ActorProcessingErr, ActorRef, SpawnErr};
use tracing::Instrument;

use super::super::accumulator::StreamBatchAccumulator;
use super::super::diarize::{ChannelSegments, SharedDiarization, stamp_stream_event};
use super::super::{BatchParams, BatchRunOutput, format_user_friendly_error, session_span};
use super::ProgressiveProvider;
use super::bootstrap::{notify_start_result, spawn_progressive_batch_task};
use crate::{BatchEvent, BatchRuntime};

const BATCH_STREAM_TIMEOUT_SECS: u64 = 30;

pub(super) async fn run_progressive_batch(
    runtime: Arc<dyn BatchRuntime>,
    params: BatchParams,
    listen_params: owhisper_interface::ListenParams,
    progressive_provider: ProgressiveProvider,
    diarization: SharedDiarization,
) -> crate::Result<BatchRunOutput> {
    let span = session_span(&params.session_id);
    let provider_label = progressive_provider.label().to_string();

    async {
        let (start_tx, start_rx) = tokio::sync::oneshot::channel::<crate::Result<()>>();
        let start_notifier = Arc::new(Mutex::new(Some(start_tx)));

        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<crate::Result<BatchRunOutput>>();
        let done_notifier = Arc::new(Mutex::new(Some(done_tx)));

        let args = BatchArgs {
            runtime: runtime.clone(),
            progressive_provider,
            provider_label: provider_label.clone(),
            file_path: params.file_path,
            base_url: params.base_url,
            api_key: params.api_key,
            listen_params,
            start_notifier,
            done_notifier: done_notifier.clone(),
            session_id: params.session_id,
            diarization,
        };

        let batch_ref = match spawn_batch_actor(args).await {
            Ok(batch_ref) => {
                tracing::info!("batch actor spawned successfully");
                batch_ref
            }
            Err(err) => {
                let raw_error = format!("{err:?}");
                let message = format_user_friendly_error(&raw_error);
                tracing::error!(
                    error = %raw_error,
                    fmtr.error.user_message = %message,
                    "batch supervisor spawn failed"
                );
                return Err(crate::BatchFailure::ProgressiveActorSpawnFailed {
                    provider: provider_label.clone(),
                    message,
                }
                .into());
            }
        };

        struct StopGuard(Option<ActorRef<BatchMsg>>);

        impl Drop for StopGuard {
            fn drop(&mut self) {
                if let Some(actor) = self.0.take() {
                    actor.stop(Some("listener2-core: run_batch dropped".to_string()));
                }
            }
        }

        let mut stop_guard = StopGuard(Some(batch_ref));

        match start_rx.await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::error!("batch actor reported start failure: {err}");
                return Err(err);
            }
            Err(_) => {
                tracing::error!("batch actor start notifier dropped before reporting result");
                return Err(crate::BatchFailure::ProgressiveStartCancelled.into());
            }
        }

        match done_rx.await {
            Ok(Ok(output)) => {
                stop_guard.0 = None;
                Ok(output)
            }
            Ok(Err(err)) => Err(err),
            Err(_) => Err(crate::BatchFailure::ProgressiveFinishedWithoutStatus.into()),
        }
    }
    .instrument(span)
    .await
}

fn is_completion_event(event: &BatchStreamEvent) -> bool {
    matches!(
        event,
        BatchStreamEvent::Result { .. } | BatchStreamEvent::Terminal { .. }
    )
}

fn provider_error_from_event(event: &BatchStreamEvent) -> Option<(&str, &str, Option<i32>)> {
    let BatchStreamEvent::Error {
        provider,
        error_message,
        error_code,
    } = event
    else {
        return None;
    };

    Some((provider.as_str(), error_message.as_str(), *error_code))
}

#[allow(clippy::enum_variant_names)]
pub(super) enum BatchMsg {
    StreamResponse { event: Box<BatchStreamEvent> },
    StreamError(crate::BatchFailure),
    StreamEnded,
    DiarizationReady(Arc<ChannelSegments>),
    StreamStartFailed(crate::BatchFailure),
}

pub(super) type BatchStartNotifier =
    Arc<Mutex<Option<tokio::sync::oneshot::Sender<crate::Result<()>>>>>;
type BatchDoneNotifier =
    Arc<Mutex<Option<tokio::sync::oneshot::Sender<crate::Result<BatchRunOutput>>>>>;

#[derive(Clone)]
pub(super) struct BatchArgs {
    pub(super) runtime: Arc<dyn BatchRuntime>,
    pub(super) progressive_provider: ProgressiveProvider,
    pub(super) provider_label: String,
    pub(super) file_path: String,
    pub(super) base_url: String,
    pub(super) api_key: String,
    pub(super) listen_params: owhisper_interface::ListenParams,
    pub(super) start_notifier: BatchStartNotifier,
    pub(super) done_notifier: BatchDoneNotifier,
    pub(super) session_id: String,
    pub(super) diarization: SharedDiarization,
}

struct BatchState {
    runtime: Arc<dyn BatchRuntime>,
    session_id: String,
    rx_task: tokio::task::JoinHandle<()>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    done_notifier: BatchDoneNotifier,
    final_result: Option<crate::Result<BatchRunOutput>>,
    accumulator: StreamBatchAccumulator,
    diarization_task: tokio::task::JoinHandle<()>,
    segments: Option<Arc<ChannelSegments>>,
    pending: std::collections::VecDeque<BatchStreamEvent>,
    stream_ended: bool,
    percentage: f64,
}

impl BatchState {
    fn emit_streamed(&mut self, mut event: BatchStreamEvent) {
        if let BatchStreamEvent::Progress { percentage, .. }
        | BatchStreamEvent::Segment { percentage, .. } = &mut event
        {
            *percentage = percentage.max(self.percentage).min(0.99);
            self.percentage = *percentage;
        }
        self.runtime.emit(BatchEvent::BatchResponseStreamed {
            session_id: self.session_id.clone(),
            event,
        });
    }
}

struct BatchActor;

impl BatchActor {
    fn name() -> ActorName {
        "batch_actor".into()
    }
}

async fn spawn_batch_actor(args: BatchArgs) -> Result<ActorRef<BatchMsg>, SpawnErr> {
    let (batch_ref, _) = Actor::spawn(Some(BatchActor::name()), BatchActor, args).await?;
    Ok(batch_ref)
}

#[ractor::async_trait]
impl Actor for BatchActor {
    type Msg = BatchMsg;
    type State = BatchState;
    type Arguments = BatchArgs;

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let (rx_task, shutdown_tx) =
            spawn_progressive_batch_task(args.clone(), myself.clone()).await?;

        let diarization = args.diarization.clone();
        let diarization_task = tokio::spawn(async move {
            let segments = diarization.segments().await;
            let _ = myself.send_message(BatchMsg::DiarizationReady(segments));
        });

        Ok(BatchState {
            runtime: args.runtime,
            session_id: args.session_id,
            rx_task,
            shutdown_tx: Some(shutdown_tx),
            done_notifier: args.done_notifier,
            final_result: None,
            accumulator: StreamBatchAccumulator::new(),
            diarization_task,
            segments: None,
            pending: Default::default(),
            stream_ended: false,
            percentage: 0.0,
        })
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self::Msg>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        state.diarization_task.abort();
        state.pending.clear();
        if let Some(shutdown_tx) = state.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
            let _ = (&mut state.rx_task).await;
        }

        let final_result = state.final_result.take().unwrap_or_else(|| {
            Err(crate::BatchFailure::ProgressiveStoppedWithoutCompletionSignal.into())
        });
        notify_done_result(&state.done_notifier, final_result);

        Ok(())
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        if state.final_result.is_some() {
            return Ok(());
        }
        match message {
            BatchMsg::StreamResponse { mut event } => {
                tracing::info!("batch stream response received");
                if matches!(*event, BatchStreamEvent::Progress { .. }) {
                    state.emit_streamed(*event);
                } else if let Some(segments) = &state.segments {
                    stamp_stream_event(&mut event, segments);
                    state.accumulator.observe(&event);
                    state.emit_streamed(*event);
                } else {
                    state.pending.push_back(*event);
                }
            }
            BatchMsg::StreamStartFailed(error) => {
                tracing::error!("batch_stream_start_failed: {}", error);
                state.final_result = Some(Err(error.clone().into()));
                myself.stop(Some(format!("batch_stream_start_failed: {}", error)));
            }
            BatchMsg::StreamError(error) => {
                tracing::error!("batch_stream_error: {}", error);
                state.final_result = Some(Err(error.clone().into()));
                myself.stop(None);
            }
            BatchMsg::StreamEnded => {
                tracing::info!("batch_stream_ended");
                state.stream_ended = true;
            }
            BatchMsg::DiarizationReady(segments) => {
                state.segments = Some(segments.clone());
                while let Some(mut event) = state.pending.pop_front() {
                    stamp_stream_event(&mut event, &segments);
                    state.accumulator.observe(&event);
                    state.emit_streamed(event);
                }
            }
        }

        if state.stream_ended && state.segments.is_some() && state.final_result.is_none() {
            let output = std::mem::take(&mut state.accumulator).finish(&state.session_id);
            state.final_result = Some(Ok(output));
            myself.stop(None);
        }
        Ok(())
    }
}

fn notify_done_result(notifier: &BatchDoneNotifier, result: crate::Result<BatchRunOutput>) {
    if let Ok(mut guard) = notifier.lock()
        && let Some(sender) = guard.take()
    {
        let _ = sender.send(result);
    }
}

pub(super) fn report_stream_start_failure(
    myself: &ActorRef<BatchMsg>,
    notifier: &BatchStartNotifier,
    provider: &str,
    error: &impl std::fmt::Debug,
    context: &str,
) {
    let raw_error = format!("{error:?}");
    let message = format_user_friendly_error(&raw_error);
    let failure = crate::BatchFailure::ProgressiveStartFailed {
        provider: provider.to_string(),
        message: message.clone(),
    };

    tracing::error!(
        error = %raw_error,
        fmtr.error.user_message = %message,
        "{context}"
    );
    notify_start_result(notifier, Err(failure.clone().into()));
    let _ = myself.send_message(BatchMsg::StreamStartFailed(failure));
}

pub(super) async fn process_provider_stream(
    stream: StreamingBatchStream,
    myself: ActorRef<BatchMsg>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    provider: ProgressiveProvider,
    context: &str,
) {
    let response_timeout = match provider {
        ProgressiveProvider::WhisperCpp => None,
        ProgressiveProvider::OpenAI => Some(Duration::from_secs(BATCH_STREAM_TIMEOUT_SECS)),
    };
    let provider = provider.label();
    let mut stream = stream;
    let mut response_count = 0;
    let mut completed = false;

    loop {
        tracing::debug!(
            "{context}: waiting for next item (received {} so far)",
            response_count
        );

        tokio::select! {
            _ = &mut shutdown_rx => {
                tracing::info!("{context}: shutdown");
                return;
            }
            result = async {
                match response_timeout {
                    Some(timeout) => tokio::time::timeout(timeout, futures_util::StreamExt::next(&mut stream)).await,
                    None => Ok(futures_util::StreamExt::next(&mut stream).await),
                }
            } => {
                tracing::debug!("{context}: received result");
                match result {
                    Ok(Some(Ok(event))) => {
                        response_count += 1;

                        let is_completion = is_completion_event(&event);

                        tracing::info!(
                            "{context}: response #{}{}",
                            response_count,
                            if matches!(&event, BatchStreamEvent::Result { .. }) {
                                " (result)"
                            } else {
                                ""
                            }
                        );

                        if let Some((provider, error_message, error_code)) =
                            provider_error_from_event(&event)
                        {
                            tracing::error!(
                                fmtr.stt.provider.name = %provider,
                                error.code = ?error_code,
                                error = %error_message,
                                fmtr.response.count = response_count,
                                "{context} received provider error response"
                            );
                            let message = format_user_friendly_error(error_message);
                            send_actor_message(
                                &myself,
                                BatchMsg::StreamError(crate::BatchFailure::ProgressiveStreamError {
                                    provider: provider.to_string(),
                                    message,
                                }),
                                context,
                                "stream error",
                            );
                            break;
                        }

                        send_actor_message(
                            &myself,
                            BatchMsg::StreamResponse {
                                event: Box::new(event),
                            },
                            context,
                            "stream response",
                        );

                        if is_completion {
                            completed = true;
                            break;
                        }
                    }
                    Ok(Some(Err(err))) => {
                        let raw_error = format!("{err:?}");
                        let message = format_user_friendly_error(&raw_error);
                        tracing::error!(
                            error = %raw_error,
                            fmtr.error.user_message = %message,
                            fmtr.response.count = response_count,
                            "{context} stream error"
                        );
                        send_actor_message(
                            &myself,
                            BatchMsg::StreamError(crate::BatchFailure::ProgressiveStreamError {
                                provider: provider.to_string(),
                                message,
                            }),
                            context,
                            "stream error",
                        );
                        break;
                    }
                    Ok(None) => {
                        tracing::error!(
                            fmtr.response.count = response_count,
                            "{context} ended without completion signal"
                        );
                        send_actor_message(
                            &myself,
                            BatchMsg::StreamError(
                                crate::BatchFailure::ProgressiveStoppedWithoutCompletionSignal,
                            ),
                            context,
                            "stream error",
                        );
                        break;
                    }
                    Err(elapsed) => {
                        tracing::warn!(
                            fmtr.timeout.elapsed = ?elapsed,
                            fmtr.response.count = response_count,
                            "{context} timeout"
                        );
                        send_actor_message(
                            &myself,
                            BatchMsg::StreamError(crate::BatchFailure::ProgressiveStreamTimeout),
                            context,
                            "timeout error",
                        );
                        break;
                    }
                }
            }
        }
    }

    if completed {
        send_actor_message(&myself, BatchMsg::StreamEnded, context, "stream ended");
    }
    tracing::info!("{context}: processing loop exited");
}

fn send_actor_message(
    myself: &ActorRef<BatchMsg>,
    message: BatchMsg,
    context: &str,
    message_kind: &str,
) {
    if let Err(err) = myself.send_message(message) {
        tracing::error!(
            "{context}: failed to send {message_kind} message: {:?}",
            err
        );
    }
}

#[cfg(test)]
mod test {
    use super::*;

    struct Observer;

    #[ractor::async_trait]
    impl Actor for Observer {
        type Msg = BatchMsg;
        type State = tokio::sync::mpsc::UnboundedSender<BatchMsg>;
        type Arguments = Self::State;

        async fn pre_start(
            &self,
            _: ActorRef<BatchMsg>,
            sender: Self::Arguments,
        ) -> Result<Self::State, ActorProcessingErr> {
            Ok(sender)
        }

        async fn handle(
            &self,
            _: ActorRef<BatchMsg>,
            message: BatchMsg,
            sender: &mut Self::State,
        ) -> Result<(), ActorProcessingErr> {
            let _ = sender.send(message);
            Ok(())
        }
    }

    struct Runtime(tokio::sync::mpsc::UnboundedSender<BatchStreamEvent>);
    impl BatchRuntime for Runtime {
        fn emit(&self, event: BatchEvent) {
            if let BatchEvent::BatchResponseStreamed { event, .. } = event {
                let _ = self.0.send(event);
            }
        }
    }

    async fn state() -> (
        BatchState,
        tokio::sync::mpsc::UnboundedReceiver<BatchStreamEvent>,
        ActorRef<BatchMsg>,
        tokio::task::JoinHandle<()>,
    ) {
        let (events, rx) = tokio::sync::mpsc::unbounded_channel();
        let (tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (actor, handle) = Actor::spawn(None, Observer, tx).await.unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        (
            BatchState {
                runtime: Arc::new(Runtime(events)),
                session_id: "test".into(),
                rx_task: tokio::spawn(async {
                    let _ = shutdown_rx.await;
                }),
                shutdown_tx: Some(shutdown_tx),
                done_notifier: Arc::new(Mutex::new(None)),
                final_result: None,
                accumulator: StreamBatchAccumulator::new(),
                diarization_task: tokio::spawn(std::future::pending()),
                segments: None,
                pending: Default::default(),
                stream_ended: false,
                percentage: 0.0,
            },
            rx,
            actor,
            handle,
        )
    }

    fn terminal() -> BatchStreamEvent {
        BatchStreamEvent::Terminal {
            request_id: "r".into(),
            created: "now".into(),
            duration: 10.0,
            channels: 1,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn slow_diarization_does_not_block_progress_and_completion_waits() {
        let (mut state, mut rx, actor, handle) = state().await;
        let queued = BatchStreamEvent::Segment {
            response: owhisper_interface::stream::StreamResponse::TranscriptResponse {
                start: 0.0,
                duration: 1.0,
                is_final: true,
                speech_final: true,
                from_finalize: false,
                channel: owhisper_interface::stream::Channel {
                    alternatives: vec![owhisper_interface::stream::Alternatives {
                        transcript: "hello".into(),
                        confidence: 1.0,
                        languages: vec![],
                        words: vec![owhisper_interface::stream::Word {
                            word: "hello".into(),
                            punctuated_word: None,
                            start: 0.1,
                            end: 0.2,
                            confidence: 1.0,
                            speaker: None,
                            language: None,
                        }],
                    }],
                },
                metadata: owhisper_interface::stream::Metadata {
                    request_id: "r".into(),
                    model_uuid: "m".into(),
                    extra: None,
                    model_info: owhisper_interface::stream::ModelInfo {
                        name: String::new(),
                        version: String::new(),
                        arch: String::new(),
                    },
                },
                channel_index: vec![0, 1],
            },
            percentage: 0.1,
        };
        BatchActor
            .handle(
                actor.clone(),
                BatchMsg::StreamResponse {
                    event: Box::new(queued),
                },
                &mut state,
            )
            .await
            .unwrap();
        for percentage in [0.2, 0.8] {
            tokio::time::advance(Duration::from_secs(35)).await;
            BatchActor
                .handle(
                    actor.clone(),
                    BatchMsg::StreamResponse {
                        event: Box::new(BatchStreamEvent::Progress {
                            percentage,
                            partial_text: None,
                        }),
                    },
                    &mut state,
                )
                .await
                .unwrap();
            assert_eq!(rx.recv().await.unwrap().percentage(), percentage);
        }
        BatchActor
            .handle(
                actor.clone(),
                BatchMsg::StreamResponse {
                    event: Box::new(terminal()),
                },
                &mut state,
            )
            .await
            .unwrap();
        BatchActor
            .handle(actor.clone(), BatchMsg::StreamEnded, &mut state)
            .await
            .unwrap();
        assert!(state.final_result.is_none());
        assert!(rx.try_recv().is_err());
        BatchActor
            .handle(
                actor.clone(),
                BatchMsg::DiarizationReady(Arc::new(ChannelSegments::from([(
                    0,
                    vec![hypr_transcribe_soniqo::diarize::DiarizeSegment {
                        start_ms: 0,
                        end_ms: 1000,
                        speaker_index: 3,
                    }],
                )]))),
                &mut state,
            )
            .await
            .unwrap();
        let event = rx.recv().await.unwrap();
        assert_eq!(event.percentage(), 0.8);
        let BatchStreamEvent::Segment {
            response: owhisper_interface::stream::StreamResponse::TranscriptResponse { channel, .. },
            ..
        } = event
        else {
            panic!("Expected queued segment");
        };
        assert_eq!(channel.alternatives[0].words[0].speaker, Some(3));
        assert!(matches!(
            rx.recv().await.unwrap(),
            BatchStreamEvent::Terminal { .. }
        ));
        assert!(matches!(state.final_result, Some(Ok(_))));
        BatchActor
            .post_stop(actor.clone(), &mut state)
            .await
            .unwrap();
        actor.stop(None);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn diarization_first_finishes_only_after_asr_and_cancellation_drops_queue() {
        let (mut state, mut rx, actor, handle) = state().await;
        BatchActor
            .handle(
                actor.clone(),
                BatchMsg::DiarizationReady(Arc::new(ChannelSegments::new())),
                &mut state,
            )
            .await
            .unwrap();
        assert!(state.final_result.is_none());
        BatchActor
            .handle(
                actor.clone(),
                BatchMsg::StreamResponse {
                    event: Box::new(terminal()),
                },
                &mut state,
            )
            .await
            .unwrap();
        BatchActor
            .handle(actor.clone(), BatchMsg::StreamEnded, &mut state)
            .await
            .unwrap();
        assert!(matches!(state.final_result, Some(Ok(_))));
        assert!(rx.try_recv().is_ok());
        BatchActor
            .post_stop(actor.clone(), &mut state)
            .await
            .unwrap();
        actor.stop(None);
        handle.await.unwrap();

        let (mut state, mut rx, actor, handle) = self::state().await;
        BatchActor
            .handle(
                actor.clone(),
                BatchMsg::StreamResponse {
                    event: Box::new(terminal()),
                },
                &mut state,
            )
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            BatchActor.post_stop(actor.clone(), &mut state),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(state.pending.is_empty());
        assert!(rx.try_recv().is_err());
        actor.stop(None);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn provider_failure_discards_queued_words_and_ignores_late_labels() {
        let (mut state, mut rx, actor, handle) = state().await;
        BatchActor
            .handle(
                actor.clone(),
                BatchMsg::StreamResponse {
                    event: Box::new(terminal()),
                },
                &mut state,
            )
            .await
            .unwrap();
        BatchActor
            .handle(
                actor.clone(),
                BatchMsg::StreamError(crate::BatchFailure::ProgressiveStreamTimeout),
                &mut state,
            )
            .await
            .unwrap();
        BatchActor
            .handle(
                actor.clone(),
                BatchMsg::DiarizationReady(Arc::new(ChannelSegments::new())),
                &mut state,
            )
            .await
            .unwrap();
        assert!(matches!(state.final_result, Some(Err(_))));
        assert!(rx.try_recv().is_err());
        BatchActor
            .post_stop(actor.clone(), &mut state)
            .await
            .unwrap();
        assert!(state.pending.is_empty());
        actor.stop(None);
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn local_batch_can_finish_after_long_event_gap() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (observer, handle) = Actor::spawn(None, Observer, tx).await.unwrap();
        let (_shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let events = futures_util::stream::once(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(BatchStreamEvent::Terminal {
                request_id: "req".into(),
                created: "now".into(),
                duration: 5100.0,
                channels: 1,
            })
        });
        process_provider_stream(
            Box::pin(events),
            observer.clone(),
            shutdown_rx,
            ProgressiveProvider::WhisperCpp,
            "test",
        )
        .await;
        assert!(matches!(
            rx.recv().await,
            Some(BatchMsg::StreamResponse { .. })
        ));
        assert!(matches!(rx.recv().await, Some(BatchMsg::StreamEnded)));
        observer.stop(None);
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn remote_batch_still_detects_a_stalled_event_stream() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (observer, handle) = Actor::spawn(None, Observer, tx).await.unwrap();
        let (_shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        process_provider_stream(
            Box::pin(futures_util::stream::pending()),
            observer.clone(),
            shutdown_rx,
            ProgressiveProvider::OpenAI,
            "test",
        )
        .await;
        assert!(matches!(
            rx.recv().await,
            Some(BatchMsg::StreamError(
                crate::BatchFailure::ProgressiveStreamTimeout
            ))
        ));
        observer.stop(None);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn local_batch_wait_is_cancellable() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (observer, handle) = Actor::spawn(None, Observer, tx).await.unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        shutdown_tx.send(()).unwrap();
        process_provider_stream(
            Box::pin(futures_util::stream::pending()),
            observer.clone(),
            shutdown_rx,
            ProgressiveProvider::WhisperCpp,
            "test",
        )
        .await;
        assert!(rx.try_recv().is_err());
        observer.stop(None);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn batch_startup_can_be_cancelled_before_response_headers() {
        let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let mut writer = hound::WavWriter::create(
            file.path(),
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        writer.write_sample(0_i16).unwrap();
        writer.finalize().unwrap();
        let (uploaded_tx, uploaded_rx) = tokio::sync::oneshot::channel();
        let uploaded_tx = Arc::new(Mutex::new(Some(uploaded_tx)));
        let app = axum::Router::new().route(
            "/v1/listen",
            axum::routing::post(move |body: axum::body::Body| {
                let uploaded_tx = uploaded_tx.clone();
                async move {
                    axum::body::to_bytes(body, 1024).await.unwrap();
                    if let Some(tx) = uploaded_tx.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                    std::future::pending::<String>().await
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (observer, handle) = Actor::spawn(None, Observer, tx).await.unwrap();
        struct Runtime;
        impl BatchRuntime for Runtime {
            fn emit(&self, _: BatchEvent) {}
        }
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let (done_tx, _done_rx) = tokio::sync::oneshot::channel();
        let (task, shutdown_tx) = spawn_progressive_batch_task(
            BatchArgs {
                runtime: Arc::new(Runtime),
                progressive_provider: ProgressiveProvider::WhisperCpp,
                provider_label: "whispercpp".into(),
                file_path: file.path().to_string_lossy().into_owned(),
                base_url: format!("http://{address}/v1"),
                api_key: String::new(),
                listen_params: Default::default(),
                start_notifier: Arc::new(Mutex::new(Some(start_tx))),
                done_notifier: Arc::new(Mutex::new(Some(done_tx))),
                session_id: "cancel-start".into(),
                diarization: SharedDiarization::disabled(),
            },
            observer.clone(),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), uploaded_rx)
            .await
            .unwrap()
            .unwrap();
        shutdown_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(start_rx.await.is_err());
        observer.stop(None);
        handle.await.unwrap();
        server.abort();
    }

    #[test]
    fn completion_event_result() {
        let event = BatchStreamEvent::Result {
            response: owhisper_interface::batch::Response {
                metadata: serde_json::json!({}),
                results: owhisper_interface::batch::Results { channels: vec![] },
            },
        };
        assert!(is_completion_event(&event));
    }

    #[test]
    fn completion_event_terminal() {
        let event = BatchStreamEvent::Terminal {
            request_id: "req".into(),
            created: "now".into(),
            duration: 1.0,
            channels: 1,
        };
        assert!(is_completion_event(&event));
    }

    #[test]
    fn provider_error_extracts_fields() {
        let event = BatchStreamEvent::Error {
            provider: "deepgram".into(),
            error_message: "boom".into(),
            error_code: Some(429),
        };
        assert_eq!(
            provider_error_from_event(&event),
            Some(("deepgram", "boom", Some(429)))
        );
    }
}

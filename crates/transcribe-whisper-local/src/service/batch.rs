use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use axum::{
    Json,
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use hypr_model_manager::ModelManager;
use hypr_transcribe_core::{
    chunk_pcm_channel, initial_resolved_until, json_error_response, next_resolved_until,
    overall_resolved_audio, record_progress,
};
use owhisper_interface::ListenParams;
use owhisper_interface::batch;
use owhisper_interface::batch_sse::BatchSseMessage;
use owhisper_interface::progress::{InferencePhase, InferenceProgress};
use tokio::sync::mpsc;

use super::response::{TranscriptKind, build_batch_words, build_transcript_response};
use super::{TARGET_SAMPLE_RATE, build_metadata, build_model, transcribe_chunk};

const PROGRESS_CAPACITY: usize = 16;

struct CancelBatchOnDrop(Arc<AtomicBool>);

impl Drop for CancelBatchOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(super) async fn handle_batch(
    audio_file: tempfile::NamedTempFile,
    params: &ListenParams,
    manager: &ModelManager<hypr_whisper_local::LoadedWhisper>,
    model_path: &Path,
) -> Response {
    let model = match manager.get(None).await {
        Ok(model) => model,
        Err(error) => {
            tracing::error!(error = %error, "failed_to_load_model");
            return json_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "model_load_failed",
                error.to_string(),
            );
        }
    };

    let model = model.clone();
    let model_path = model_path.to_path_buf();
    let params = params.clone();

    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    let worker = tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            super::transcribe_batch_file(
                model.as_ref(),
                &model_path,
                audio_file.path(),
                &params,
                worker_cancelled,
                |_| Ok(()),
            )
        }))
    });
    match finish_json_worker(worker, cancelled).await {
        Ok(Ok(Ok(response))) => Json(response).into_response(),
        Ok(Ok(Err(error))) => {
            tracing::error!(error = %error, "batch_transcription_failed");
            json_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "transcription_failed",
                error.to_string(),
            )
        }
        Ok(Err(_)) | Err(_) => json_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transcription_failed",
            "task panicked",
        ),
    }
}

pub(super) async fn handle_batch_sse(
    audio_file: tempfile::NamedTempFile,
    params: &ListenParams,
    manager: &ModelManager<hypr_whisper_local::LoadedWhisper>,
    model_path: &Path,
) -> Response {
    let model = match manager.get(None).await {
        Ok(model) => model,
        Err(error) => {
            tracing::error!(error = %error, "failed_to_load_model");
            return json_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "model_load_failed",
                error.to_string(),
            );
        }
    };

    let model = model.clone();
    let model_path = model_path.to_path_buf();
    let params = params.clone();
    let (event_tx, event_rx) = mpsc::channel::<BatchSseMessage>(PROGRESS_CAPACITY);

    tokio::spawn(async move {
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancellation = CancelBatchOnDrop(cancelled.clone());
        let worker_cancelled = cancelled.clone();
        let worker_tx = event_tx.clone();
        let worker = tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                super::transcribe_batch_file(
                    model.as_ref(),
                    &model_path,
                    audio_file.path(),
                    &params,
                    worker_cancelled.clone(),
                    |event| send_batch_event(&worker_tx, &worker_cancelled, event),
                )
            }))
        });
        let Some(result) = finish_batch_worker(worker, &event_tx, &cancelled).await else {
            tracing::info!("whisper_batch_cancelled");
            return;
        };
        let message = match result {
            Ok(Ok(Ok(response))) => BatchSseMessage::Result { response },
            Ok(Ok(Err(error))) => BatchSseMessage::Error {
                error: "transcription_failed".to_string(),
                detail: error.to_string(),
            },
            Ok(Err(_)) | Err(_) => BatchSseMessage::Error {
                error: "transcription_failed".to_string(),
                detail: "task panicked".to_string(),
            },
        };
        let _ = event_tx.send(message).await;
    });

    batch_sse_response(event_rx)
}

fn finish_json_worker<T>(
    worker: tokio::task::JoinHandle<T>,
    cancelled: Arc<AtomicBool>,
) -> impl std::future::Future<Output = Result<T, tokio::task::JoinError>> {
    let cancellation = CancelBatchOnDrop(cancelled);
    async move {
        let _cancellation = cancellation;
        worker.await
    }
}

async fn finish_batch_worker<T>(
    mut worker: tokio::task::JoinHandle<T>,
    event_tx: &mpsc::Sender<BatchSseMessage>,
    cancelled: &AtomicBool,
) -> Option<Result<T, tokio::task::JoinError>> {
    tokio::select! {
        result = &mut worker => Some(result),
        _ = event_tx.closed() => {
            cancelled.store(true, Ordering::Release);
            let _ = worker.await;
            None
        }
    }
}

fn send_batch_event(
    sender: &mpsc::Sender<BatchSseMessage>,
    cancelled: &AtomicBool,
    mut event: BatchSseMessage,
) -> Result<(), crate::Error> {
    loop {
        check_cancelled(cancelled)?;
        match sender.try_send(event) {
            Ok(()) => return Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                cancelled.store(true, Ordering::Release);
                return check_cancelled(cancelled);
            }
            Err(mpsc::error::TrySendError::Full(pending)) => {
                event = pending;
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
    }
}

fn batch_sse_response(receiver: mpsc::Receiver<BatchSseMessage>) -> Response {
    let events = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|message| {
            let event = Event::default()
                .event(owhisper_interface::batch_sse::EVENT_NAME)
                .json_data(&message)
                .unwrap_or_else(|_| Event::default().event(owhisper_interface::batch_sse::EVENT_NAME).data(r#"{"error":"transcription_failed","detail":"failed to serialize SSE event"}"#));
            (Ok::<_, std::convert::Infallible>(event), receiver)
        })
    });
    Sse::new(events)
        .keep_alive(KeepAlive::new().interval(owhisper_interface::batch_sse::KEEP_ALIVE_INTERVAL))
        .into_response()
}

pub(super) fn transcribe_recorded_file(
    loaded_model: &hypr_whisper_local::LoadedWhisper,
    model_path: &Path,
    audio_path: &Path,
) -> Result<Vec<owhisper_interface::Word2>, crate::Error> {
    let response = super::transcribe_batch_file(
        loaded_model,
        model_path,
        audio_path,
        &ListenParams::default(),
        Arc::new(AtomicBool::new(false)),
        |_| Ok(()),
    )?;
    let words = response
        .results
        .channels
        .into_iter()
        .flat_map(|channel| channel.alternatives.into_iter())
        .flat_map(|alt| alt.words.into_iter())
        .map(|word| owhisper_interface::Word2 {
            text: word.punctuated_word.unwrap_or(word.word),
            speaker: word
                .speaker
                .map(|speaker| owhisper_interface::SpeakerIdentity::Unassigned {
                    index: speaker as u8,
                }),
            confidence: Some(word.confidence as f32),
            start_ms: Some((word.start * 1000.0) as u64),
            end_ms: Some((word.end * 1000.0) as u64),
        })
        .collect();
    Ok(words)
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), crate::Error> {
    if cancelled.load(Ordering::Acquire) {
        Err(crate::Error::protocol("batch transcription cancelled"))
    } else {
        Ok(())
    }
}

fn advance_context(previous_end: &mut Option<usize>, range: std::ops::Range<usize>) -> bool {
    let reset = previous_end.is_some_and(|end| end != range.start);
    *previous_end = Some(range.end);
    reset
}

pub(super) fn transcribe_source(
    audio_path: &Path,
    params: &ListenParams,
    loaded_model: &hypr_whisper_local::LoadedWhisper,
    model_path: &Path,
    cancelled: Arc<AtomicBool>,
    mut emit: impl FnMut(BatchSseMessage) -> Result<(), crate::Error>,
) -> Result<batch::Response, crate::Error> {
    check_cancelled(&cancelled)?;
    let mut emit = |event| {
        check_cancelled(&cancelled)?;
        let result = emit(event);
        if result.is_err() {
            cancelled.store(true, Ordering::Release);
        }
        result?;
        check_cancelled(&cancelled)
    };
    let pcm_file =
        hypr_audio_utils::PcmFile::prepare(audio_path, || cancelled.load(Ordering::Acquire))?;
    check_cancelled(&cancelled)?;
    let pcm = &pcm_file.descriptor;
    let channel_count = pcm.channels;
    let total_duration = pcm.duration();
    tracing::debug!(channel_count, "whisper_batch_audio_prepared");
    let metadata = build_metadata(model_path);
    let channel_durations = vec![total_duration; channel_count];
    let started = std::time::Instant::now();
    let raw_chunks = (0..channel_count)
        .map(|channel| {
            chunk_pcm_channel::<crate::Error>(pcm, channel, || cancelled.load(Ordering::Acquire))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let vad_ms = started.elapsed().as_millis();
    let languages = super::configured_languages(params);
    let mut models = (0..channel_count)
        .map(|_| {
            check_cancelled(&cancelled)?;
            build_model(loaded_model, params)
        })
        .collect::<Result<Vec<_>, _>>()?;
    for model in &mut models {
        model.set_cancellation(cancelled.clone());
    }
    let mut language = super::language::BatchLanguage::new(
        hypr_whisper_local::LanguageResolver::new(&languages),
        if languages.len() == 1 {
            vec![]
        } else {
            super::language::evidence_windows(channel_count, pcm.frames, &raw_chunks)
        },
        pcm,
    );
    language.startup(&mut models[0])?;
    check_cancelled(&cancelled)?;
    let packing_started = std::time::Instant::now();
    let channel_chunks = raw_chunks
        .iter()
        .map(|chunks| {
            let packed =
                super::packing::pack_ranges(pcm.frames, chunks, super::packing::gap_override());
            tracing::info!(
                vad_windows = chunks.len(),
                inference_windows = packed.len(),
                "whisper_packing"
            );
            packed
        })
        .collect::<Vec<_>>();
    let packing_ms = packing_started.elapsed().as_millis();
    drop(raw_chunks);
    let resolved_until = channel_chunks
        .iter()
        .zip(channel_durations.iter().copied())
        .map(|(chunks, channel_duration)| initial_resolved_until(chunks, channel_duration))
        .collect::<Vec<_>>();
    let mut progress = BatchProgress {
        resolved_until,
        total_duration,
        last_progress: 0.0,
    };
    progress.emit(None, &mut emit)?;
    let mut all_segments = vec![Vec::new(); channel_chunks.len()];
    let mut jobs = channel_chunks
        .iter()
        .enumerate()
        .flat_map(|(channel, chunks)| {
            chunks
                .iter()
                .enumerate()
                .map(move |(index, chunk)| (channel, index, chunk))
        })
        .collect::<Vec<_>>();
    jobs.sort_by_key(|(channel, _, chunk)| (chunk.sample_start, *channel));
    let mut reader = pcm.reader()?;
    let mut first_text = true;
    let mut previous_ends = vec![None; channel_count];
    for (channel, index, chunk) in jobs {
        check_cancelled(&cancelled)?;
        language.advance(chunk.sample_start, &mut models[0])?;
        check_cancelled(&cancelled)?;
        for model in &mut models {
            model.select_language(language.resolver.selected());
        }
        if advance_context(
            &mut previous_ends[channel],
            chunk.sample_start..chunk.sample_end,
        ) {
            // Packed ranges can omit minutes of silence. Text from a disconnected
            // range must not condition this range; explicit vocabulary stays intact.
            models[channel].reset_dynamic_prompt();
        }
        let samples = reader.channel(channel, chunk.sample_start..chunk.sample_end)?;
        let segments = transcribe_chunk(
            &mut models[channel],
            &samples,
            chunk.sample_start as f64 / TARGET_SAMPLE_RATE as f64,
        )?;
        check_cancelled(&cancelled)?;
        if first_text && !segments.is_empty() {
            tracing::info!(
                elapsed_ms = started.elapsed().as_millis(),
                "whisper_batch_first_text"
            );
            first_text = false;
        }
        for segment in segments {
            emit(BatchSseMessage::Segment {
                response: build_transcript_response(
                    &segment,
                    TranscriptKind::Confirmed,
                    &metadata,
                    &[channel as i32, channel_chunks.len() as i32],
                ),
            })?;
            all_segments[channel].push(segment);
        }
        progress.resolved_until[channel] =
            next_resolved_until(&channel_chunks[channel], index, channel_durations[channel]);
        progress.emit(Some(join_transcript(&all_segments[channel])), &mut emit)?;
    }
    let response_channels = all_segments
        .into_iter()
        .enumerate()
        .map(|(channel, segments)| batch::Channel {
            alternatives: vec![batch::Alternatives {
                transcript: join_transcript(&segments),
                confidence: if segments.is_empty() {
                    0.0
                } else {
                    segments.iter().map(|s| s.confidence).sum::<f64>() / segments.len() as f64
                },
                words: segments
                    .iter()
                    .flat_map(|s| build_batch_words(s, channel as i32))
                    .collect(),
            }],
        })
        .collect::<Vec<_>>();
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis(),
        channels = channel_chunks.len(),
        vad_ms,
        packing_ms,
        detection_calls = models.iter().map(|m| m.counters().0).sum::<usize>(),
        inference_calls = models.iter().map(|m| m.counters().1).sum::<usize>(),
        "whisper_batch_completed"
    );

    let mut metadata_json = serde_json::to_value(&metadata).unwrap_or_default();
    if let Some(obj) = metadata_json.as_object_mut() {
        obj.insert("duration".to_string(), serde_json::json!(total_duration));
        obj.insert(
            "channels".to_string(),
            serde_json::json!(response_channels.len()),
        );
    }

    check_cancelled(&cancelled)?;
    Ok(batch::Response {
        metadata: metadata_json,
        results: batch::Results {
            channels: response_channels,
        },
    })
}

struct BatchProgress {
    resolved_until: Vec<f64>,
    total_duration: f64,
    last_progress: f64,
}

impl BatchProgress {
    fn emit(
        &mut self,
        partial_text: Option<String>,
        emit: &mut impl FnMut(BatchSseMessage) -> Result<(), crate::Error>,
    ) -> Result<(), crate::Error> {
        let previous = self.last_progress;
        let percentage = record_progress(
            overall_resolved_audio(&self.resolved_until),
            self.total_duration,
            &mut self.last_progress,
        );
        if percentage > previous {
            emit(BatchSseMessage::Progress {
                progress: InferenceProgress {
                    percentage,
                    partial_text,
                    phase: InferencePhase::Decoding,
                },
            })?;
        }
        Ok(())
    }
}

fn join_transcript(segments: &[crate::service::Segment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod performance_tests {
    #[test]
    fn discontinuous_ranges_reset_only_their_own_channel_context() {
        let mut ends = [None, None];
        assert!(!super::advance_context(&mut ends[0], 0..400_000));
        assert!(!super::advance_context(&mut ends[1], 8_000..15_000));
        assert!(!super::advance_context(&mut ends[0], 400_000..800_000));
        assert!(super::advance_context(&mut ends[1], 16_000..30_000));
        assert!(super::advance_context(&mut ends[0], 4_800_000..5_200_000));
        assert!(!super::advance_context(&mut ends[1], 30_000..40_000));
        assert!(super::advance_context(&mut ends[0], 5_000_000..5_400_000));
    }
    use super::*;
    #[test]
    #[ignore = "requires LOOFAH_WHISPER_MODEL; run in release mode"]
    fn batch_benchmark() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("transcribe_whisper_local=info,whisper_local=debug")
            .with_test_writer()
            .try_init();
        let model_path = std::env::var("LOOFAH_WHISPER_MODEL").unwrap();
        let model_path = Path::new(&model_path);
        let load_started = std::time::Instant::now();
        let loaded = super::super::load_model(model_path).unwrap();
        println!("load_ms={}", load_started.elapsed().as_millis());
        let audio_path = std::env::var("LOOFAH_WHISPER_AUDIO")
            .unwrap_or_else(|_| hypr_data::english_1::AUDIO_PATH.into());
        let params = ListenParams {
            languages: std::env::var("LOOFAH_WHISPER_BENCH_LANGUAGES")
                .unwrap_or_else(|_| "en".into())
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().unwrap())
                .collect(),
            ..Default::default()
        };
        for run in 0..4 {
            let started = std::time::Instant::now();
            let response = transcribe_source(
                Path::new(&audio_path),
                &params,
                &loaded,
                model_path,
                Arc::new(AtomicBool::new(false)),
                |_| Ok(()),
            )
            .unwrap();
            let elapsed = started.elapsed();
            let words: Vec<_> = response
                .results
                .channels
                .iter()
                .flat_map(|c| &c.alternatives[0].words)
                .collect();
            assert!(!words.is_empty());
            assert!(
                words
                    .iter()
                    .all(|w| w.start.is_finite() && w.end >= w.start)
            );
            println!(
                "batch_run={run} elapsed_ms={} words={}",
                elapsed.as_millis(),
                words.len()
            );
            if let Ok(path) = std::env::var("LOOFAH_WHISPER_BENCH_OUTPUT") {
                std::fs::write(
                    format!("{path}-{run}.json"),
                    serde_json::to_vec_pretty(&response).unwrap(),
                )
                .unwrap();
            }
        }
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;

    fn event() -> BatchSseMessage {
        BatchSseMessage::Error {
            error: "test".into(),
            detail: "test".into(),
        }
    }

    #[tokio::test]
    async fn a_full_progress_queue_stays_bounded_and_cancels_without_dropping_its_receiver() {
        let (tx, receiver) = mpsc::channel(PROGRESS_CAPACITY);
        for _ in 0..PROGRESS_CAPACITY {
            tx.try_send(event()).unwrap();
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let work = tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            send_batch_event(&tx, &worker_cancelled, event())
        });
        started_rx.await.unwrap();
        assert_eq!(receiver.len(), PROGRESS_CAPACITY);
        cancelled.store(true, Ordering::Release);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), work)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert_eq!(receiver.len(), PROGRESS_CAPACITY);
    }

    #[tokio::test]
    async fn a_full_progress_queue_stops_after_receiver_disconnect() {
        let (tx, receiver) = mpsc::channel(PROGRESS_CAPACITY);
        for _ in 0..PROGRESS_CAPACITY {
            tx.try_send(event()).unwrap();
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let work =
            tokio::task::spawn_blocking(move || send_batch_event(&tx, &worker_cancelled, event()));
        drop(receiver);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), work)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn dropping_json_wait_requests_cancel_but_worker_keeps_audio_and_admission_until_done() {
        let admission = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = admission.clone().acquire_owned().await.unwrap();
        let audio = tempfile::NamedTempFile::new().unwrap();
        let audio_path = audio.path().to_path_buf();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = tokio::task::spawn_blocking(move || {
            let permit_owner = permit;
            let audio_owner = audio;
            started_tx.send(()).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !worker_cancelled.load(Ordering::Acquire) && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert!(worker_cancelled.load(Ordering::Acquire));
            observed_tx.send(()).unwrap();
            release_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            drop(audio_owner);
            drop(permit_owner);
            done_tx.send(()).unwrap();
        });
        let request = tokio::spawn(finish_json_worker(worker, cancelled.clone()));
        started_rx.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(5), observed_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(audio_path.exists());
        assert_eq!(admission.available_permits(), 0);
        release_tx.send(()).unwrap();
        done_rx.await.unwrap();
        assert!(!audio_path.exists());
        assert_eq!(admission.available_permits(), 1);
    }

    #[tokio::test]
    async fn dropping_sse_body_cancels_and_finishes_blocking_worker() {
        let (tx, rx) = mpsc::channel(PROGRESS_CAPACITY);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let worker = tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            while !worker_cancelled.load(Ordering::Acquire) {
                std::thread::park_timeout(std::time::Duration::from_millis(1));
            }
        });
        started_rx.await.unwrap();
        let response = batch_sse_response(rx);
        drop(response);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            finish_batch_worker(worker, &tx, &cancelled),
        )
        .await;
        let signalled = cancelled.load(Ordering::Acquire);
        cancelled.store(true, Ordering::Release);
        assert!(
            result
                .expect("disconnected batch left its blocking worker running")
                .is_none()
        );
        assert!(signalled);
    }
}

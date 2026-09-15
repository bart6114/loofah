use hypr_audio_chunking::AudioChunk;
use hypr_whisper_local::{LanguageResolver, LoadedWhisper};
use owhisper_interface::ListenParams;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use tokio::sync::{OwnedSemaphorePermit, mpsc, oneshot};

pub(super) struct Work {
    pub channel: usize,
    pub chunk: AudioChunk,
    pub permits: Vec<OwnedSemaphorePermit>,
    pub queued: Instant,
}

pub(super) enum Job {
    Audio(Work),
    Evidence(Vec<f32>),
    Deadline,
    Finish { duration: f64, finalized: bool },
}

pub(super) enum Output {
    Segment(usize, super::Segment, bool),
    Finished { duration: f64, finalized: bool },
    Error(String),
}

pub(super) fn start(
    loaded: Arc<LoadedWhisper>,
    params: ListenParams,
    cancelled: Arc<AtomicBool>,
) -> (
    mpsc::Sender<Job>,
    mpsc::Receiver<Output>,
    oneshot::Receiver<()>,
) {
    let (tx, mut rx) = mpsc::channel::<Job>(32);
    let (out, output) = mpsc::channel(16);
    let (done, completion) = oneshot::channel();
    std::thread::Builder::new()
        .name("whisper-live".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || -> Result<(), crate::Error> {
                    let languages = super::configured_languages(&params);
                    let mut resolver = LanguageResolver::new(&languages);
                    let mut models = (0..usize::from(params.channels).clamp(1, 2))
                        .map(|_| {
                            let mut model = super::build_model(&loaded, &params)?;
                            model.set_cancellation(cancelled.clone());
                            Ok(model)
                        })
                        .collect::<Result<Vec<_>, crate::Error>>()?;
                    let mut pending = VecDeque::new();
                    let mut deadline = false;
                    while let Some(job) = rx.blocking_recv() {
                        if cancelled.load(Ordering::Acquire) {
                            break;
                        }
                        let mut finish = None;
                        match job {
                            Job::Audio(work) => pending.push_back(work),
                            Job::Evidence(samples) => {
                                resolver.add_speech(samples.len());
                                if resolver.needs_observation() {
                                    resolver.observe(models[0].detect_language(&samples)?);
                                    if deadline {
                                        resolver.finish_startup();
                                    }
                                }
                            }
                            Job::Deadline => {
                                deadline = true;
                                resolver.finish_startup();
                                if resolver.selected().is_none()
                                    && let Some(work) = pending.front()
                                {
                                    resolver.observe(
                                        models[work.channel]
                                            .detect_language(&work.chunk.samples)?,
                                    );
                                    resolver.finish_startup();
                                }
                            }
                            Job::Finish {
                                duration,
                                finalized,
                            } => {
                                resolver.finish_startup();
                                if resolver.selected().is_none()
                                    && let Some(work) = pending.front()
                                {
                                    resolver.observe(
                                        models[work.channel]
                                            .detect_language(&work.chunk.samples)?,
                                    );
                                    resolver.finish_startup();
                                }
                                finish = Some((duration, finalized));
                            }
                        }
                        if resolver.selected().is_some() || finish.is_some() {
                            for model in &mut models {
                                model.select_language(resolver.selected());
                            }
                            while let Some(work) = pending.pop_front() {
                                if cancelled.load(Ordering::Acquire) {
                                    return Ok(());
                                }
                                let model = &mut models[work.channel];
                                tracing::debug!(
                                    channel = work.channel,
                                    queue_ms = work.queued.elapsed().as_millis(),
                                    samples = work.chunk.samples.len(),
                                    "whisper_live_inference"
                                );
                                let segments = super::transcribe_chunk(
                                    model,
                                    &work.chunk.samples,
                                    work.chunk.sample_start as f64 / 16000.0,
                                )?;
                                drop(work.permits);
                                for segment in segments {
                                    if cancelled.load(Ordering::Acquire)
                                        || out
                                            .blocking_send(Output::Segment(
                                                work.channel,
                                                segment,
                                                finish.is_some_and(|(_, finalized)| finalized),
                                            ))
                                            .is_err()
                                    {
                                        return Ok(());
                                    }
                                }
                            }
                        }
                        if let Some((duration, finalized)) = finish {
                            tracing::info!(
                                detection_calls =
                                    models.iter().map(|m| m.counters().0).sum::<usize>(),
                                inference_calls =
                                    models.iter().map(|m| m.counters().1).sum::<usize>(),
                                "whisper_live_counters"
                            );
                            let _ = out.blocking_send(Output::Finished {
                                duration,
                                finalized,
                            });
                            break;
                        }
                    }
                    Ok(())
                },
            ));
            if !cancelled.load(Ordering::Acquire) {
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some(e.to_string()),
                    Err(_) => Some("inference worker panicked".into()),
                };
                if let Some(error) = error {
                    let _ = out.blocking_send(Output::Error(error));
                }
            }
            let _ = done.send(());
        })
        .expect("start whisper worker");
    (tx, output, completion)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    #[ignore = "requires LOOFAH_WHISPER_MODEL; exercises real inference with blocked output"]
    async fn cancellation_releases_saturated_output_and_audio_budget() {
        let loaded = tokio::task::spawn_blocking(|| {
            super::super::load_model(std::path::Path::new(
                &std::env::var("LOOFAH_WHISPER_MODEL").unwrap(),
            ))
            .unwrap()
        })
        .await
        .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, out, completion) = start(
            Arc::new(loaded),
            ListenParams {
                languages: vec!["en".parse().unwrap()],
                channels: 1,
                ..Default::default()
            },
            cancelled.clone(),
        );
        let budget = Arc::new(tokio::sync::Semaphore::new(
            super::super::live_input::AUDIO_BUDGET,
        ));
        let producer_budget = budget.clone();
        let producer = tokio::spawn(async move {
            let samples: Vec<f32> = hypr_data::english_1::AUDIO
                .chunks_exact(2)
                .take(160000)
                .map(|p| i16::from_le_bytes([p[0], p[1]]) as f32 / 32768.0)
                .collect();
            for index in 0..64 {
                let held = producer_budget
                    .clone()
                    .acquire_many_owned(samples.len() as u32)
                    .await
                    .unwrap();
                let Ok(permit) = tx.reserve().await else {
                    break;
                };
                permit.send(Job::Audio(Work {
                    channel: 0,
                    chunk: AudioChunk {
                        sample_start: index * samples.len(),
                        sample_end: (index + 1) * samples.len(),
                        samples: samples.clone(),
                    },
                    permits: vec![held],
                    queued: Instant::now(),
                }));
            }
        });
        let saturated = tokio::time::timeout(Duration::from_secs(60), async {
            while out.len() < 16 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        cancelled.store(true, Ordering::Release);
        drop(out);
        tokio::time::timeout(Duration::from_secs(10), completion)
            .await
            .unwrap()
            .unwrap();
        producer.await.unwrap();
        assert!(saturated.is_ok(), "output never filled");
        assert_eq!(
            budget.available_permits(),
            super::super::live_input::AUDIO_BUDGET
        );
    }
}

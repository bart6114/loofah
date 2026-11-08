use super::*;
use hypr_transcribe_core::{
    recorded_audio::PreparedAudio,
    transcript::{BatchTranscriptMeta, transcript_from_batch_response},
};
use owhisper_interface::batch;

pub(super) async fn run(
    prepared: Prepared,
    input: TranscriptionInput,
    cancel: CancellationToken,
) -> Result<hypr_vault_read::TranscriptWithData, TranscriptionFailure> {
    if input.request != prepared.request || input.metadata.session_id != input.request.session_id {
        return Err(TranscriptionFailure::InvalidOutput);
    }
    let flag = Arc::new(AtomicBool::new(cancel.is_cancelled()));
    let bridge_flag = flag.clone();
    let bridge_cancel = cancel.clone();
    let host_shutdown = prepared.resources.shutdown.clone();
    let done = CancellationToken::new();
    let bridge_done = done.clone();
    let bridge = tokio::spawn(async move {
        tokio::select! {
            _ = bridge_cancel.cancelled() => bridge_flag.store(true, Ordering::Release),
            _ = host_shutdown.cancelled() => bridge_flag.store(true, Ordering::Release),
            _ = bridge_done.cancelled() => {},
        }
    });
    let resources = prepared.resources.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = prepared.permit;
        let (path, _source) = input.source.into_parts();
        let stopped = || flag.load(Ordering::Acquire) || resources.stopping.load(Ordering::Acquire);
        if stopped() {
            return Err(TranscriptionFailure::Interrupted);
        }
        let audio = PreparedAudio::from_path(&path, input.metadata.audio, &flag).map_err(|_| {
            if stopped() {
                TranscriptionFailure::Interrupted
            } else {
                TranscriptionFailure::SourceUnavailable
            }
        })?;
        // Agglomerative speaker clustering has quadratic scratch use. Bound the
        // complete file before model dispatch, not after collecting embeddings.
        if audio.pcm.frames > 16000 * 2 * 60 * 60 {
            return Err(TranscriptionFailure::ResourceLimit);
        }
        let operation = || -> anyhow::Result<hypr_vault_read::TranscriptWithData> {
            anyhow::ensure!(!stopped(), "Stopped");
            let mut models = resources.models.lock().unwrap_or_else(|e| e.into_inner());
            let models = models
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Models unavailable"))?;
            let mut channels = Vec::new();
            for channel in 0..audio.pcm.channels {
                anyhow::ensure!(!stopped(), "Stopped");
                let chunks = hypr_transcribe_core::chunk_pcm_channel::<anyhow::Error>(
                    &audio.pcm, channel, stopped,
                )?;
                let mut reader = audio.pcm.reader()?;
                let mut words = Vec::new();
                for chunk in chunks {
                    anyhow::ensure!(!stopped(), "Stopped");
                    let samples = reader.channel(channel, chunk.sample_start..chunk.sample_end)?;
                    let file = tempfile::NamedTempFile::new()?;
                    let mut writer =
                        hound::WavWriter::create(file.path(), hypr_audio_utils::pcm_spec(1))?;
                    for sample in samples {
                        writer.write_sample(sample)?;
                    }
                    writer.finalize()?;
                    let (_, segments) = models.batch.transcribe_file(file.path())?;
                    let offset = chunk.sample_start as f64 / 16000.0;
                    let end = chunk.sample_end as f64 / 16000.0;
                    for segment in segments {
                        anyhow::ensure!(
                            segment.start.is_finite()
                                && segment.end.is_finite()
                                && segment.start >= 0.0
                                && segment.end >= segment.start,
                            "Invalid word timing"
                        );
                        let start = (offset + segment.start).min(end);
                        let word_end = (offset + segment.end).min(end);
                        if segment.text.trim().is_empty() {
                            continue;
                        }
                        words.push(batch::Word {
                            word: segment.text.clone(),
                            start,
                            end: word_end,
                            confidence: 1.0,
                            channel: channel as i32,
                            speaker: None,
                            punctuated_word: Some(segment.text),
                        });
                    }
                }
                // The microphone remains the user's channel. Diarization labels mixed audio or system audio only.
                if audio.pcm.channels == 1 || channel == 1 {
                    let speakers =
                        models
                            .diarizer
                            .process_file_cancellable(&audio.path, channel, &stopped)?;
                    for word in &mut words {
                        let middle = ((word.start + word.end) * 500.0).round() as i64;
                        word.speaker = speakers
                            .iter()
                            .find(|segment| segment.start_ms <= middle && middle < segment.end_ms)
                            .and_then(|segment| usize::try_from(segment.speaker_index).ok());
                    }
                }
                let transcript = words
                    .iter()
                    .map(|word| word.word.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                channels.push(batch::Channel {
                    alternatives: vec![batch::Alternatives {
                        transcript,
                        confidence: 1.0,
                        words,
                    }],
                });
            }
            anyhow::ensure!(!stopped(), "Stopped");
            let mut response = batch::Response {
                metadata: serde_json::json!({"duration": audio.pcm.frames as f64 / 16000.0}),
                results: batch::Results { channels },
            };
            audio.stamp_response(&mut response);
            let metadata = &input.metadata;
            Ok(transcript_from_batch_response(
                &response,
                BatchTranscriptMeta {
                    session_id: &metadata.session_id,
                    user_id: &metadata.user_id,
                    provider: "onnx-parakeet",
                    created_at: metadata.created_at.clone(),
                    started_at_ms: metadata.started_at_ms(),
                    memo_md: metadata.memo_md.clone(),
                },
                &mut || uuid::Uuid::new_v4().to_string(),
            )
            .unwrap_or_else(|| metadata.empty_transcript()))
        };
        operation().map_err(|_| {
            if stopped() {
                TranscriptionFailure::Interrupted
            } else {
                TranscriptionFailure::EngineFailed
            }
        })
    })
    .await;
    done.cancel();
    let _ = bridge.await;
    if cancel.is_cancelled() {
        return Err(TranscriptionFailure::Interrupted);
    }
    result.unwrap_or_else(|_| {
        prepared.resources.phase.store(FAILED, Ordering::Release);
        Err(TranscriptionFailure::EngineFailed)
    })
}

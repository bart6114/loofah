use std::io::Write;
use std::path::Path;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use hypr_model_manager::ModelManager;
use hypr_transcribe_core::{
    ProgressTracker, batch_sse_response, channel_duration_sec, chunk_channel_audio,
    initial_resolved_until, json_error_response, next_resolved_until, split_resampled_channels,
};
use owhisper_interface::ListenParams;
use owhisper_interface::batch;
use owhisper_interface::batch_sse::BatchSseMessage;
use rodio::Source;
use tokio::sync::mpsc;

use super::response::{TranscriptKind, build_batch_words, build_transcript_response};
use super::{TARGET_SAMPLE_RATE, build_metadata, build_model, transcribe_chunk};

pub(super) async fn handle_batch(
    body: Bytes,
    content_type: &str,
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
    let content_type = content_type.to_string();
    let params = params.clone();

    match tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transcribe_batch(
                &body,
                &content_type,
                &params,
                model.as_ref(),
                &model_path,
                None,
            )
        }))
    })
    .await
    {
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
    body: Bytes,
    content_type: &str,
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
    let content_type = content_type.to_string();
    let params = params.clone();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<BatchSseMessage>();

    tokio::task::spawn_blocking(move || {
        let message = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transcribe_batch(
                &body,
                &content_type,
                &params,
                model.as_ref(),
                &model_path,
                Some(event_tx.clone()),
            )
        })) {
            Ok(Ok(response)) => BatchSseMessage::Result { response },
            Ok(Err(error)) => BatchSseMessage::Error {
                error: "transcription_failed".to_string(),
                detail: error.to_string(),
            },
            Err(_) => BatchSseMessage::Error {
                error: "transcription_failed".to_string(),
                detail: "task panicked".to_string(),
            },
        };

        let _ = event_tx.send(message);
    });

    batch_sse_response(event_rx)
}

fn transcribe_batch(
    audio_data: &[u8],
    content_type: &str,
    params: &ListenParams,
    loaded_model: &hypr_whisper_local::LoadedWhisper,
    model_path: &Path,
    event_tx: Option<mpsc::UnboundedSender<BatchSseMessage>>,
) -> Result<batch::Response, crate::Error> {
    let extension = hypr_audio_utils::content_type_to_extension(content_type);
    let mut temp_file = tempfile::Builder::new()
        .prefix("whisper_local_batch_")
        .suffix(&format!(".{}", extension))
        .tempfile()?;

    temp_file.write_all(audio_data)?;
    temp_file.flush()?;

    let source = hypr_audio_utils::source_from_path(temp_file.path())?;
    transcribe_source(source, params, loaded_model, model_path, event_tx)
}

pub(super) fn transcribe_recorded_file(
    loaded_model: &hypr_whisper_local::LoadedWhisper,
    model_path: &Path,
    audio_path: &Path,
) -> Result<Vec<owhisper_interface::Word2>, crate::Error> {
    let source = hypr_audio_utils::source_from_path(audio_path)?;
    let response = transcribe_source(
        source,
        &ListenParams::default(),
        loaded_model,
        model_path,
        None,
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

fn transcribe_source<S>(
    source: S,
    params: &ListenParams,
    loaded_model: &hypr_whisper_local::LoadedWhisper,
    model_path: &Path,
    event_tx: Option<mpsc::UnboundedSender<BatchSseMessage>>,
) -> Result<batch::Response, crate::Error>
where
    S: Source<Item = f32>,
{
    let channel_count = u16::from(source.channels()).max(1) as usize;
    let resampled = hypr_audio_utils::resample_audio(source, TARGET_SAMPLE_RATE)?;
    let channel_samples = split_resampled_channels(&resampled, channel_count);
    let total_duration = channel_samples
        .iter()
        .map(|samples| channel_duration_sec(samples))
        .fold(0.0_f64, f64::max);

    let metadata = build_metadata(model_path);
    let channel_durations = channel_samples
        .iter()
        .map(|samples| channel_duration_sec(samples))
        .collect::<Vec<_>>();
    let started = std::time::Instant::now();
    let raw_chunks = channel_samples
        .iter()
        .map(|samples| chunk_channel_audio::<crate::Error>(samples))
        .collect::<Result<Vec<_>, _>>()?;
    let mut language = super::language::BatchLanguage::new(
        hypr_whisper_local::LanguageResolver::new(&super::configured_languages(params)),
        super::language::evidence_windows(&channel_samples, &raw_chunks),
    );
    let mut detector = build_model(loaded_model, params)?;
    language.startup(&mut detector)?;
    let channel_chunks = channel_samples
        .iter()
        .zip(&raw_chunks)
        .map(|(samples, chunks)| {
            let packed = super::packing::pack(samples, chunks, super::packing::gap_override());
            tracing::info!(
                vad_windows = chunks.len(),
                inference_windows = packed.len(),
                "whisper_packing"
            );
            packed
        })
        .collect::<Vec<_>>();
    drop(raw_chunks);
    let resolved_until = channel_chunks
        .iter()
        .zip(channel_durations.iter().copied())
        .map(|(chunks, channel_duration)| initial_resolved_until(chunks, channel_duration))
        .collect::<Vec<_>>();
    let mut progress = ProgressTracker::new(resolved_until, total_duration, event_tx);
    progress.emit(None);
    let mut models = channel_chunks
        .iter()
        .map(|_| build_model(loaded_model, params))
        .collect::<Result<Vec<_>, _>>()?;
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
    for (channel, index, chunk) in jobs {
        language.advance(chunk.sample_start, &mut detector)?;
        models[channel].select_language(language.resolver.selected());
        let segments = transcribe_chunk(
            &mut models[channel],
            &chunk.samples,
            chunk.sample_start as f64 / TARGET_SAMPLE_RATE as f64,
        )?;
        for segment in segments {
            if let Some(tx) = progress.event_tx() {
                let _ = tx.send(BatchSseMessage::Segment {
                    response: build_transcript_response(
                        &segment,
                        TranscriptKind::Confirmed,
                        &metadata,
                        &[channel as i32, channel_chunks.len() as i32],
                    ),
                });
            }
            all_segments[channel].push(segment);
        }
        progress.update_channel(
            channel,
            next_resolved_until(&channel_chunks[channel], index, channel_durations[channel]),
        );
        progress.emit(Some(join_transcript(&all_segments[channel])));
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

    Ok(batch::Response {
        metadata: metadata_json,
        results: batch::Results {
            channels: response_channels,
        },
    })
}

fn join_transcript(segments: &[crate::service::Segment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

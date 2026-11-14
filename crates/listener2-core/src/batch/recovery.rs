use std::sync::Arc;

use hypr_fs_format::{BatchCompletionStatus, BatchRecovery, FailedAudioRange};

use super::{BatchRunMode, BatchRunOutput, audio::PreparedAudio, diarize::stamp_batch_response};
use crate::BatchRuntime;

pub async fn retry_missing_audio(
    runtime: Arc<dyn BatchRuntime>,
    session_id: String,
    file_path: String,
    mut recovery: BatchRecovery,
) -> Result<BatchRunOutput, String> {
    if recovery.version != 1 || recovery.provider != "soniqo" {
        return Err(
            "This recovery plan is unsupported. Transcribe the full recording again.".into(),
        );
    }
    let path = file_path.clone();
    let fingerprint = tokio::task::spawn_blocking(move || {
        super::source_audio_fingerprint(std::path::Path::new(&path))
    })
    .await
    .map_err(|e| e.to_string())??;
    if fingerprint != recovery.source_fingerprint {
        return Err("The recording changed. Transcribe the full recording again.".into());
    }
    let model = recovery
        .model
        .parse::<hypr_transcribe_soniqo::SoniqoModel>()
        .map_err(|e| e.to_string())?;
    if !hypr_transcribe_soniqo::is_model_downloaded(model).map_err(|e| e.to_string())? {
        return Err(format!(
            "Download {} in Settings → Transcription, then retry missing audio.",
            model.display_name()
        ));
    }
    let languages = recovery
        .languages
        .iter()
        .map(|l| l.parse::<hypr_language::Language>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let language = model.language_config(&languages);
    let prepared = PreparedAudio::prepare(file_path, recovery.audio, Some(runtime.clone()))
        .await
        .map_err(|e| e.to_string())?;
    if recovery.diarization_pending {
        let segments = super::diarize::SharedDiarization::for_file(
            Arc::new(super::diarize::SoniqoDiarizer),
            prepared.clone(),
        )
        .segments()
        .await
        .map_err(|e| e.to_string())?;
        recovery.diarization = segments
            .iter()
            .flat_map(|(channel, segments)| {
                segments
                    .iter()
                    .map(move |segment| hypr_fs_format::RecoverySpeakerSegment {
                        channel: *channel as u32,
                        start_ms: segment.start_ms,
                        end_ms: segment.end_ms,
                        speaker_index: segment.speaker_index,
                    })
            })
            .collect();
        recovery.diarization_pending = false;
    }
    let segments: super::diarize::ChannelSegments =
        recovery
            .diarization
            .iter()
            .fold(Default::default(), |mut segments, segment| {
                segments.entry(segment.channel as i32).or_default().push(
                    hypr_transcribe_soniqo::diarize::DiarizeSegment {
                        start_ms: segment.start_ms,
                        end_ms: segment.end_ms,
                        speaker_index: segment.speaker_index,
                    },
                );
                segments
            });
    for range in &recovery.unresolved {
        if range.channel as usize >= prepared.pcm.channels
            || range.start_sample >= range.end_sample
            || range.end_sample > prepared.pcm.frames as u64
        {
            return Err("Saved audio ranges no longer match the recording. Transcribe the full recording again.".into());
        }
    }
    recovery.planning_pending = false;
    let mut splits = Vec::new();
    let cap = 16000 * 59 / 2;
    for original in recovery.unresolved.clone() {
        if original.end_sample.saturating_sub(original.start_sample) > cap {
            let parts: Vec<_> = (original.start_sample..original.end_sample)
                .step_by(cap as usize)
                .map(|start| FailedAudioRange {
                    channel: original.channel,
                    start_sample: start,
                    end_sample: (start + cap).min(original.end_sample),
                    reason: original.reason.clone(),
                })
                .collect();
            recovery.unresolved.retain(|r| !same_range(r, &original));
            recovery.unresolved.extend(parts.clone());
            splits.push((original, parts));
        }
    }
    let pending = recovery.unresolved.clone();
    let mut output = BatchRunOutput {
        session_id: session_id.clone(),
        mode: BatchRunMode::Direct,
        status: BatchCompletionStatus::Incomplete,
        unresolved: pending.clone(),
        response: hypr_transcribe_soniqo::batch_response_from_channels(model, Vec::new()),
    };
    if !splits.is_empty() {
        output.response.metadata["batch_recovery"] =
            serde_json::to_value(&recovery).map_err(|e| e.to_string())?;
        output.response.metadata["batch_recovery_resolved"] = serde_json::json!([]);
        output.response.metadata["batch_recovery_split"] =
            serde_json::to_value(splits).map_err(|e| e.to_string())?;
        runtime.checkpoint(&mut output).await?;
    }
    for range in pending {
        if runtime.is_cancelled() {
            recovery.status = BatchCompletionStatus::Cancelled;
            break;
        }
        if range.channel as usize >= prepared.pcm.channels
            || range.start_sample >= range.end_sample
            || range.end_sample > prepared.pcm.frames as u64
        {
            return Err("Saved audio ranges no longer match the recording. Transcribe the full recording again.".into());
        }
        let decode_audio = prepared.clone();
        let requested = range.clone();
        let language = language.clone();
        let channels = tokio::task::spawn_blocking(move || {
            super::simple::decode_recovery_range(model, &decode_audio.pcm, &requested, &language)
        })
        .await
        .map_err(|e| e.to_string())?;
        match channels {
            Ok(channels) => {
                recovery.unresolved.retain(|r| !same_range(r, &range));
                let mut response =
                    hypr_transcribe_soniqo::batch_response_from_channels(model, channels);
                stamp_batch_response(&mut response, &segments);
                prepared.stamp_response(&mut response);
                recovery.status = if recovery.unresolved.is_empty() {
                    BatchCompletionStatus::Complete
                } else {
                    BatchCompletionStatus::Incomplete
                };
                response.metadata["batch_recovery"] =
                    serde_json::to_value(&recovery).map_err(|e| e.to_string())?;
                response.metadata["batch_recovery_resolved"] =
                    serde_json::to_value(vec![range]).map_err(|e| e.to_string())?;
                output = BatchRunOutput {
                    session_id: session_id.clone(),
                    mode: BatchRunMode::Direct,
                    status: recovery.status,
                    unresolved: recovery.unresolved.clone(),
                    response,
                };
                runtime.checkpoint(&mut output).await?;
            }
            Err(reason) => {
                if let Some(failed) = recovery
                    .unresolved
                    .iter_mut()
                    .find(|r| same_range(r, &range))
                {
                    failed.reason = reason;
                }
            }
        }
    }
    output.status = recovery.status;
    output.unresolved = recovery.unresolved.clone();
    output.response.metadata["batch_recovery"] =
        serde_json::to_value(recovery).map_err(|e| e.to_string())?;
    output.response.metadata["batch_recovery_resolved"] = serde_json::json!([]);
    runtime.checkpoint(&mut output).await?;
    Ok(output)
}

fn same_range(a: &FailedAudioRange, b: &FailedAudioRange) -> bool {
    a.channel == b.channel && a.start_sample == b.start_sample && a.end_sample == b.end_sample
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Runtime;
    impl BatchRuntime for Runtime {
        fn emit(&self, _: crate::BatchEvent) {}
    }

    #[tokio::test]
    async fn replaced_audio_is_rejected_before_model_loading_or_decoding() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"replacement audio").unwrap();
        let recovery = BatchRecovery {
            version: 1,
            run_id: "saved-run".into(),
            source_fingerprint: "sha256:original".into(),
            audio: Default::default(),
            provider: "soniqo".into(),
            model: "soniqo-parakeet-batch".into(),
            languages: vec!["en".into()],
            status: BatchCompletionStatus::Incomplete,
            unresolved: vec![],
            diarization: vec![],
            diarization_pending: false,
            planning_pending: false,
        };
        let error = retry_missing_audio(
            Arc::new(Runtime),
            "session".into(),
            file.path().to_string_lossy().into_owned(),
            recovery,
        )
        .await
        .unwrap_err();
        assert!(error.contains("recording changed"));
    }

    #[test]
    fn source_fingerprint_hashes_actual_file_bytes() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"abc").unwrap();
        assert_eq!(
            super::super::source_audio_fingerprint(file.path()).unwrap(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}

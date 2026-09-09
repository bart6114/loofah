use crate::{Error, ModelDownloadState, Result};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiarizeSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker_index: i32,
}

pub fn model_download_state() -> Result<ModelDownloadState> {
    ensure_supported_platform()?;
    platform::model_download_state()
}

pub fn start_model_download() -> Result<()> {
    ensure_supported_platform()?;
    platform::start_model_download()
}

pub fn is_ready() -> bool {
    model_download_state().is_ok_and(|state| state.status == "ready")
}

pub fn diarize_samples(samples: &[f32], sample_rate_hz: u32) -> Result<Vec<DiarizeSegment>> {
    ensure_supported_platform()?;
    platform::diarize_samples(samples, sample_rate_hz)
}

fn ensure_supported_platform() -> Result<()> {
    if cfg!(any(
        target_os = "windows",
        all(target_os = "macos", target_arch = "aarch64")
    )) {
        Ok(())
    } else {
        Err(Error::UnsupportedPlatform)
    }
}

#[cfg_attr(
    not(any(test, all(target_os = "macos", target_arch = "aarch64"))),
    allow(dead_code)
)]
fn parse_run_payload(payload: &str) -> Result<Vec<DiarizeSegment>> {
    #[derive(serde::Deserialize)]
    struct RunPayload {
        #[serde(default)]
        segments: Vec<DiarizeSegment>,
        error: Option<String>,
    }

    let payload: RunPayload = serde_json::from_str(payload)?;

    if let Some(error) = payload.error {
        return Err(Error::Bridge(error));
    }

    Ok(payload.segments)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod platform {
    use super::*;
    use swift_rs::{Bool, Int, SRData, SRString, swift};

    swift!(fn _diarize_model_download_state() -> SRString);
    swift!(fn _diarize_start_model_download() -> Bool);
    swift!(fn _diarize_run(samples: &SRData, sample_rate_hz: Int) -> SRString);

    pub(super) fn model_download_state() -> Result<ModelDownloadState> {
        let payload = unsafe { _diarize_model_download_state() };
        let state: ModelDownloadState = serde_json::from_str(payload.as_str())?;

        Ok(state)
    }

    pub(super) fn start_model_download() -> Result<()> {
        if unsafe { _diarize_start_model_download() } {
            Ok(())
        } else {
            Err(Error::Bridge(
                "failed to start speaker model download".to_string(),
            ))
        }
    }

    pub(super) fn diarize_samples(
        samples: &[f32],
        sample_rate_hz: u32,
    ) -> Result<Vec<DiarizeSegment>> {
        let samples = floats_to_sr_data(samples);
        let payload = unsafe { _diarize_run(&samples, sample_rate_hz as Int) };

        parse_run_payload(payload.as_str())
    }

    fn floats_to_sr_data(samples: &[f32]) -> SRData {
        let bytes = samples
            .iter()
            .flat_map(|sample| sample.to_bits().to_le_bytes())
            .collect::<Vec<_>>();
        SRData::from(bytes.as_slice())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use hypr_transcribe_onnx::{
        diarize::Diarizer,
        models::{self, Model},
    };
    static OPERATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(super) fn model_download_state() -> Result<ModelDownloadState> {
        let state = models::state(Model::Diarizer).map_err(|e| Error::Bridge(e.to_string()))?;
        Ok(ModelDownloadState {
            status: state.status,
            current_file: state.current_file,
            progress_percent: state.progress_percent,
            local_path: state.local_path,
            error: state.error,
        })
    }
    pub(super) fn start_model_download() -> Result<()> {
        models::start(Model::Diarizer).map_err(|e| Error::Bridge(e.to_string()))
    }
    pub(super) fn diarize_samples(
        samples: &[f32],
        sample_rate_hz: u32,
    ) -> Result<Vec<DiarizeSegment>> {
        let _operation = OPERATION.lock().unwrap_or_else(|e| e.into_inner());
        let run =
            || Diarizer::load().and_then(|mut engine| engine.process(samples, sample_rate_hz));
        // Release the model after each job so batch transcription and local summaries
        // do not leave several idle engines resident on a 16 GB machine.
        run()
            .map(|segments| {
                segments
                    .into_iter()
                    .map(|s| DiarizeSegment {
                        start_ms: s.start_ms,
                        end_ms: s.end_ms,
                        speaker_index: s.speaker_index,
                    })
                    .collect()
            })
            .map_err(|e| Error::Bridge(e.to_string()))
    }
}

#[cfg(not(any(
    target_os = "windows",
    all(target_os = "macos", target_arch = "aarch64")
)))]
mod platform {
    use super::*;

    pub(super) fn model_download_state() -> Result<ModelDownloadState> {
        Err(Error::UnsupportedPlatform)
    }

    pub(super) fn start_model_download() -> Result<()> {
        Err(Error::UnsupportedPlatform)
    }

    pub(super) fn diarize_samples(
        _samples: &[f32],
        _sample_rate_hz: u32,
    ) -> Result<Vec<DiarizeSegment>> {
        Err(Error::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_segments_payload() {
        let payload = r#"{
            "segments": [
                {"startMs": 0, "endMs": 1200, "speakerIndex": 0},
                {"startMs": 1450, "endMs": 3900, "speakerIndex": 1}
            ],
            "error": null
        }"#;

        assert_eq!(
            parse_run_payload(payload).unwrap(),
            vec![
                DiarizeSegment {
                    start_ms: 0,
                    end_ms: 1200,
                    speaker_index: 0,
                },
                DiarizeSegment {
                    start_ms: 1450,
                    end_ms: 3900,
                    speaker_index: 1,
                },
            ]
        );
    }

    #[test]
    fn parses_empty_segments_payload() {
        assert!(
            parse_run_payload(r#"{"segments": [], "error": null}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn surfaces_bridge_errors() {
        let error =
            parse_run_payload(r#"{"segments": [], "error": "model exploded"}"#).unwrap_err();
        assert!(matches!(error, Error::Bridge(message) if message == "model exploded"));
    }

    #[test]
    fn rejects_malformed_payloads() {
        assert!(matches!(
            parse_run_payload("not json"),
            Err(Error::ResponseParse(_))
        ));
    }
}

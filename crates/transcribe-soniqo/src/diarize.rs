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

pub fn diarize_file(path: &std::path::Path, channel: usize) -> Result<Vec<DiarizeSegment>> {
    ensure_supported_platform()?;
    platform::diarize_file(path, channel)
}

fn ensure_supported_platform() -> Result<()> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
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
    swift!(fn _diarize_file(path: &SRString, channel: Int) -> SRString);
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

    pub(super) fn diarize_file(
        path: &std::path::Path,
        channel: usize,
    ) -> Result<Vec<DiarizeSegment>> {
        let path: SRString = path.to_string_lossy().as_ref().into();
        let payload = unsafe { _diarize_file(&path, channel as Int) };
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

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
mod platform {
    use super::*;

    pub(super) fn model_download_state() -> Result<ModelDownloadState> {
        Err(Error::UnsupportedPlatform)
    }

    pub(super) fn start_model_download() -> Result<()> {
        Err(Error::UnsupportedPlatform)
    }

    pub(super) fn diarize_file(
        _path: &std::path::Path,
        _channel: usize,
    ) -> Result<Vec<DiarizeSegment>> {
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

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod native_tests {
    use super::*;
    #[test]
    #[ignore = "requires downloaded speaker detection models"]
    fn file_and_sample_bridge_preserve_whole_recording_speakers() {
        assert!(is_ready());
        let file =
            hypr_audio_utils::PcmFile::prepare(hypr_data::english_1::AUDIO_PATH, || false).unwrap();
        let pcm = &file.descriptor;
        let samples = pcm.reader().unwrap().channel(0, 0..pcm.frames).unwrap();
        let old = diarize_samples(&samples, 16000).unwrap();
        let new = diarize_file(&pcm.path, 0).unwrap();
        assert!(!new.is_empty());
        assert_eq!(new, old);
    }
}

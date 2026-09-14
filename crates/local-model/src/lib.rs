use std::path::{Path, PathBuf};

use hypr_model_downloader::{DownloadableModel, Error};
pub use hypr_transcribe_soniqo::SoniqoModel;
pub use hypr_whisper_local_model::WhisperModel;

#[derive(
    Debug, Clone, Copy, serde::Serialize, serde::Deserialize, specta::Type, Eq, Hash, PartialEq,
)]
pub enum DiarizerModel {
    #[serde(rename = "diarizer-fluid-community")]
    FluidCommunity,
}

impl DiarizerModel {
    pub fn as_str(&self) -> &'static str {
        match self {
            DiarizerModel::FluidCommunity => "diarizer-fluid-community",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            DiarizerModel::FluidCommunity => "Speaker detection",
        }
    }

    pub fn size_bytes(&self) -> u64 {
        match self {
            DiarizerModel::FluidCommunity => 104857600,
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            DiarizerModel::FluidCommunity => "100 MB",
        }
    }
}

impl std::fmt::Display for DiarizerModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub enum LocalModelKind {
    Stt,
    Diarizer,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, Eq, Hash, PartialEq)]
#[serde(untagged)]
pub enum LocalModel {
    Soniqo(SoniqoModel),
    Whisper(WhisperModel),
    Diarizer(DiarizerModel),
}

impl std::fmt::Display for LocalModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LocalModel::Soniqo(model) => write!(f, "{model}"),
            LocalModel::Whisper(model) => write!(f, "whisper-{model}"),
            LocalModel::Diarizer(model) => write!(f, "{model}"),
        }
    }
}

impl LocalModel {
    pub fn all() -> Vec<LocalModel> {
        let mut models = SoniqoModel::all()
            .iter()
            .copied()
            .map(LocalModel::Soniqo)
            .collect::<Vec<_>>();

        models.extend([
            LocalModel::Whisper(WhisperModel::LargeV3),
            LocalModel::Whisper(WhisperModel::QuantizedBase),
            LocalModel::Whisper(WhisperModel::QuantizedBaseEn),
            LocalModel::Whisper(WhisperModel::QuantizedSmall),
            LocalModel::Whisper(WhisperModel::QuantizedSmallEn),
            LocalModel::Whisper(WhisperModel::QuantizedLargeTurbo),
        ]);

        models.push(LocalModel::Diarizer(DiarizerModel::FluidCommunity));

        models
    }

    pub fn kind(&self) -> &'static str {
        match self {
            LocalModel::Soniqo(_) => "stt-soniqo",
            LocalModel::Whisper(_) => "stt-whisper",
            LocalModel::Diarizer(_) => "diarizer",
        }
    }

    pub fn model_kind(&self) -> LocalModelKind {
        match self {
            LocalModel::Soniqo(_) | LocalModel::Whisper(_) => LocalModelKind::Stt,
            LocalModel::Diarizer(_) => LocalModelKind::Diarizer,
        }
    }

    pub fn cli_name(&self) -> &'static str {
        match self {
            LocalModel::Soniqo(model) => model.as_str(),
            LocalModel::Whisper(WhisperModel::LargeV3) => "whisper-large-v3",
            LocalModel::Whisper(WhisperModel::QuantizedTiny) => "whisper-tiny",
            LocalModel::Whisper(WhisperModel::QuantizedTinyEn) => "whisper-tiny-en",
            LocalModel::Whisper(WhisperModel::QuantizedBase) => "whisper-base",
            LocalModel::Whisper(WhisperModel::QuantizedBaseEn) => "whisper-base-en",
            LocalModel::Whisper(WhisperModel::QuantizedSmall) => "whisper-small",
            LocalModel::Whisper(WhisperModel::QuantizedSmallEn) => "whisper-small-en",
            LocalModel::Whisper(WhisperModel::QuantizedLargeTurbo) => "whisper-large-turbo",
            LocalModel::Diarizer(model) => model.as_str(),
        }
    }

    pub fn install_path(&self, models_base: &Path) -> PathBuf {
        match self {
            LocalModel::Soniqo(model) => models_base.join("soniqo").join(model.as_str()),
            LocalModel::Whisper(model) => models_base.join("stt").join(model.file_name()),
            LocalModel::Diarizer(model) => models_base.join("diarizer").join(model.as_str()),
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            LocalModel::Soniqo(model) => model.display_name().to_string(),
            LocalModel::Whisper(model) => model.display_name().to_string(),
            LocalModel::Diarizer(model) => model.display_name().to_string(),
        }
    }

    pub fn description(&self) -> String {
        match self {
            LocalModel::Soniqo(model) => model.description().to_string(),
            LocalModel::Whisper(model) => model.description(),
            LocalModel::Diarizer(model) => model.description().to_string(),
        }
    }

    pub fn is_available_on_current_platform(&self) -> bool {
        let is_apple_silicon = cfg!(target_arch = "aarch64") && cfg!(target_os = "macos");

        match self {
            LocalModel::Soniqo(model) => model.is_available_on_current_platform(),
            LocalModel::Whisper(_) => is_apple_silicon,
            LocalModel::Diarizer(_) => is_apple_silicon,
        }
    }
}

impl DownloadableModel for LocalModel {
    fn download_key(&self) -> String {
        match self {
            LocalModel::Soniqo(model) => format!("soniqo:{}", model.as_str()),
            LocalModel::Whisper(model) => format!("whisper:{}", model.file_name()),
            LocalModel::Diarizer(model) => format!("diarizer:{}", model.as_str()),
        }
    }

    fn download_url(&self) -> Option<String> {
        match self {
            LocalModel::Soniqo(_) | LocalModel::Diarizer(_) => None,
            LocalModel::Whisper(model) => Some(model.model_url().to_string()),
        }
    }

    fn download_checksum(&self) -> Option<u32> {
        match self {
            LocalModel::Soniqo(_) | LocalModel::Diarizer(_) => None,
            LocalModel::Whisper(model) => Some(model.checksum()),
        }
    }

    fn download_destination(&self, models_base: &Path) -> PathBuf {
        match self {
            LocalModel::Soniqo(model) => models_base.join("soniqo").join(model.as_str()),
            LocalModel::Whisper(model) => models_base.join("stt").join(model.file_name()),
            LocalModel::Diarizer(model) => models_base.join("diarizer").join(model.as_str()),
        }
    }

    fn is_downloaded(&self, models_base: &Path) -> Result<bool, Error> {
        match self {
            LocalModel::Soniqo(model) => hypr_transcribe_soniqo::is_model_downloaded(*model)
                .map_err(|e| Error::OperationFailed(e.to_string())),
            LocalModel::Diarizer(_) => Ok(hypr_transcribe_soniqo::diarize::is_ready()),
            LocalModel::Whisper(model) => {
                Ok(models_base.join("stt").join(model.file_name()).exists())
            }
        }
    }

    fn finalize_download(&self, _downloaded_path: &Path, _models_base: &Path) -> Result<(), Error> {
        match self {
            LocalModel::Soniqo(_) => Err(Error::FinalizeFailed(
                "Soniqo models are downloaded through the Soniqo bridge".to_string(),
            )),
            LocalModel::Diarizer(_) => Err(Error::FinalizeFailed(
                "Diarizer models are downloaded through the diarizer bridge".to_string(),
            )),
            LocalModel::Whisper(_) => Ok(()),
        }
    }

    fn delete_downloaded(&self, models_base: &Path) -> Result<(), Error> {
        match self {
            LocalModel::Soniqo(model) => hypr_transcribe_soniqo::delete_model(*model)
                .map_err(|e| Error::DeleteFailed(e.to_string())),
            LocalModel::Diarizer(_) => Err(Error::DeleteFailed(
                "Diarizer models are managed by the diarizer bridge".to_string(),
            )),
            LocalModel::Whisper(model) => {
                let model_path = models_base.join("stt").join(model.file_name());
                if model_path.exists() {
                    std::fs::remove_file(&model_path)
                        .map_err(|e| Error::DeleteFailed(e.to_string()))?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_models_are_absent_and_cannot_deserialize() {
        let catalog = LocalModel::all()
            .iter()
            .map(LocalModel::cli_name)
            .collect::<Vec<_>>();
        for id in [
            "am-parakeet-v2",
            "am-parakeet-v3",
            "am-whisper-large-v3",
            "soniqo-qwen3-small",
            "soniqo-qwen3-large",
            "Llama3p2_3bQ4",
            "Gemma3_4bQ4",
            "HyprLLM",
        ] {
            assert!(!catalog.contains(&id));
            assert!(serde_json::from_value::<LocalModel>(serde_json::json!(id)).is_err());
        }
        for id in ["QuantizedTiny", "QuantizedTinyEn", "soniqo-omnilingual"] {
            assert!(serde_json::from_value::<LocalModel>(serde_json::json!(id)).is_ok());
        }
    }

    #[test]
    fn soniqo_models_reject_generic_download_finalize() {
        let model = LocalModel::Soniqo(SoniqoModel::ParakeetStreaming);

        let error = model
            .finalize_download(Path::new("download"), Path::new("models"))
            .unwrap_err();

        assert!(error.to_string().contains("Soniqo bridge"));
    }
}

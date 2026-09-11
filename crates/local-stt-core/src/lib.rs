pub use hypr_local_model::{AmModel, LocalModel, SoniqoModel, WhisperModel};

pub static SUPPORTED_MODELS: &[LocalModel] = &[
    LocalModel::Soniqo(SoniqoModel::ParakeetStreaming),
    LocalModel::Soniqo(SoniqoModel::ParakeetBatch),
    LocalModel::Whisper(WhisperModel::LargeV3),
    LocalModel::Whisper(WhisperModel::QuantizedLargeTurbo),
    LocalModel::Whisper(WhisperModel::QuantizedSmall),
    LocalModel::Whisper(WhisperModel::QuantizedSmallEn),
    LocalModel::Whisper(WhisperModel::QuantizedBase),
    LocalModel::Whisper(WhisperModel::QuantizedBaseEn),
    LocalModel::Am(AmModel::ParakeetV2),
    LocalModel::Am(AmModel::ParakeetV3),
    LocalModel::Am(AmModel::WhisperLargeV3),
];

#[derive(serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum SttModelType {
    Soniqo,
    Whispercpp,
    Argmax,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct SttModelInfo {
    pub key: LocalModel,
    pub display_name: String,
    pub description: String,
    pub size_bytes: Option<u64>,
    pub model_type: SttModelType,
}

pub fn stt_model_info(model: &LocalModel) -> SttModelInfo {
    match model {
        LocalModel::Soniqo(value) => SttModelInfo {
            key: model.clone(),
            display_name: value.display_name().to_string(),
            description: value.description().to_string(),
            size_bytes: Some(value.size_bytes()),
            model_type: SttModelType::Soniqo,
        },
        LocalModel::Whisper(value) => SttModelInfo {
            key: model.clone(),
            display_name: value.display_name().to_string(),
            description: value.description(),
            size_bytes: Some(value.model_size_bytes()),
            model_type: SttModelType::Whispercpp,
        },
        LocalModel::Am(value) => SttModelInfo {
            key: model.clone(),
            display_name: value.display_name().to_string(),
            description: value.description().to_string(),
            size_bytes: Some(value.model_size_bytes()),
            model_type: SttModelType::Argmax,
        },
        LocalModel::GgufLlm(_) | LocalModel::Diarizer(_) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_large_v3_is_selectable_with_full_model_metadata() {
        let model = LocalModel::Whisper(WhisperModel::LargeV3);
        assert!(SUPPORTED_MODELS.contains(&model));
        let info = stt_model_info(&model);
        assert!(matches!(info.model_type, SttModelType::Whispercpp));
        assert_eq!(info.size_bytes, Some(3095033483));
        assert_eq!(info.display_name, "Whisper Large V3 (Multilingual)");
    }

    #[test]
    fn supported_models_include_soniqo_models_from_rust_source_of_truth() {
        let supported_soniqo_models = SUPPORTED_MODELS
            .iter()
            .filter_map(|model| match model {
                LocalModel::Soniqo(value) => Some(*value),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(supported_soniqo_models, SoniqoModel::selectable());
    }

    #[test]
    fn soniqo_model_info_comes_from_soniqo_metadata() {
        for model in SoniqoModel::all() {
            let info = stt_model_info(&LocalModel::Soniqo(*model));

            assert_eq!(info.key, LocalModel::Soniqo(*model));
            assert_eq!(info.display_name, model.display_name());
            assert_eq!(info.description, model.description());
            assert_eq!(info.size_bytes, Some(model.size_bytes()));
            assert!(matches!(info.model_type, SttModelType::Soniqo));
        }
    }
}

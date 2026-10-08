pub use hypr_local_model::{LocalModel, SoniqoModel, WhisperModel};

#[cfg(not(target_os = "windows"))]
pub static SUPPORTED_MODELS: &[LocalModel] = &[
    LocalModel::Soniqo(SoniqoModel::ParakeetStreaming),
    LocalModel::Soniqo(SoniqoModel::ParakeetBatch),
    LocalModel::Whisper(WhisperModel::LargeV3),
    LocalModel::Whisper(WhisperModel::QuantizedLargeTurbo),
    LocalModel::Whisper(WhisperModel::QuantizedSmall),
    LocalModel::Whisper(WhisperModel::QuantizedSmallEn),
    LocalModel::Whisper(WhisperModel::QuantizedBase),
    LocalModel::Whisper(WhisperModel::QuantizedBaseEn),
];

#[cfg(target_os = "windows")]
pub static SUPPORTED_MODELS: &[LocalModel] = &[
    LocalModel::Soniqo(SoniqoModel::OnnxParakeetStreaming),
    LocalModel::Soniqo(SoniqoModel::OnnxParakeetBatch),
    LocalModel::Whisper(WhisperModel::LargeV3),
    LocalModel::Whisper(WhisperModel::QuantizedLargeTurbo),
    LocalModel::Whisper(WhisperModel::QuantizedSmall),
    LocalModel::Whisper(WhisperModel::QuantizedSmallEn),
    LocalModel::Whisper(WhisperModel::QuantizedBase),
    LocalModel::Whisper(WhisperModel::QuantizedBaseEn),
];

#[derive(serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum SttModelType {
    Soniqo,
    Onnx,
    Whispercpp,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct SttModelInfo {
    pub key: LocalModel,
    pub display_name: String,
    pub description: String,
    pub size_bytes: Option<u64>,
    pub model_type: SttModelType,
    pub supported_languages: Option<Vec<String>>,
}

pub fn stt_model_info(model: &LocalModel) -> SttModelInfo {
    match model {
        LocalModel::Soniqo(value) => SttModelInfo {
            key: model.clone(),
            display_name: value.display_name().to_string(),
            description: value.description().to_string(),
            size_bytes: Some(value.size_bytes()),
            model_type: if matches!(
                value,
                SoniqoModel::OnnxParakeetStreaming | SoniqoModel::OnnxParakeetBatch
            ) {
                SttModelType::Onnx
            } else {
                SttModelType::Soniqo
            },
            supported_languages: value.supported_language_codes(),
        },
        LocalModel::Whisper(value) => SttModelInfo {
            key: model.clone(),
            display_name: value.display_name().to_string(),
            description: value.description(),
            size_bytes: Some(value.model_size_bytes()),
            model_type: SttModelType::Whispercpp,
            supported_languages: Some(
                value
                    .supported_languages()
                    .iter()
                    .map(|language| language.iso639_code().to_string())
                    .collect(),
            ),
        },
        LocalModel::Diarizer(_) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_catalog_has_six_models_with_consistent_language_metadata() {
        let models = SUPPORTED_MODELS
            .iter()
            .filter_map(|model| match model {
                LocalModel::Whisper(value) => Some((model, value)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(models.len(), 6);
        for (model, value) in models {
            let info = stt_model_info(model);
            assert!(matches!(info.model_type, SttModelType::Whispercpp));
            assert_eq!(info.key, *model);
            assert_eq!(info.size_bytes, Some(value.model_size_bytes()));
            let languages = info.supported_languages.unwrap();
            if matches!(
                value,
                WhisperModel::QuantizedBaseEn | WhisperModel::QuantizedSmallEn
            ) {
                assert_eq!(languages, ["en"]);
            } else {
                assert!(languages.contains(&"en".to_string()));
                assert!(languages.contains(&"nl".to_string()));
            }
        }
    }

    #[test]
    fn whisper_large_v3_has_platform_specific_availability_and_full_metadata() {
        let model = LocalModel::Whisper(WhisperModel::LargeV3);
        assert!(SUPPORTED_MODELS.contains(&model));
        assert_eq!(
            model.is_available_on_current_platform(),
            cfg!(target_os = "windows") || cfg!(all(target_os = "macos", target_arch = "aarch64"))
        );
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
    fn language_metadata_distinguishes_streaming_and_batch_parakeet() {
        let streaming = stt_model_info(&LocalModel::Soniqo(SoniqoModel::ParakeetStreaming));
        assert_eq!(streaming.supported_languages.unwrap(), vec!["en"]);
        let batch = stt_model_info(&LocalModel::Soniqo(SoniqoModel::ParakeetBatch));
        let languages = batch.supported_languages.unwrap();
        assert_eq!(languages.len(), 25);
        assert!(languages.contains(&"nl".to_string()));
    }

    #[test]
    fn soniqo_model_info_comes_from_soniqo_metadata() {
        for model in SoniqoModel::all() {
            let info = stt_model_info(&LocalModel::Soniqo(*model));

            assert_eq!(info.key, LocalModel::Soniqo(*model));
            assert_eq!(info.display_name, model.display_name());
            assert_eq!(info.description, model.description());
            assert_eq!(info.size_bytes, Some(model.size_bytes()));
            assert!(matches!(
                info.model_type,
                SttModelType::Soniqo | SttModelType::Onnx
            ));
            assert_eq!(info.supported_languages, model.supported_language_codes());
        }
    }
}

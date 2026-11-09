use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionAvailability {
    Disabled,
    Loading,
    Ready,
    InUse,
    Unavailable,
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionPolicy {
    /// Operator-facing public model alias, never a model file path.
    pub model: Option<String>,
    /// Advisory model state; operation admission remains authoritative.
    pub availability: TranscriptionAvailability,
    pub batch_enabled: bool,
    pub live_enabled: bool,
}
impl Default for TranscriptionPolicy {
    fn default() -> Self {
        Self {
            model: None,
            availability: TranscriptionAvailability::Disabled,
            batch_enabled: false,
            live_enabled: false,
        }
    }
}
pub fn validate_transcription_policy(value: &TranscriptionPolicy) -> Result<(), &'static str> {
    if value.model.as_deref().is_some_and(|model| {
        model.is_empty()
            || model.len() > 512
            || model.trim() != model
            || model.chars().any(char::is_control)
    }) || (value.availability == TranscriptionAvailability::Disabled
        && (value.model.is_some() || value.batch_enabled || value.live_enabled))
        || (value.availability != TranscriptionAvailability::Disabled && value.model.is_none())
    {
        return Err("Invalid transcription policy");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_is_bounded_public_metadata_and_disabled_is_explicit() {
        assert!(validate_transcription_policy(&TranscriptionPolicy::default()).is_ok());
        let mut value = TranscriptionPolicy {
            model: Some("operator-cpu-base".into()),
            availability: TranscriptionAvailability::Loading,
            batch_enabled: true,
            live_enabled: true,
        };
        assert!(validate_transcription_policy(&value).is_ok());
        value.availability = TranscriptionAvailability::InUse;
        assert!(validate_transcription_policy(&value).is_ok());
        for model in [
            "".into(),
            "x".repeat(513),
            " alias".into(),
            "private\npath".into(),
        ] {
            value.model = Some(model);
            assert!(validate_transcription_policy(&value).is_err());
        }
        let mut wire = serde_json::to_value(TranscriptionPolicy::default()).unwrap();
        wire["model_path"] = serde_json::json!("/private/model.bin");
        assert!(serde_json::from_value::<TranscriptionPolicy>(wire).is_err());
        value.model = None;
        assert!(validate_transcription_policy(&value).is_err());
    }
}

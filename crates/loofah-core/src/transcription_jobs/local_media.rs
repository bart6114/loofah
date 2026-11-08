use super::local::PreparedLocalInference;
use super::*;
use crate::{
    local_capture_intents::LocalTranscriptionOptions, local_cli::LocalCliArtifact,
    settings::AppConfig,
};
use std::path::PathBuf;

impl VaultContext {
    /// The Local media journal owns replacement authority. This only runs its
    /// already declared input through the configured device engine; it never
    /// submits an absent-transcript job or publishes canonical content.
    #[allow(clippy::too_many_arguments)]
    pub async fn transcribe_local_media_artifact(
        self: &Arc<Self>,
        request_id: String,
        source: LocalCliArtifact,
        filename: String,
        session_id: String,
        audio: SessionAudio,
        config: AppConfig,
        temporary: PathBuf,
        cancel: CancellationToken,
    ) -> Result<TranscriptWithData, ApplicationError> {
        let context = self.clone();
        tokio::spawn(async move {
            if !uuid::Uuid::parse_str(&request_id)
                .is_ok_and(|id| !id.is_nil() && id.to_string() == request_id)
                || !is_transcription_sha256(&source.sha256)
            {
                return Err(error(
                    ErrorCode::Validation,
                    "Invalid original Local media request",
                ));
            }
            validate_session_id(&session_id)
                .map_err(|_| error(ErrorCode::Validation, "Invalid Local media session"))?;
            if config.current_stt_provider.as_deref() != Some("fmtr") {
                return Err(error(
                    ErrorCode::Validation,
                    "Select an on-device transcription model in Settings",
                ));
            }
            let options = LocalTranscriptionOptions {
                model: config
                    .current_stt_model
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        error(
                            ErrorCode::Validation,
                            "Select an on-device transcription model in Settings",
                        )
                    })?,
                languages: config.meeting_languages.unwrap_or_default(),
                keywords: vec![],
            };
            let request = TranscriptionJobRequest {
                session_id: session_id.clone(),
                expected_recording_sha256: source.sha256.clone(),
                expected_transcript: AbsentTranscriptExpectation::Absent,
                model: options.model.clone(),
                languages: options.languages.clone(),
                keywords: options.keywords.clone(),
            };
            let prepared = context
                .transcription_service()?
                .engine
                .prepare(&request, cancel.clone())
                .await
                .map_err(|failure| {
                    error(
                        ErrorCode::Unavailable,
                        &format!("Local transcription could not be prepared: {failure:?}"),
                    )
                })?;
            let sha = source.sha256.clone();
            let pin = context.clone();
            let materialized = tokio::task::spawn_blocking(move || {
                let _pin = pin;
                source.materialize(&filename, temporary)
            })
            .await
            .map_err(|_| {
                error(
                    ErrorCode::Unavailable,
                    "Original Local media input could not be retained",
                )
            })??;
            let input = LocalInferenceInput {
                request_id,
                session_id,
                options,
                source_sha256: sha,
                path: materialized.path().to_owned(),
                audio,
                owner: Box::new(materialized),
            };
            context
                .transcribe_local_capture_source(
                    input,
                    PreparedLocalInference { prepared, request },
                    cancel,
                )
                .await
        })
        .await
        .map_err(|_| {
            error(
                ErrorCode::Unavailable,
                "Local media inference requires recovery",
            )
        })?
    }
}

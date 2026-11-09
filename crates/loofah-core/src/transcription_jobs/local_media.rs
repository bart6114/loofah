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
    ) -> Result<crate::local_cli::LocalMediaTranscription, ApplicationError> {
        if config.current_stt_provider.as_deref() != Some("fmtr") {
            return Err(error(
                ErrorCode::Validation,
                "Select an on-device transcription model in Settings",
            ));
        }
        self.transcribe_scoped_media_artifact(
            crate::local_capture_intents::TRANSCRIPTION_OWNER.into(),
            request_id,
            source,
            filename,
            session_id,
            audio,
            config,
            temporary,
            cancel,
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn transcribe_scoped_media_artifact(
        self: &Arc<Self>,
        principal: String,
        request_id: String,
        source: LocalCliArtifact,
        filename: String,
        session_id: String,
        audio: SessionAudio,
        config: AppConfig,
        temporary: PathBuf,
        cancel: CancellationToken,
    ) -> Result<crate::local_cli::LocalMediaTranscription, ApplicationError> {
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
            let path = materialized.path().to_owned();
            // Physical source release precedes result validation. Keep the immutable
            // directory and original descriptor available through that final hash check.
            let materialized = Arc::new(std::sync::Mutex::new(materialized));
            let input = LocalInferenceInput {
                request_id,
                session_id,
                options,
                source_sha256: sha,
                path,
                audio,
                owner: Box::new(materialized.clone()),
            };
            let result = context
                .complete_media_inference(
                    input,
                    PreparedLocalInference { prepared, request },
                    cancel,
                    principal,
                )
                .await;
            drop(materialized);
            result
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Release(Arc<AtomicBool>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    struct Engine {
        denied: Arc<AtomicBool>,
        cancel_after_result: bool,
        source_released: Arc<AtomicBool>,
    }
    struct Prepared {
        denied: Arc<AtomicBool>,
        cancel_after_result: bool,
        source_released: Arc<AtomicBool>,
    }
    impl TranscriptionEngine for Engine {
        fn prepare<'a>(
            &'a self,
            _: &'a TranscriptionJobRequest,
            _: CancellationToken,
        ) -> TranscriptionFuture<'a, Result<Box<dyn PreparedTranscription>, TranscriptionFailure>>
        {
            Box::pin(async move {
                Ok(Box::new(Prepared {
                    denied: self.denied.clone(),
                    cancel_after_result: self.cancel_after_result,
                    source_released: self.source_released.clone(),
                }) as Box<dyn PreparedTranscription>)
            })
        }
    }
    impl PreparedTranscription for Prepared {
        fn transcribe(
            self: Box<Self>,
            input: TranscriptionInput,
            cancel: CancellationToken,
        ) -> TranscriptionFuture<'static, Result<TranscriptWithData, TranscriptionFailure>>
        {
            Box::pin(async move {
                assert_eq!(
                    std::fs::read(input.source.path()).unwrap(),
                    b"original immutable media"
                );
                assert!(!self.source_released.load(Ordering::Acquire));
                let path = input.source.path().to_owned();
                let mut transcript = input.metadata.empty_transcript();
                transcript.words.push(hypr_vault_read::TranscriptWord {
                    id: Some(uuid::Uuid::new_v4().to_string()),
                    text: "Retained".into(),
                    start_ms: 0.0,
                    end_ms: 1.0,
                    channel: 2.0,
                    speaker: None,
                    metadata: None,
                });
                if self.cancel_after_result {
                    cancel.cancel();
                } else {
                    self.denied.store(true, Ordering::Release);
                }
                drop(input);
                // Native release must not delete the immutable source before Core's final hash.
                assert!(path.exists());
                assert!(!self.source_released.load(Ordering::Acquire));
                Ok(transcript)
            })
        }
    }
    struct Auth(Arc<AtomicBool>);
    impl TranscriptionAuthorizer for Auth {
        fn observe<'a>(
            &'a self,
            _: &'a TranscriptionWork,
        ) -> TranscriptionFuture<'a, Result<TranscriptionAccess, TranscriptionFailure>> {
            Box::pin(async {
                Ok(TranscriptionAccess {
                    cancelled: CancellationToken::new(),
                    interrupted: CancellationToken::new(),
                    owner: Box::new(()),
                })
            })
        }
        fn authorize<'a>(
            &'a self,
            _: &'a TranscriptionWork,
        ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>> {
            Box::pin(async move {
                if self.0.load(Ordering::Acquire) {
                    Err(TranscriptionFailure::AccessDenied)
                } else {
                    Ok(Box::new(()) as Box<dyn Send>)
                }
            })
        }
    }
    #[tokio::test]
    async fn completed_manual_output_survives_cancel_or_revocation_and_keeps_source_through_final_validation()
     {
        for cancel_after_result in [true, false] {
            let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
            let vault = root.path().join("vault");
            std::fs::create_dir(&vault).unwrap();
            let hypr_storage::vault::LeaseAttempt::Acquired(lease) =
                hypr_storage::vault::VaultLease::try_acquire_existing(&vault).unwrap()
            else {
                panic!("owned fixture")
            };
            let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
            let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Original","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
            store.create_note_session(&meta, "Notes").await.unwrap();
            let context = Arc::new(
                VaultContext::new(
                    "original".into(),
                    store,
                    crate::startup::StartupState::new(Some(&vault)),
                    Arc::new(crate::settings::ConfigState::load_or_default(&vault)),
                    Arc::new(crate::search::SearchService::default()),
                )
                .with_operation_storage(root.path().join("operations"))
                .with_vault_lease(lease)
                .unwrap(),
            );
            let released = Arc::new(AtomicBool::new(false));
            let denied = Arc::new(AtomicBool::new(false));
            let worker = context
                .start_transcription(
                    Arc::new(Engine {
                        denied: denied.clone(),
                        cancel_after_result,
                        source_released: released.clone(),
                    }),
                    Arc::new(Auth(denied)),
                    Default::default(),
                )
                .await
                .unwrap();
            let input = root.path().join("input.mp3");
            std::fs::write(&input, b"original immutable media").unwrap();
            let file = std::fs::File::open(&input).unwrap();
            let metadata =
                hypr_fs_sync_core::media::metadata_from_reader("input.mp3", "audio/mpeg", &file)
                    .unwrap();
            let artifact = LocalCliArtifact::new(
                file,
                metadata.size_bytes,
                metadata.sha256,
                None,
                Box::new(Release(released.clone())),
            );
            let completed = context
                .transcribe_local_media_artifact(
                    uuid::Uuid::new_v4().to_string(),
                    artifact,
                    "input.mp3".into(),
                    "note".into(),
                    Default::default(),
                    AppConfig {
                        current_stt_provider: Some("fmtr".into()),
                        current_stt_model: Some("frozen".into()),
                        ..Default::default()
                    },
                    root.path().join("temporary"),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            assert_eq!(completed.transcript.words[0].text, "Retained");
            assert!(completed.publication_error.is_some());
            assert!(released.load(Ordering::Acquire));
            assert!(!vault.join("sessions/note/transcript.json").exists());
            worker.shutdown().await.unwrap();
        }
    }
}

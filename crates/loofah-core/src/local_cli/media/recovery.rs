use super::*;

impl LocalCli {
    pub(in crate::local_cli) async fn recover_media(
        &self,
        request_id: String,
    ) -> Result<LocalCliOutput, ApplicationError> {
        let parsed = uuid::Uuid::parse_str(&request_id)
            .ok()
            .filter(|id| !id.is_nil() && id.to_string() == request_id)
            .ok_or_else(|| {
                error(
                    ErrorCode::Validation,
                    "A canonical original request UUID is required",
                )
            })?;
        let result = self.recover_media_inner(&request_id, parsed).await;
        result
            .map(|mut reply| {
                if let LocalCliReply::Imported {
                    transcription: Some(Err(cause)),
                    ..
                } = &mut reply
                {
                    super::super::identify_media_error(cause, &request_id);
                }
                LocalCliOutput {
                    reply,
                    artifact: None,
                }
            })
            .map_err(|mut cause| {
                super::super::identify_media_error(&mut cause, &request_id);
                cause
            })
    }

    async fn recover_media_inner(
        &self,
        request_id: &str,
        parsed: uuid::Uuid,
    ) -> Result<LocalCliReply, ApplicationError> {
        let _request =
            self.context
                .admit_session_lifecycle_request("local-media", "local-cli", parsed)?;
        let root = &self
            .context
            .operations
            .as_ref()
            .ok_or_else(|| {
                error(
                    ErrorCode::Unavailable,
                    "Local operation storage is unavailable",
                )
            })?
            .root;
        let claim =
            journal::Claim::recover(self.context.clone(), root.join("local-media"), request_id)
                .await?;
        if let Some(result) = claim.result() {
            let unresolved = match result {
                Err(cause) => cause.mutation_outcome == Some(MutationOutcome::Unknown),
                Ok(LocalCliReply::Imported {
                    transcription: Some(Err(cause)),
                    ..
                }) => cause.mutation_outcome == Some(MutationOutcome::Unknown),
                _ => false,
            };
            if !unresolved {
                return result.clone();
            }
        }
        let result = match claim.operation() {
            Some(
                LocalCliOperation::Transcribe { session_id }
                | LocalCliOperation::TranscribeChecked { session_id, .. },
            ) => {
                let _admission = self.context.admit_session_work(&session_id).await?;
                let outcome = self
                    .recover_transcription(request_id, root, &session_id, &claim)
                    .await?;
                Ok(LocalCliReply::Transcribed {
                    session_id,
                    transcript_id: outcome.transcript_id,
                    word_count: outcome.words,
                })
            }
            Some(LocalCliOperation::Import {
                into, transcribe, ..
            }) => {
                let session_id = match into {
                    Some(id) => id,
                    None => self
                        .context
                        .created_session_id("local-cli-import", request_id)?,
                };
                let _admission = self.context.admit_session_work(&session_id).await?;
                let import = claim.import().ok_or_else(|| unknown("No normalized import was durably bound; inspect the original session before an explicit new import"))?;
                let request = &import.publication;
                if request.request_id != parsed
                    || request.session_id != session_id
                    || import.meta.id != session_id
                    || request.namespace != "local-cli-import"
                    || request.vault_id != self.context.events.vault_id()
                    || request.expected != RecordingExpectation::AbsentImport
                {
                    return Err(unknown(
                        "Retained import binding does not match its original request",
                    ));
                }
                let operations = root.join("recording-publications");
                let completed = self
                    .context
                    .store
                    .recording_publication_completed(&operations, request.clone())
                    .await
                    .map_err(|cause| storage_error(cause, false))?;
                if !completed {
                    let mut ticket = self
                        .context
                        .recording_publications
                        .begin(&session_id, request_id)?
                        .with_events(self.context.events.clone());
                    let plan = self.context.store.prepare_recording_publication_status(&operations, request.clone()).await
                        .map_err(|cause| storage_error(cause, false))?
                        .ok_or_else(|| unknown("No durable normalized import is available; the original source will not be read again"))?;
                    ticket.committing();
                    match plan
                        .commit()
                        .await
                        .map_err(|cause| storage_error(cause, true))?
                    {
                        RecordingPublication::Published { .. } => ticket.resolved(),
                        RecordingPublication::Changed
                        | RecordingPublication::Busy
                        | RecordingPublication::Uncertain => {
                            return Err(unknown(
                                "Retained import cannot be published against the current session; original bytes remain retained",
                            ));
                        }
                    }
                }
                let transcription = if transcribe {
                    Some(
                        self.recover_transcription(request_id, root, &session_id, &claim)
                            .await,
                    )
                } else {
                    None
                };
                Ok(LocalCliReply::Imported {
                    meta: import.meta,
                    audio_path: self
                        .context
                        .store
                        .vault_base()
                        .join("sessions")
                        .join(session_id)
                        .join("audio.mp3"),
                    transcription,
                })
            }
            _ => Err(unknown(
                "Original media declaration is unavailable; this receipt requires review",
            )),
        };
        claim.finish(result.clone()).await?;
        result
    }

    async fn recover_transcription(
        &self,
        request_id: &str,
        root: &std::path::Path,
        session_id: &str,
        claim: &journal::Claim,
    ) -> Result<LocalTranscriptionOutcome, ApplicationError> {
        let outcome = claim.transcription().ok_or_else(|| {
            unknown("No generated transcript was durably bound; transcription will not be repeated")
        })?;
        let plan = self.context.store.prepare_transcript_publication_status(
            &root.join("transcript-publications"),
            TranscriptPublicationIdentity {
                vault_id: self.context.events.vault_id(), namespace: "local-cli-transcribe".into(),
                request_id: uuid::Uuid::parse_str(request_id).unwrap(),
            },
        ).await.map_err(|cause| storage_error(cause, false))?
            .ok_or_else(|| unknown("Generated transcript publication was not durably prepared; transcription will not be repeated"))?;
        if plan.result_identity() != (session_id, outcome.transcript_id.as_str()) {
            return Err(unknown(
                "Retained transcript does not match its original request",
            ));
        }
        match plan
            .commit()
            .await
            .map_err(|cause| storage_error(cause, true))?
        {
            TranscriptPublicationOutcome::Published { .. }
            | TranscriptPublicationOutcome::AlreadyCommitted { .. } => Ok(outcome),
            TranscriptPublicationOutcome::Empty => Err(error(
                ErrorCode::Validation,
                "No speech was detected in the retained transcript",
            )),
            TranscriptPublicationOutcome::Conflict
            | TranscriptPublicationOutcome::Busy
            | TranscriptPublicationOutcome::RequiresReview => Err(unknown(
                "Recording or transcript changed; generated transcript is retained without overwriting current content",
            )),
        }
    }
}

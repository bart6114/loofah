use super::*;
impl LocalCli {
    pub async fn media_status(&self, request: &str) -> Result<LocalMediaStatus, ApplicationError> {
        if !uuid::Uuid::parse_str(request).is_ok_and(|id| !id.is_nil() && id.to_string() == request)
        {
            return Err(error(
                ErrorCode::Validation,
                "Invalid original media request UUID",
            ));
        }
        let root = self
            .context
            .operations
            .as_ref()
            .ok_or_else(|| error(ErrorCode::Unavailable, "Local media state is unavailable"))?
            .root
            .join("local-media");
        let claim = journal::Claim::recover(self.context.clone(), root, request).await?;
        let (kind, session) = match claim.operation() {
            Some(LocalCliOperation::Import { into, .. }) => (
                LocalMediaKind::ImportAudio,
                match into {
                    Some(session) => session,
                    None => self
                        .context
                        .created_session_id("local-cli-import", request)?,
                },
            ),
            Some(
                LocalCliOperation::Transcribe { session_id }
                | LocalCliOperation::TranscribeChecked { session_id, .. },
            ) => (LocalMediaKind::Transcribe, session_id),
            _ => {
                return Err(error(
                    ErrorCode::Conflict,
                    "Original media declaration is unavailable",
                ));
            }
        };
        let mut status = LocalMediaStatus {
            request_id: request.into(),
            session_id: session,
            kind,
            state: LocalMediaState::Review,
            transcript_id: None,
            word_count: None,
            error: None,
        };
        match claim.result() {
            Some(Ok(LocalCliReply::Transcribed {
                transcript_id,
                word_count,
                ..
            })) => {
                status.state = LocalMediaState::Succeeded;
                status.transcript_id = Some(transcript_id.clone());
                status.word_count = Some(*word_count);
            }
            Some(Ok(LocalCliReply::Imported { transcription, .. })) => {
                status.state = LocalMediaState::Succeeded;
                match transcription {
                    Some(Ok(result)) => {
                        status.transcript_id = Some(result.transcript_id.clone());
                        status.word_count = Some(result.words);
                    }
                    Some(Err(cause)) => apply_error(&mut status, cause),
                    None => {}
                }
            }
            Some(Err(cause)) => apply_error(&mut status, cause),
            None => {}
            _ => {
                return Err(error(
                    ErrorCode::Conflict,
                    "Original media result is unavailable",
                ));
            }
        }
        Ok(status)
    }
    pub async fn media_statuses(
        &self,
        session: Option<&str>,
    ) -> Result<Vec<LocalMediaStatus>, ApplicationError> {
        if let Some(session) = session {
            validate_session_id(session)
                .map_err(|_| error(ErrorCode::Validation, "Invalid media session"))?;
        }
        let root = self
            .context
            .operations
            .as_ref()
            .ok_or_else(|| error(ErrorCode::Unavailable, "Local media state is unavailable"))?
            .root
            .join("local-media");
        let pin = self.context.clone();
        let ids = tokio::task::spawn_blocking(move || {
            let _pin = pin;
            let entries = match std::fs::read_dir(root) {
                Ok(entries) => entries,
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(_) => {
                    return Err(error(
                        ErrorCode::Unavailable,
                        "Local media inventory is unavailable",
                    ));
                }
            };
            let mut ids = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|_| {
                    error(
                        ErrorCode::Unavailable,
                        "Local media inventory is unavailable",
                    )
                })?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let Some(id) = name.strip_suffix(".json") else {
                    continue;
                };
                if !uuid::Uuid::parse_str(id)
                    .is_ok_and(|value| !value.is_nil() && value.to_string() == id)
                {
                    continue;
                }
                if ids.len() >= 4096 {
                    return Err(error(
                        ErrorCode::Unavailable,
                        "Local media inventory exceeds its read budget",
                    ));
                }
                ids.push(id.to_owned());
            }
            ids.sort();
            Ok(ids)
        })
        .await
        .map_err(|_| {
            error(
                ErrorCode::Unavailable,
                "Local media inventory worker stopped",
            )
        })??;
        let mut statuses = Vec::new();
        for id in ids {
            let status = self.media_status(&id).await?;
            if session.is_none_or(|session| session == status.session_id) {
                statuses.push(status);
            }
        }
        Ok(statuses)
    }
}
fn apply_error(status: &mut LocalMediaStatus, cause: &ApplicationError) {
    status.state = if cause.mutation_outcome == Some(MutationOutcome::Unknown) {
        LocalMediaState::Review
    } else if cause.code == ErrorCode::Cancelled {
        LocalMediaState::Cancelled
    } else {
        LocalMediaState::Failed
    };
    status.error = Some(cause.clone());
}

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
            Some(LocalCliOperation::ImportIntoChecked { session_id, .. }) => {
                (LocalMediaKind::ImportAudio, session_id)
            }
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

impl LocalCli {
    pub(crate) async fn media_origin(
        &self,
        request: &str,
    ) -> Result<(LocalMediaKind, String, String), ApplicationError> {
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
        if !matches!(claim.result(), Some(Ok(_))) {
            return Err(error(
                ErrorCode::Conflict,
                "Original media operation has not confirmed publication",
            ));
        }
        match claim.operation() {
            Some(LocalCliOperation::Import { .. }) => {
                let original = claim.import().ok_or_else(|| {
                    error(
                        ErrorCode::Conflict,
                        "Original audio import identity is unavailable",
                    )
                })?;
                Ok((
                    LocalMediaKind::ImportAudio,
                    original.meta.id,
                    original.meta.created_at,
                ))
            }
            Some(LocalCliOperation::ImportIntoChecked {
                session_id,
                created_at,
                ..
            }) => {
                let original = claim.import().ok_or_else(|| {
                    error(
                        ErrorCode::Conflict,
                        "Original audio import identity is unavailable",
                    )
                })?;
                if session_id != original.meta.id {
                    return Err(error(
                        ErrorCode::Conflict,
                        "Original audio import identity changed",
                    ));
                }
                Ok((
                    LocalMediaKind::ImportAudio,
                    session_id,
                    created_at.unwrap_or(original.meta.created_at),
                ))
            }
            Some(LocalCliOperation::TranscribeChecked {
                session_id,
                expected,
            }) if session_id == expected.session_id => Ok((
                LocalMediaKind::Transcribe,
                session_id,
                expected.session_created_at,
            )),
            _ => Err(error(
                ErrorCode::Conflict,
                "Original manual media source identity is unavailable",
            )),
        }
    }
}

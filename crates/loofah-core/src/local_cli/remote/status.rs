use super::*;

impl MediaDispatcher {
    pub(super) async fn admission_status(
        &self,
        record: Admission,
    ) -> Result<MediaAdmission, ApplicationError> {
        let status = self.status_record(record.clone()).await?;
        Ok(MediaAdmission {
            declaration: record.declaration,
            revision: record.revision,
            status,
        })
    }
    async fn status_record(
        &self,
        record: Admission,
    ) -> Result<MediaRequestStatus, ApplicationError> {
        let cli = self.cli(&record.declaration, self.stop.child_token())?;
        let request = &record.declaration.request_id;
        let subtitle = matches!(
            record.declaration.operation,
            MediaOperation::ImportSubtitles { .. }
        );
        let running = self
            .tasks
            .lock()
            .await
            .get(request)
            .is_some_and(|task| !task.join.is_finished());
        let result = if subtitle {
            cli.subtitle_status(request).await
        } else {
            cli.media_status(request).await
        };
        let mut media = match result {
            Some(Ok(status)) => status,
            None
            | Some(Err(ApplicationError {
                code: ErrorCode::NotFound,
                ..
            })) => LocalMediaStatus {
                request_id: request.clone(),
                session_id: match record.declaration.operation.session_id() {
                    Some(id) => id.into(),
                    None => self
                        .context
                        .created_session_id(&cli.media_namespace("local-cli-import"), request)?,
                },
                kind: match record.declaration.operation {
                    MediaOperation::ImportAudio { .. } => LocalMediaKind::ImportAudio,
                    MediaOperation::Transcribe { .. } => LocalMediaKind::Transcribe,
                    MediaOperation::ImportSubtitles { .. } => LocalMediaKind::ImportSubtitles,
                },
                state: LocalMediaState::Review,
                transcript_id: None,
                word_count: None,
                error: None,
            },
            Some(Err(cause)) => return Err(cause),
        };
        let phase = if running {
            media.state = LocalMediaState::Running;
            MediaPhase::Running
        } else if record.phase == MediaPhase::Cancelled {
            media.state = LocalMediaState::Cancelled;
            MediaPhase::Cancelled
        } else {
            match media.state {
                LocalMediaState::Succeeded => MediaPhase::Succeeded,
                LocalMediaState::Failed => MediaPhase::Failed,
                LocalMediaState::Cancelled => MediaPhase::Cancelled,
                _ if matches!(record.phase, MediaPhase::Receiving | MediaPhase::Ready) => {
                    record.phase
                }
                _ => MediaPhase::Review,
            }
        };
        Ok(MediaRequestStatus {
            declaration: record.declaration,
            revision: record.revision,
            phase,
            media: Some(media),
        })
    }
    pub async fn status(&self, request: &str) -> Result<MediaRequestStatus, ApplicationError> {
        let _authorization = self.authorization.authorize().await?;
        let record = self
            .load(request)
            .await?
            .ok_or_else(|| error(ErrorCode::NotFound, "Media request not found"))?;
        self.status_record(record).await
    }
    pub async fn list(
        &self,
        session: Option<&str>,
    ) -> Result<Vec<MediaRequestStatus>, ApplicationError> {
        let _authorization = self.authorization.authorize().await?;
        if let Some(session) = session {
            validate_session_id(session)
                .map_err(|_| error(ErrorCode::Validation, "Invalid media session"))?;
        }
        let mut result = Vec::new();
        for record in self.inventory().await? {
            let status = self.status_record(record).await?;
            if session.is_none_or(|session| {
                status
                    .media
                    .as_ref()
                    .is_some_and(|media| media.session_id == session)
            }) {
                result.push(status);
            }
        }
        Ok(result)
    }
    pub async fn result(
        &self,
        request: &str,
    ) -> Result<Option<TranscriptWithData>, ApplicationError> {
        let _authorization = self.authorization.authorize().await?;
        let record = self
            .load(request)
            .await?
            .ok_or_else(|| error(ErrorCode::NotFound, "Media request not found"))?;
        let cli = self.cli(&record.declaration, self.stop.child_token())?;
        let result = if matches!(
            record.declaration.operation,
            MediaOperation::ImportSubtitles { .. }
        ) {
            cli.subtitle_result(request).await
        } else {
            cli.media_result(request).await
        };
        match result {
            Err(cause) if cause.code == ErrorCode::NotFound => Ok(None),
            result => result,
        }
    }
    pub async fn cancel(&self, request: &str) -> Result<MediaRequestStatus, ApplicationError> {
        drop(self.authorization.authorize().await?);
        if let Some((stop, _)) = self.receivers.lock().await.get(request) {
            stop.cancel();
        }
        if let Some(task) = self.tasks.lock().await.get(request) {
            task.cancel.cancel();
        }
        let _gate = self.request_gate(request).await?;
        let mut record = self
            .load(request)
            .await?
            .ok_or_else(|| error(ErrorCode::NotFound, "Media request not found"))?;
        if matches!(record.phase, MediaPhase::Receiving | MediaPhase::Ready) {
            record.phase = MediaPhase::Cancelled;
            self.save(record, false).await?;
        }
        self.status(request).await
    }
    pub async fn recover(
        self: &Arc<Self>,
        request: &str,
    ) -> Result<MediaRequestStatus, ApplicationError> {
        let _gate = self.request_gate(request).await?;
        drop(self.authorization.authorize().await?);
        let mut record = self
            .load(request)
            .await?
            .ok_or_else(|| error(ErrorCode::NotFound, "Media request not found"))?;
        if self
            .tasks
            .lock()
            .await
            .get(request)
            .is_some_and(|task| !task.join.is_finished())
        {
            return self.status(request).await;
        }
        if record.phase == MediaPhase::Receiving {
            let directory = self.root.join(format!("{request}.source"));
            let exists = tokio::task::spawn_blocking(move || {
                Ok::<_, ApplicationError>(
                    ledger::existing_directory(&directory)?
                        && ledger::check_file(&directory.join("source"))?,
                )
            })
            .await
            .map_err(|_| unavailable("Media source inspection stopped"))??;
            if exists {
                self.artifact(&record).await?;
                record.phase = MediaPhase::Ready;
                self.save(record.clone(), false).await?;
            }
        }
        if record.phase == MediaPhase::Ready {
            self.dispatch(record).await?;
        } else if matches!(record.phase, MediaPhase::Running | MediaPhase::Review) {
            let cli = self.cli(&record.declaration, self.stop.child_token())?;
            let operation = if matches!(
                record.declaration.operation,
                MediaOperation::ImportSubtitles { .. }
            ) {
                LocalCliOperation::RecoverSubtitles {
                    request_id: request.into(),
                }
            } else {
                LocalCliOperation::RecoverMedia {
                    request_id: request.into(),
                }
            };
            let _result = cli
                .execute(
                    uuid::Uuid::new_v4().to_string(),
                    operation,
                    None,
                    Arc::new(|_| {}),
                )
                .await;
        }
        self.status(request).await
    }
}

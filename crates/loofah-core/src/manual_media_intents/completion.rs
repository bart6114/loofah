use super::*;
impl VaultContext {
    pub async fn complete_manual_media_summary(
        self: &Arc<Self>,
        request_id: String,
    ) -> Result<ManualMediaSummaryStatus, ApplicationError> {
        let owner = self.clone();
        tokio::spawn(async move { owner.complete_manual_summary_owned(&request_id).await })
            .await
            .map_err(review)?
    }
    async fn complete_manual_summary_owned(
        self: &Arc<Self>,
        request_id: &str,
    ) -> Result<ManualMediaSummaryStatus, ApplicationError> {
        let _request =
            self.admit_session_lifecycle_request("manual-media-summary", OWNER, uuid(request_id)?)?;
        let mut record = journal::load(self, request_id).await?.ok_or_else(|| {
            error(
                ErrorCode::NotFound,
                "Original manual media summary declaration missing",
            )
        })?;
        if matches!(
            record.status.state,
            ManualMediaSummaryState::Child | ManualMediaSummaryState::Skipped
        ) {
            return self.manual_summary_status_owned(record).await;
        }
        if record.request.is_none() {
            return skip(
                self,
                record,
                "Automatic summary was disabled before media admission",
            )
            .await;
        }
        let session = record.declaration.session_id.clone();
        let _admission = self.admit_session_work(&session).await?;
        if record.source.is_none() {
            let original_created = match record.declaration.kind {
                ManualMediaKind::Subtitles => record
                    .session_created_at
                    .clone()
                    .ok_or_else(|| review("Original subtitle session generation missing"))?,
                ManualMediaKind::AudioImport | ManualMediaKind::Transcription => {
                    let cli = LocalCli::new(self.clone())?;
                    let status = cli.media_status(request_id).await?;
                    if status.state == LocalMediaState::Succeeded && status.transcript_id.is_none()
                    {
                        return skip(
                            self,
                            record,
                            "Original media completed without a new transcript",
                        )
                        .await;
                    }

                    let (kind, original_session, created) = cli.media_origin(request_id).await?;
                    let expected = match record.declaration.kind {
                        ManualMediaKind::AudioImport => LocalMediaKind::ImportAudio,
                        _ => LocalMediaKind::Transcribe,
                    };
                    if original_session != session
                        || kind != expected
                        || record
                            .session_created_at
                            .as_ref()
                            .is_some_and(|original| original != &created)
                    {
                        return Err(review("Original manual media source generation changed"));
                    }
                    created
                }
            };
            let operations = self
                .operations
                .as_ref()
                .ok_or_else(|| review("Local operations unavailable"))?;
            let (publication_root, namespace) =
                if record.declaration.kind == ManualMediaKind::Subtitles {
                    (
                        operations.root.join("local-subtitles/publications"),
                        "local-subtitles",
                    )
                } else {
                    (
                        operations.root.join("transcript-publications"),
                        "local-cli-transcribe",
                    )
                };
            let publication = self
                .store
                .completed_transcript_publication(
                    &publication_root,
                    TranscriptPublicationIdentity {
                        vault_id: self.events.vault_id(),
                        namespace: namespace.into(),
                        request_id: uuid(request_id)?,
                    },
                )
                .await
                .map_err(|cause| storage_error(cause, false))?;
            let Some(publication) = publication else {
                return Err(review(
                    "Original media has no completed transcript publication; summary was not dispatched",
                ));
            };
            if publication.session_id != session
                || (publication.recording_sha256.is_none())
                    != (record.declaration.kind == ManualMediaKind::Subtitles)
            {
                return Err(review("Original media publication kind changed"));
            }
            if publication.empty {
                return skip(
                    self,
                    record,
                    "Empty transcript does not trigger an automatic summary",
                )
                .await;
            }
            record.source = Some(ManualMediaSource {
                transcript: TranscriptOnlyBinding {
                    session_created_at: original_created,
                    transcript_revision: publication.revision,
                    transcript_present: true,
                },
                recording_sha256: publication.recording_sha256,
            });
            record.status.state = ManualMediaSummaryState::SourceBound;
            journal::persist(self, &record, false).await?;
        }
        let origin = ManualMediaChildOrigin {
            parent_request_id: request_id.into(),
            child_request_id: record.status.child_request_id.clone(),
            session_id: session.clone(),
            source: record
                .source
                .clone()
                .ok_or_else(|| review("Manual summary source missing"))?,
        };
        let request = record
            .request
            .clone()
            .ok_or_else(|| review("Manual summary request missing"))?;
        let jobs = self.processing_jobs()?;
        if let Some(job) = jobs
            .replay_manual_media(
                origin.child_request_id.clone(),
                request.clone(),
                origin.clone(),
            )
            .await?
        {
            record.status.job = Some(job);
            record.status.state = ManualMediaSummaryState::Child;
            journal::persist(self, &record, false).await?;
            return Ok(record.status);
        }
        drop(
            self.guard_manual_media_summary(OWNER, &origin.child_request_id, Some(&origin))
                .await?,
        );
        let transcripts = self.session_transcripts(&session).await?;
        if !crate::local_capture_intents::automatic_eligible(&transcripts) {
            return skip(
                self,
                record,
                "Automatic summary requires at least five words and 160 normalized characters",
            )
            .await;
        }
        drop(
            self.guard_manual_media_summary(OWNER, &origin.child_request_id, Some(&origin))
                .await?,
        );
        let target = self.read_document(&session, &request.target).await?;
        if target.revision != request.expected_revision
            || crate::jobs::conditional_target_filled(&request, &target)
        {
            return Err(error(
                ErrorCode::Conflict,
                "Automatic summary target changed after the original declaration",
            ));
        }
        let job = jobs
            .submit_with_all_origins(
                OWNER.into(),
                origin.child_request_id.clone(),
                request,
                None,
                None,
                Some(origin),
            )
            .await?;
        record.status.job = Some(job);
        record.status.state = ManualMediaSummaryState::Child;
        journal::persist(self, &record, false).await?;
        Ok(record.status)
    }
}
async fn skip(
    context: &Arc<VaultContext>,
    mut record: Record,
    reason: &str,
) -> Result<ManualMediaSummaryStatus, ApplicationError> {
    record.status.state = ManualMediaSummaryState::Skipped;
    record.status.reason = Some(reason.into());
    journal::persist(context, &record, false).await?;
    Ok(record.status)
}

use super::*;
use futures_util::FutureExt;
use hypr_loofah_protocol::ProcessingJobState;
use hypr_vault_read::TranscriptWithData;
use hypr_vault_write::transcript_publication::{
    TranscriptPublicationIdentity, TranscriptPublicationOutcome, TranscriptSourcePreparation,
    TranscriptSourceSnapshot,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
};

impl LocalCaptureIntents {
    pub(super) fn start(self: Arc<Self>, request: String) {
        tokio::spawn(async move {
            let result = std::panic::AssertUnwindSafe(self.process(&request))
                .catch_unwind()
                .await;
            if let Err(cause) =
                result.unwrap_or_else(|_| Err(review("Local processing worker was lost")))
            {
                let _ = self
                    .update(&request, |record| {
                        record.status.state = if cause.mutation_outcome
                            == Some(hypr_loofah_protocol::MutationOutcome::NotCommitted)
                        {
                            LocalCaptureState::Conflict
                        } else {
                            LocalCaptureState::Review
                        };
                        record.status.message = Some(cause.message);
                    })
                    .await;
            }
            let mut state = self.state.lock().await;
            if let Some(entry) = state.get_mut(&request) {
                entry.running = false;
                entry.admission.take();
            }
            drop(state);
            self.changed.notify_waiters();
        });
    }
    pub(super) async fn process(&self, request: &str) -> Result<(), ApplicationError> {
        let mut record = self
            .state
            .lock()
            .await
            .get(request)
            .ok_or_else(|| review("Original intent missing"))?
            .record
            .clone();
        let declaration = &record.status.declaration;
        let session = declaration.session_id.clone();
        if self.stop.is_cancelled() {
            return Err(review("Local processing stopped before dispatch"));
        }
        if !matches!(record.physical, Some(Ok(()))) {
            return Err(review("Original physical completion is unknown"));
        }
        let persisted = record
            .persistence
            .as_ref()
            .and_then(|value| value.as_ref().ok())
            .cloned()
            .ok_or_else(|| review("Original canonical persistence is unknown"))?;
        if record.inference_dispatched && record.published_revision.is_none() {
            if let Some(recovered) = self.recover_publication(request).await? {
                record = recovered;
            }
        }
        let guard = self
            .context
            .store
            .guard_processing_source(&session, record.source.clone())
            .await
            .map_err(|e| storage_error(e, false))?
            .ok_or_else(|| review("Completed recording is unavailable"))?;
        let binding = guard.binding().clone();
        drop(guard);
        record = self
            .update(request, |record| record.source = Some(binding))
            .await?;
        let repair = record.status.declaration.transcription.is_some()
            && (record.status.declaration.batch_requested
                || persisted.needs_batch_repair
                || !persisted.live_transcription_active);
        if record.status.declaration.transcription.is_none()
            || (record.status.declaration.append
                && !repair
                && persisted.new_transcript_id.is_none())
        {
            self.update(request, |record| {
                record.status.state = LocalCaptureState::Skipped;
                record.status.message =
                    Some("No complete new transcript was requested or persisted".into());
            })
            .await?;
            return Ok(());
        }
        if repair && record.published_revision.is_none() {
            record = self.transcribe(request, record).await?;
            if record.status.state == LocalCaptureState::Skipped {
                return Ok(());
            }
        }
        let guard = self
            .context
            .store
            .guard_processing_source(&session, record.source.clone())
            .await
            .map_err(|e| storage_error(e, false))?
            .ok_or_else(|| review("Completed recording changed"))?;
        drop(guard);
        let transcripts = self.context.session_transcripts(&session).await?;
        if !repair
            && persisted
                .new_transcript_id
                .as_ref()
                .is_some_and(|id| !transcripts.iter().any(|transcript| transcript.id == *id))
        {
            return Err(review(
                "Native transcript completion does not match canonical data",
            ));
        }
        let eligible = automatic_eligible(&transcripts);
        drop(
            self.context
                .store
                .guard_processing_source(&session, record.source.clone())
                .await
                .map_err(|e| storage_error(e, false))?
                .ok_or_else(|| review("Summary source changed"))?,
        );
        if !eligible {
            self.update(request, |record| {
                record.status.state = LocalCaptureState::Skipped;
                record.status.message = Some(
                    "Automatic summary requires at least five words and 160 normalized characters"
                        .into(),
                );
            })
            .await?;
            return Ok(());
        }
        let Some(summary) = record.summary.clone() else {
            self.update(request, |record| {
                record.status.state = LocalCaptureState::Succeeded
            })
            .await?;
            return Ok(());
        };
        // The child owns a separate lifecycle admission. Durable parent source
        // guards reject deletion/changed transcript while that admission is queued.
        self.state
            .lock()
            .await
            .get_mut(request)
            .unwrap()
            .admission
            .take();
        self.update(request, |record| {
            record.status.state = LocalCaptureState::Summarizing
        })
        .await?;
        let origin = LocalCaptureChildOrigin {
            request_id: request.into(),
            session_id: session,
            source: record
                .source
                .clone()
                .ok_or_else(|| review("Summary source missing"))?,
        };
        let job = self
            .context
            .submit_local_capture_summary(request.into(), summary, origin)
            .await?;
        self.update(request, |record| {
            record.status.summary_job_id = Some(job.id.clone())
        })
        .await?;
        loop {
            let snapshot = self
                .context
                .processing_job(SUMMARY_OWNER.into(), job.id.clone())
                .await?;
            if snapshot.state.is_terminal() {
                self.update(request, |record| {
                    record.status.state = match snapshot.state {
                        ProcessingJobState::Succeeded => LocalCaptureState::Succeeded,
                        ProcessingJobState::Conflict => LocalCaptureState::Conflict,
                        _ => LocalCaptureState::Review,
                    };
                    if snapshot.state != ProcessingJobState::Succeeded {
                        record.status.message =
                            Some(format!("Summary child ended {:?}", snapshot.state));
                    }
                })
                .await?;
                return Ok(());
            }
            if self.stop.is_cancelled() {
                self.context
                    .cancel_processing_job(SUMMARY_OWNER.into(), job.id.clone())
                    .await?;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }
    async fn transcribe(&self, request: &str, record: Record) -> Result<Record, ApplicationError> {
        let session = record.status.declaration.session_id.clone();
        let source = match self
            .context
            .store
            .prepare_transcript_replacement_source(&session)
            .await
            .map_err(|e| storage_error(e, false))?
        {
            Some(TranscriptSourcePreparation::Ready(source)) => Arc::from(source),
            _ => return Err(review("Completed transcript source unavailable")),
        };
        let source: Arc<TranscriptSourceSnapshot> = source;
        let binding = record
            .source
            .as_ref()
            .ok_or_else(|| review("Original source binding missing"))?;
        if source.recording_sha256() != binding.recording_sha256
            || source.transcript_revision() != binding.transcript_revision
        {
            return Err(review("Completed source changed before repair"));
        }
        let baseline = source.baseline();
        let directory = self.root.join(request);
        let transcript = if let Some(expected) = &record.result_sha256 {
            let path = directory.join("result.json");
            let expected = expected.clone();
            let pin = self.context.clone();
            tokio::task::spawn_blocking(move || {
                let _pin = pin;
                let bytes = ledger::read(
                    &path,
                    hypr_loofah_protocol::TRANSCRIPTION_RESULT_LIMIT as u64,
                )?;
                if ledger::hash(&bytes) != expected {
                    return Err(review("Retained Local inference output changed"));
                }
                let mut result: hypr_vault_read::TranscriptJson =
                    serde_json::from_slice(&bytes).map_err(review)?;
                if result.transcripts.len() != 1 {
                    return Err(review("Invalid retained Local inference output"));
                }
                let transcript = result.transcripts.pop().unwrap();
                if ledger::encode_result(&transcript)? != bytes {
                    return Err(review("Unsupported retained Local inference fields"));
                }
                Ok(transcript)
            })
            .await
            .map_err(review)??
        } else {
            if record.inference_dispatched {
                return Err(review(
                    "Inference outcome is unknown; automatic replay is forbidden",
                ));
            }
            let extension = Path::new(source.recording_selection())
                .extension()
                .and_then(|value| value.to_str())
                .ok_or_else(|| review("Invalid recording selection"))?;
            let path = directory.join(format!("source.{extension}"));
            let copy = source.clone();
            let target = path.clone();
            let pin = self.context.clone();
            tokio::task::spawn_blocking(move || {
                let _pin = pin;
                copy_source(&copy, &target)
            })
            .await
            .map_err(review)??;
            let admission = self
                .state
                .lock()
                .await
                .get(request)
                .and_then(|entry| entry.admission.clone())
                .ok_or_else(|| review("Original lifecycle admission missing"))?;
            let input = crate::transcription_jobs::LocalInferenceInput {
                request_id: request.into(),
                session_id: session.clone(),
                options: record
                    .status
                    .declaration
                    .transcription
                    .clone()
                    .ok_or_else(|| review("Frozen transcription options missing"))?,
                source_sha256: source.recording_sha256().into(),
                path,
                audio: source.audio(),
                owner: Box::new((source.clone(), admission)),
            };
            let prepared = self
                .context
                .prepare_local_capture_engine(&input, self.stop.child_token())
                .await?;
            if record.inference_dispatched {
                return Err(review(
                    "Inference was already dispatched; explicit retained-output recovery is required",
                ));
            }
            self.update(request, |record| {
                record.inference_dispatched = true;
                record.status.state = LocalCaptureState::Transcribing;
            })
            .await?;
            let transcript = self
                .context
                .transcribe_local_capture_source(input, prepared, self.stop.child_token())
                .await?;
            let bytes = ledger::encode_result(&transcript)?;
            let sha = ledger::hash(&bytes);
            let resultpath = directory.join("result.json");
            let pin = self.context.clone();
            tokio::task::spawn_blocking(move || {
                let _pin = pin;
                ledger::write_new(&resultpath, &bytes)
            })
            .await
            .map_err(review)??;
            self.update(request, |record| {
                record.result_sha256 = Some(sha);
                record.status.transcript_id = Some(transcript.id.clone());
            })
            .await?;
            transcript
        };
        if transcript.session_id != session || transcript.user_id != TRANSCRIPTION_OWNER {
            return Err(review("Retained inference identity changed"));
        }
        if transcript.words.is_empty() {
            return self
                .update(request, |record| {
                    record.status.state = LocalCaptureState::Skipped;
                    record.status.message = Some("No speech detected".into());
                })
                .await;
        }
        let plan = self
            .context
            .store
            .prepare_transcript_publication(
                &self
                    .root
                    .parent()
                    .ok_or_else(|| review("Operation root unavailable"))?
                    .join("transcript-publications"),
                self.publication_identity(request)?,
                baseline,
                transcript,
            )
            .await
            .map_err(|e| storage_error(e, true))?;
        let outcome = plan
            .recheck()
            .await
            .map_err(|e| storage_error(e, true))?
            .commit()
            .await
            .map_err(|e| storage_error(e, true))?;
        match outcome {
            TranscriptPublicationOutcome::Published { revision, .. }
            | TranscriptPublicationOutcome::AlreadyCommitted { revision, .. } => {
                self.update(request, |record| {
                    record.published_revision = Some(revision.clone());
                    if let Some(source) = &mut record.source {
                        source.transcript_present = true;
                        source.transcript_revision = revision;
                    }
                })
                .await
            }
            _ => Err(review(format!(
                "Retained transcript publication requires recovery: {outcome:?}"
            ))),
        }
    }
    pub(super) fn publication_identity(
        &self,
        request: &str,
    ) -> Result<TranscriptPublicationIdentity, ApplicationError> {
        Ok(TranscriptPublicationIdentity {
            vault_id: self.context.events.vault_id(),
            namespace: "local-capture-transcription".into(),
            request_id: uuid(request)?,
        })
    }
}
fn copy_source(source: &TranscriptSourceSnapshot, path: &Path) -> Result<(), ApplicationError> {
    if ledger::check_file(path)? {
        return crate::capture_append::verify_file_sha256(path, source.recording_sha256());
    }
    let mut input = source.recording_file().try_clone().map_err(review)?;
    input.rewind().map_err(review)?;
    let mut output = File::options()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(review)?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut block = [0u8; 64 * 1024];
    loop {
        let read = input.read(&mut block).map_err(review)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > 8 * 1024 * 1024 * 1024 {
            return Err(review(
                "Local inference source exceeds its retained byte budget",
            ));
        }
        digest.update(&block[..read]);
        output.write_all(&block[..read]).map_err(review)?;
    }
    output.sync_all().map_err(review)?;
    if ledger::hex(&digest.finalize()) != source.recording_sha256() {
        return Err(review("Recording changed while retaining inference source"));
    }
    let mut permissions = output.metadata().map_err(review)?.permissions();
    permissions.set_readonly(true);
    output.set_permissions(permissions).map_err(review)?;
    ledger::sync_parent(path)
}
pub(super) fn automatic_eligible(transcripts: &[TranscriptWithData]) -> bool {
    let mut words = 0usize;
    let mut characters = 0usize;
    for transcript in transcripts {
        words = words.saturating_add(transcript.words.len());
        for word in &transcript.words {
            for text in word.text.split_whitespace() {
                if characters > 0 {
                    characters = characters.saturating_add(1);
                }
                characters = characters.saturating_add(text.chars().count());
            }
        }
    }
    words >= 5 && characters >= 160
}

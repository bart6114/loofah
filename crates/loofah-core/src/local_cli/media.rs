use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use hypr_loofah_protocol::*;
use hypr_vault_write::{
    recording_publish::{RecordingExpectation, RecordingPublication, RecordingPublicationRequest},
    transcript_publication::{
        TranscriptPublicationIdentity, TranscriptPublicationOutcome, TranscriptSourcePreparation,
    },
};

use super::{LocalCli, LocalCliArtifact, LocalCliOutput};
use crate::vault::{error, storage_error};

mod journal;
mod recovery;
mod status;

pub type LocalMediaFuture<T> =
    Pin<Box<dyn Future<Output = Result<T, ApplicationError>> + Send + 'static>>;

pub struct LocalMediaTranscription {
    pub transcript: TranscriptWithData,
    pub publication_error: Option<ApplicationError>,
}
impl From<TranscriptWithData> for LocalMediaTranscription {
    fn from(transcript: TranscriptWithData) -> Self {
        Self {
            transcript,
            publication_error: None,
        }
    }
}

/// Device engines receive private inputs and return artifacts; only Core publishes canonical files.
pub trait LocalMediaHost: Send + Sync + 'static {
    fn cancellation(&self, _request_id: &str) -> tokio_util::sync::CancellationToken {
        tokio_util::sync::CancellationToken::new()
    }
    fn normalize(
        &self,
        source: LocalCliArtifact,
        filename: String,
        temporary: PathBuf,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<LocalCliArtifact>;
    #[allow(clippy::too_many_arguments)]
    fn transcribe(
        &self,
        request_id: String,
        source: LocalCliArtifact,
        filename: String,
        session_id: String,
        audio: hypr_vault_read::SessionAudio,
        config: crate::settings::AppConfig,
        temporary: PathBuf,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<LocalMediaTranscription>;
}

impl LocalCli {
    pub(super) async fn execute_media(
        &self,
        request_id: String,
        operation: LocalCliOperation,
        artifact: Option<LocalCliArtifact>,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> Result<LocalCliOutput, ApplicationError> {
        let host = self
            .media
            .as_ref()
            .ok_or_else(|| error(ErrorCode::Unavailable, "Local media host is unavailable"))?;
        let root = self
            .context
            .operations
            .as_ref()
            .ok_or_else(|| {
                error(
                    ErrorCode::Unavailable,
                    "Local operation storage is unavailable",
                )
            })?
            .root
            .clone();
        use sha2::{Digest, Sha256};
        let fingerprint = Sha256::digest(
            serde_json::to_vec(&(
                &operation,
                artifact
                    .as_ref()
                    .map(|value| (&value.sha256, value.size_bytes)),
            ))
            .map_err(|_| error(ErrorCode::Validation, "Invalid local media declaration"))?,
        )
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
        let _request = self.context.admit_session_lifecycle_request(
            "local-media",
            "local-cli",
            uuid::Uuid::parse_str(&request_id).unwrap(),
        )?;
        let mut claim = journal::Claim::open(
            self.context.clone(),
            root.join("local-media"),
            &request_id,
            fingerprint,
            operation.clone(),
            self.context.settings.read_current().await.map_err(|_| {
                error(
                    ErrorCode::Unavailable,
                    "Cannot freeze Local transcription settings",
                )
            })?,
        )
        .await?;
        if let Some(result) = claim.result() {
            return result.clone().map(|reply| LocalCliOutput {
                reply,
                artifact: None,
            });
        }
        progress(LocalCliProgress {
            schema_version: "2".into(),
            phase: "preparing media".into(),
            processed: 0,
            total: None,
            added: 0,
            updated: 0,
            deleted: 0,
            elapsed_ms: 0,
        });
        let result = match operation {
            LocalCliOperation::Import {
                filename,
                title,
                into,
                transcribe,
                metadata,
            } => {
                self.import(
                    &request_id,
                    &mut claim,
                    host,
                    &root,
                    filename,
                    title,
                    into,
                    transcribe,
                    metadata,
                    None,
                    artifact
                        .ok_or_else(|| error(ErrorCode::Validation, "Import source is required"))?,
                    progress,
                )
                .await
            }
            LocalCliOperation::ImportIntoChecked {
                session_id,
                filename,
                transcribe,
                expected_metadata_revision,
                created_at,
            } => {
                self.import(
                    &request_id,
                    &mut claim,
                    host,
                    &root,
                    filename,
                    None,
                    Some(session_id),
                    transcribe,
                    LocalInitialSessionMetadata::default(),
                    Some((expected_metadata_revision, created_at)),
                    artifact
                        .ok_or_else(|| error(ErrorCode::Validation, "Import source is required"))?,
                    progress,
                )
                .await
            }
            LocalCliOperation::Transcribe { session_id } => self
                .transcribe(
                    &request_id,
                    &mut claim,
                    host,
                    &root,
                    &session_id,
                    None,
                    progress,
                )
                .await
                .map(|outcome| LocalCliReply::Transcribed {
                    session_id,
                    transcript_id: outcome.transcript_id,
                    word_count: outcome.words,
                }),
            LocalCliOperation::TranscribeChecked {
                session_id,
                expected,
            } => self
                .transcribe(
                    &request_id,
                    &mut claim,
                    host,
                    &root,
                    &session_id,
                    Some(expected),
                    progress,
                )
                .await
                .map(|outcome| LocalCliReply::Transcribed {
                    session_id,
                    transcript_id: outcome.transcript_id,
                    word_count: outcome.words,
                }),
            _ => unreachable!("media dispatch is restricted"),
        };
        claim.finish(result.clone()).await?;
        result.map(|reply| LocalCliOutput {
            reply,
            artifact: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn import(
        &self,
        request_id: &str,
        claim: &mut journal::Claim,
        host: &Arc<dyn LocalMediaHost>,
        root: &std::path::Path,
        filename: String,
        title: Option<String>,
        into: Option<String>,
        transcribe: bool,
        metadata: LocalInitialSessionMetadata,
        metadata_guard: Option<(String, Option<String>)>,
        source: LocalCliArtifact,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> Result<LocalCliReply, ApplicationError> {
        if metadata_guard
            .as_ref()
            .and_then(|(_, created)| created.as_ref())
            .is_some_and(|created| chrono::DateTime::parse_from_rfc3339(created).is_err())
        {
            return Err(error(ErrorCode::Validation, "Invalid imported audio date"));
        }
        if host.cancellation(request_id).is_cancelled() {
            return Err(cancelled(false));
        }
        let path = std::path::Path::new(&filename);
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if path.file_name().and_then(|value| value.to_str()) != Some(filename.as_str())
            || !["wav", "mp3", "ogg", "mp4", "m4a", "flac", "webm", "aac"]
                .contains(&extension.as_str())
        {
            return Err(error(
                ErrorCode::Validation,
                "unsupported audio format; supported extensions: wav, mp3, ogg, mp4, m4a, flac, webm, aac",
            ));
        }
        if into.is_some() && (title.is_some() || metadata != LocalInitialSessionMetadata::default())
        {
            return Err(error(
                ErrorCode::Validation,
                "Import into an existing meeting cannot override its metadata",
            ));
        }
        let namespace = "local-cli-import";
        let session_id = match &into {
            Some(id) => id.clone(),
            None => self.context.created_session_id(namespace, request_id)?,
        };
        let _admission = self.context.admit_session_work(&session_id).await?;
        self.context
            .ensure_recording_publication_ready(&session_id)?;
        let existing = match &into {
            Some(id) => {
                let snapshot = self.context.read_metadata(id).await?;
                if metadata_guard
                    .as_ref()
                    .is_some_and(|(expected, _)| expected != &snapshot.revision)
                {
                    return Err(error(
                        ErrorCode::Conflict,
                        "Session metadata changed before audio import",
                    ));
                }
                Some(snapshot.meta)
            }
            None => None,
        };
        if existing.is_some()
            && !matches!(
                self.context
                    .store
                    .open_recording(&session_id)
                    .await
                    .map_err(|cause| storage_error(cause, false))?,
                Some(hypr_vault_write::media::RecordingSnapshot::Absent)
            )
        {
            return Err(error(
                ErrorCode::Validation,
                &format!(
                    "meeting {session_id} already has a recording; the CLI never replaces existing audio"
                ),
            ));
        }
        let source_binding = serde_json::to_string(&(&filename, &source.sha256, source.size_bytes))
            .map_err(|_| error(ErrorCode::Validation, "Invalid import identity"))?;
        let meta = match existing {
            Some(meta) => meta,
            None => {
                self.context
                    .create_session_with_metadata(
                        namespace.into(),
                        request_id.into(),
                        CreateSessionRequest {
                            title: title.unwrap_or_else(|| {
                                path.file_stem()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned()
                            }),
                            markdown: String::new(),
                        },
                        crate::InitialSessionMetadata {
                            created_at: metadata.created_at,
                            started_at: metadata.started_at,
                            ended_at: metadata.ended_at,
                            tags: metadata.tags,
                            author: metadata.author,
                            skill: metadata.skill,
                        },
                    )
                    .await?
            }
        };
        let normalized = host
            .normalize(
                source,
                filename,
                root.join("local-media-artifacts"),
                progress.clone(),
            )
            .await
            .map_err(|mut cause| {
                cause.message = format!(
                    "meeting {session_id} exists, but importing its audio failed: {}",
                    cause.message
                );
                cause
            })?;
        if host.cancellation(request_id).is_cancelled() {
            return Err(cancelled(false));
        }
        let request = RecordingPublicationRequest {
            vault_id: self.context.events.vault_id(),
            namespace: namespace.into(),
            request_id: uuid::Uuid::parse_str(request_id).unwrap(),
            session_id: session_id.clone(),
            source_binding,
            expected: RecordingExpectation::AbsentImport,
            artifact_sha256: normalized.sha256.clone(),
            artifact_size_bytes: normalized.size_bytes,
            layout: hypr_vault_read::AudioLayout::Mixed,
            first_audio_observed_at_ms: 0,
            capture_ended_at_ms: None,
        };
        claim
            .bind_import(journal::ImportRecovery {
                meta: meta.clone(),
                publication: request.clone(),
            })
            .await?;
        let mut ticket = self
            .context
            .recording_publications
            .begin(&session_id, request_id)?
            .with_events(self.context.events.clone());
        let plan = self
            .context
            .store
            .prepare_recording_publication(
                &root.join("recording-publications"),
                request,
                normalized,
            )
            .await
            .map_err(|cause| storage_error(cause, true))?
            .recheck()
            .await
            .map_err(|cause| storage_error(cause, true))?;
        ticket.committing();
        match plan
            .commit()
            .await
            .map_err(|cause| storage_error(cause, true))?
        {
            RecordingPublication::Published { .. } => ticket.resolved(),
            RecordingPublication::Changed => {
                ticket.resolved();
                return Err(error(
                    ErrorCode::Conflict,
                    "Session changed or already has a recording; import was not applied",
                ));
            }
            RecordingPublication::Busy => {
                ticket.resolved();
                return Err(error(
                    ErrorCode::Unavailable,
                    "Recording is busy; import was not applied",
                ));
            }
            RecordingPublication::Uncertain => {
                return Err(unknown(
                    "Recording publication needs review; do not repeat the import",
                ));
            }
        }
        if let Some((_, Some(created_at))) = metadata_guard {
            if let Ok(current) = self.context.read_metadata(&session_id).await
                && current.meta.created_at == meta.created_at
            {
                let patch =
                    serde_json::from_value(serde_json::json!({"created_at": created_at}))
                        .map_err(|_| error(ErrorCode::Validation, "Invalid imported audio date"))?;
                // The estimate is advisory; an edit during import must win.
                let _ = self
                    .context
                    .save_metadata_local(
                        &session_id,
                        SaveMetadataRequest {
                            patch,
                            expected_revision: current.revision,
                        },
                    )
                    .await;
            }
        }
        let audio_path = self
            .context
            .store
            .vault_base()
            .join(
                self.context
                    .store
                    .session_dir(&session_id)
                    .await
                    .map_err(|cause| storage_error(cause, false))?,
            )
            .join("audio.mp3");
        let transcription = if transcribe {
            Some(
                self.transcribe(request_id, claim, host, root, &session_id, None, progress)
                    .await,
            )
        } else {
            None
        };
        Ok(LocalCliReply::Imported {
            meta,
            audio_path,
            transcription,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn transcribe(
        &self,
        request_id: &str,
        claim: &mut journal::Claim,
        host: &Arc<dyn LocalMediaHost>,
        root: &std::path::Path,
        session_id: &str,
        expected: Option<LocalMediaSourceSnapshot>,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> Result<LocalTranscriptionOutcome, ApplicationError> {
        if host.cancellation(request_id).is_cancelled() {
            return Err(cancelled(false));
        }
        let _admission = self.context.admit_session_work(session_id).await?;
        self.context
            .ensure_recording_publication_ready(session_id)?;
        let metadata = self.context.read_metadata(session_id).await?;
        let source = match self
            .context
            .store
            .prepare_transcript_replacement_source(session_id)
            .await
            .map_err(|cause| storage_error(cause, false))?
        {
            Some(TranscriptSourcePreparation::Ready(source)) => source,
            Some(TranscriptSourcePreparation::NoRecording) => {
                return Err(error(
                    ErrorCode::NotFound,
                    &format!("audio recording for meeting '{session_id}' not found"),
                ));
            }
            Some(TranscriptSourcePreparation::Busy) => {
                return Err(error(ErrorCode::Unavailable, "Recording is busy"));
            }
            _ => return Err(error(ErrorCode::Conflict, "Recording source changed")),
        };
        let actual = source_snapshot(session_id, &metadata.meta.created_at, &source);
        if expected
            .as_ref()
            .is_some_and(|expected| expected != &actual)
        {
            return Err(error(
                ErrorCode::Conflict,
                "Recording or transcript changed before manual transcription",
            ));
        }
        let config = claim.config()?;
        let baseline = source.baseline();
        let audio = source.audio();
        let filename = std::path::Path::new(source.recording_selection())
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let file = source
            .recording_file()
            .try_clone()
            .map_err(|_| error(ErrorCode::Internal, "Cannot retain recording input"))?;
        let size = file
            .metadata()
            .map_err(|_| error(ErrorCode::Internal, "Cannot inspect recording input"))?
            .len();
        let sha256 = source.recording_sha256().to_owned();
        let input = LocalCliArtifact::new(file, size, sha256, None, Box::new(source));
        let generated = host
            .transcribe(
                request_id.into(),
                input,
                filename,
                session_id.into(),
                audio,
                config,
                root.join("local-media-artifacts"),
                progress,
            )
            .await?;
        let publication_error = generated.publication_error;
        let generated = generated.transcript;
        let outcome = LocalTranscriptionOutcome {
            transcript_id: generated.id.clone(),
            words: generated.words.len(),
        };
        claim.bind_transcription(outcome.clone()).await?;
        let identity = TranscriptPublicationIdentity {
            vault_id: self.context.events.vault_id(),
            namespace: "local-cli-transcribe".into(),
            request_id: uuid::Uuid::parse_str(request_id).unwrap(),
        };
        let plan = self
            .context
            .store
            .prepare_transcript_publication(
                &root.join("transcript-publications"),
                identity,
                baseline,
                generated,
            )
            .await
            .map_err(|cause| storage_error(cause, true))?;
        if let Some(mut cause) = publication_error {
            cause.mutation_outcome = Some(MutationOutcome::Unknown);
            cause
                .message
                .push_str("; generated transcript is retained for explicit recovery");
            return Err(cause);
        }
        if host.cancellation(request_id).is_cancelled() {
            return Err(cancelled(true));
        }
        if self
            .context
            .read_metadata(session_id)
            .await?
            .meta
            .created_at
            != actual.session_created_at
        {
            return Err(error(
                ErrorCode::Conflict,
                "Original session was replaced; generated transcript is retained",
            ));
        }
        match plan
            .recheck()
            .await
            .map_err(|cause| storage_error(cause, true))?
            .commit()
            .await
            .map_err(|cause| storage_error(cause, true))?
        {
            TranscriptPublicationOutcome::Published { .. }
            | TranscriptPublicationOutcome::AlreadyCommitted { .. } => Ok(outcome),
            TranscriptPublicationOutcome::Empty => Err(error(
                ErrorCode::Validation,
                "No speech was detected in the audio.",
            )),
            TranscriptPublicationOutcome::Conflict => Err(error(
                ErrorCode::Conflict,
                "Recording or transcript changed; generated transcript is retained without overwriting user edits",
            )),
            TranscriptPublicationOutcome::Busy => Err(error(
                ErrorCode::Unavailable,
                "Transcript publication is busy; generated transcript is retained",
            )),
            TranscriptPublicationOutcome::RequiresReview => Err(unknown(
                "Transcript publication needs review; generated transcript is retained",
            )),
        }
    }
}

fn unknown(message: &str) -> ApplicationError {
    let mut cause = error(ErrorCode::Conflict, message);
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    cause
}

fn source_snapshot(
    session: &str,
    created: &str,
    source: &hypr_vault_write::transcript_publication::TranscriptSourceSnapshot,
) -> LocalMediaSourceSnapshot {
    LocalMediaSourceSnapshot {
        session_id: session.into(),
        session_created_at: created.into(),
        recording_selection: source.recording_selection().into(),
        recording_sha256: source.recording_sha256().into(),
        transcript_revision: source.transcript_revision().into(),
        transcript_present: source.transcript_present(),
        audio: source.audio(),
    }
}
impl LocalCli {
    pub async fn media_source(
        &self,
        session: &str,
    ) -> Result<LocalMediaSourceSnapshot, ApplicationError> {
        let _admission = self.context.admit_session_work(session).await?;
        self.context.ensure_recording_publication_ready(session)?;
        let metadata = self.context.read_metadata(session).await?;
        match self
            .context
            .store
            .prepare_transcript_replacement_source(session)
            .await
            .map_err(|cause| storage_error(cause, false))?
        {
            Some(TranscriptSourcePreparation::Ready(source)) => {
                Ok(source_snapshot(session, &metadata.meta.created_at, &source))
            }
            Some(TranscriptSourcePreparation::NoRecording) => {
                Err(error(ErrorCode::NotFound, "Recording not found"))
            }
            _ => Err(error(
                ErrorCode::Conflict,
                "Recording source is busy or changed",
            )),
        }
    }
}

fn cancelled(retained: bool) -> ApplicationError {
    let mut cause = error(
        ErrorCode::Cancelled,
        if retained {
            "Transcription stopped; original generated output is retained for explicit recovery"
        } else {
            "Local media processing stopped"
        },
    );
    cause.mutation_outcome = Some(if retained {
        MutationOutcome::Unknown
    } else {
        MutationOutcome::NotCommitted
    });
    cause
}

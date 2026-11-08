use super::*;
use crate::commands::NewSessionOptions;
use hypr_loofah_client::media_jobs::{MediaAuthority, MediaScope, MediaSubmissions};
use hypr_loofah_protocol::*;
use std::{
    fs::File,
    io::{Seek, SeekFrom},
    time::Duration,
};

impl RemoteSession {
    async fn media(&self) -> Result<Arc<MediaAuthority>> {
        self.client
            .media_jobs_capability(&CancellationToken::new())?;
        let profile = self.generation.profile();
        Ok(self
            .client
            .bind_media(
                self.lease.clone(),
                MediaScope {
                    profile_id: profile.id.clone(),
                    server: profile.server.clone(),
                    member_id: self.member.clone(),
                    vault_id: self.vault.clone(),
                },
                CancellationToken::new(),
            )
            .await?)
    }
    async fn media_options(
        &self,
        transcribe: bool,
    ) -> Result<(
        Option<MediaTranscriptionOptions>,
        Option<CaptureProcessingSummary>,
    )> {
        let transcription = if transcribe {
            let RemoteReply::TranscriptionPolicy(policy) = self
                .execute(|vault_id| RemoteOperation::TranscriptionPolicy { vault_id })
                .await?
            else {
                return Err(unexpected());
            };
            if !policy.batch_enabled {
                return Err(Error::operation(
                    "transcribe audio",
                    "Server batch transcription is disabled",
                ));
            }
            Some(MediaTranscriptionOptions {
                model: policy.model.ok_or_else(|| {
                    Error::operation(
                        "transcribe audio",
                        "The server has no configured transcription model",
                    )
                })?,
                languages: Vec::new(),
            })
        } else {
            None
        };
        let summary = if self
            .client
            .capabilities()
            .features
            .iter()
            .any(|f| f == "processing_settings")
        {
            let RemoteReply::ProcessingSettings(settings) = self
                .execute(|vault_id| RemoteOperation::ProcessingSettings { vault_id })
                .await?
            else {
                return Err(unexpected());
            };
            settings
                .settings
                .summary_model
                .map(|_| CaptureProcessingSummary {
                    target: DocumentTarget::Summary,
                    title_policy: TitlePolicy::GenerateIfEmpty,
                })
        } else {
            None
        };
        Ok((transcription, summary))
    }
    pub async fn import_media(
        &self,
        file: PathBuf,
        title: Option<String>,
        into: Option<String>,
        transcribe: bool,
        metadata: NewSessionOptions,
        json: bool,
    ) -> Result<u8> {
        self.client
            .media_jobs_capability(&CancellationToken::new())?;
        let created_at = metadata.created_at.clone();
        let metadata = LocalInitialSessionMetadata {
            created_at: metadata.created_at,
            started_at: metadata.started_at,
            ended_at: metadata.ended_at,
            tags: metadata.tags,
            author: metadata.author,
            skill: metadata.skill,
        };
        let initial = (title.is_some() || metadata != LocalInitialSessionMetadata::default())
            .then_some(MediaImportInitialMetadata { title, metadata });
        if initial.is_some() {
            if into.is_some() {
                return Err(Error::operation(
                    "import audio",
                    "Initial title and metadata require a new session",
                ));
            }
            self.client
                .media_initial_metadata_capability(&CancellationToken::new())?;
        }
        let authority = self.media().await?;
        crate::commands::import::validate_format(&file)?;
        let expected = match &into {
            Some(id) => Some(authority.source(id).await?),
            None => None,
        };
        let (transcription, summary) = self.media_options(transcribe).await?;
        let (filename, descriptor, source) = open_source(file, authority.clone()).await?;
        let declaration = MediaDeclaration {
            request_id: self.request_id.clone(),
            operation: MediaOperation::ImportAudio {
                session_id: into,
                expected,
                filename,
                content_type: None,
                created_at,
                initial,
                transcribe,
                source: descriptor,
            },
            transcription,
            summary,
        };
        let store = MediaSubmissions::new(self.media_receipts.clone())?;
        let (submission, first) = store
            .prepare(authority.scope().clone(), declaration)
            .await?;
        if !first {
            return Err(review(
                &self.request_id,
                "This original request is already saved; inspect or explicitly recover it",
            ));
        }
        announce(&self.request_id, json);
        let admission = authority
            .declare(&submission)
            .await
            .map_err(|cause| original_error(cause, &self.request_id))?;
        let submission = store
            .observe_admission(&submission, &admission)
            .await
            .map_err(|cause| original_error(cause, &self.request_id))?;
        let status = if admission.status.phase == MediaPhase::Receiving {
            authority
                .upload_source(&submission, &admission, source)
                .await
                .map_err(|cause| original_error(cause, &self.request_id))?
        } else {
            admission.status
        };
        let status = wait(&authority, status).await?;
        self.emit_media(status, "import", json).await
    }
    pub async fn transcribe_media(&self, session: &str, json: bool) -> Result<()> {
        let authority = self.media().await?;
        let expected = authority
            .source(session)
            .await?
            .recording
            .ok_or_else(|| Error::NotFound(format!("recording for session '{session}'")))?;
        let (transcription, summary) = self.media_options(true).await?;
        let declaration = MediaDeclaration {
            request_id: self.request_id.clone(),
            operation: MediaOperation::Transcribe { expected },
            transcription,
            summary,
        };
        let store = MediaSubmissions::new(self.media_receipts.clone())?;
        let (submission, first) = store
            .prepare(authority.scope().clone(), declaration)
            .await?;
        if !first {
            return Err(review(
                &self.request_id,
                "This original request is already saved; inspect or explicitly recover it",
            ));
        }
        announce(&self.request_id, json);
        let admission = authority
            .declare(&submission)
            .await
            .map_err(|cause| original_error(cause, &self.request_id))?;
        store
            .observe_admission(&submission, &admission)
            .await
            .map_err(|cause| original_error(cause, &self.request_id))?;
        let status = wait(&authority, admission.status).await?;
        self.emit_media(status, "transcribe", json).await?;
        Ok(())
    }
    pub async fn recover_media(
        &self,
        request: &str,
        source: Option<PathBuf>,
        json: bool,
    ) -> Result<()> {
        let authority = self.media().await?;
        let store = MediaSubmissions::new(self.media_receipts.clone())?;
        let submission = store
            .load(authority.scope().clone(), request.into())
            .await?
            .ok_or_else(|| Error::NotFound(format!("original device media request '{request}'")))?;
        let status = authority
            .recover(&submission)
            .await
            .map_err(|cause| original_error(cause, request))?;
        let admission = MediaAdmission {
            declaration: status.declaration.clone(),
            revision: status.revision.clone(),
            status,
        };
        let submission = store
            .observe_admission(&submission, &admission)
            .await
            .map_err(|cause| original_error(cause, request))?;
        let status = if admission.status.phase == MediaPhase::Receiving {
            if let Some(path) = source {
                let (_, descriptor, file) = open_source(path, authority.clone()).await?;
                let original = submission
                    .declaration
                    .operation
                    .upload()
                    .ok_or_else(unexpected)?
                    .1;
                if &descriptor != original {
                    return Err(review(
                        request,
                        "Selected source differs from the original saved SHA/length; nothing was uploaded",
                    ));
                }
                authority
                    .upload_source(&submission, &admission, file)
                    .await
                    .map_err(|cause| original_error(cause, request))?
            } else {
                admission.status
            }
        } else {
            admission.status
        };
        let status = wait(&authority, status).await?;
        output::emit(&if json {
            output::json(
                "sessions.recover",
                &serde_json::json!({"request_id":request,"result":status}),
                None,
            )?
        } else {
            output::raw_json(&status)?
        });
        terminal(&status)?;
        Ok(())
    }
    pub async fn inspect_media(&self, request: Option<&str>, json: bool) -> Result<()> {
        let authority = self.media().await?;
        let store = MediaSubmissions::new(self.media_receipts.clone())?;
        let value = if let Some(request) = request {
            let submission = store
                .load(authority.scope().clone(), request.into())
                .await?;
            let status = match authority.status(request).await {
                Ok(status) => Some(status),
                Err(cause) if cause.code == ErrorCode::NotFound => None,
                Err(cause) => return Err(cause.into()),
            };
            if submission.is_none() && status.is_none() {
                return Err(Error::NotFound(format!("media request '{request}'")));
            }
            serde_json::json!({"submission":submission,"status":status})
        } else {
            serde_json::json!({"submissions":store.list(authority.scope().clone()).await?,"requests":authority.list(None).await?})
        };
        output::emit(&if json {
            output::json("sessions.media", &value, None)?
        } else {
            output::raw_json(&value)?
        });
        Ok(())
    }
    async fn emit_media(
        &self,
        status: MediaRequestStatus,
        command: &'static str,
        json: bool,
    ) -> Result<u8> {
        let media = status.media.as_ref().ok_or_else(unexpected)?;
        let mut partial_error = None;
        if command != "import" || media.imported.is_none() {
            terminal(&status)?;
        } else if status.phase != MediaPhase::Succeeded {
            partial_error = terminal(&status).err();
        }
        let transcript = media
            .transcript_id
            .as_ref()
            .map(|id| serde_json::json!({"id":id,"words":media.word_count}));
        let mut data = if command == "import" {
            let meta = media.imported.as_ref().ok_or_else(unexpected)?;
            serde_json::json!({"id":meta.id,"title":meta.title,"created_at":meta.created_at,"audio":null,"request_id":status.declaration.request_id})
        } else {
            serde_json::json!({"id":media.session_id,"transcript":transcript,"request_id":status.declaration.request_id})
        };
        if transcript.is_some() {
            data["transcript"] = serde_json::to_value(transcript)
                .map_err(|cause| Error::operation("serialize media result", cause.to_string()))?;
        }
        if let Some(cause) = &partial_error {
            data["transcript"] = serde_json::Value::Null;
            data["transcript_error"] =
                serde_json::json!({"code":cause.code(),"message":cause.to_string()});
            eprintln!(
                "error: meeting {} was imported, but transcription failed: {cause}",
                media.session_id
            );
        }

        output::emit(&if json {
            output::json(command, &data, None)?
        } else if command == "import" {
            media.session_id.clone()
        } else {
            media.transcript_id.clone().ok_or_else(unexpected)?
        });
        Ok(partial_error.map_or(0, |cause| cause.exit_code()))
    }
}
async fn open_source(
    path: PathBuf,
    authority: Arc<MediaAuthority>,
) -> Result<(String, MediaUploadSource, File)> {
    authority.check_current().await?;
    let result = tokio::task::spawn_blocking(move || {
        let _owner = authority;
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                Error::operation("read media source", "Source filename must be valid UTF-8")
            })?
            .to_owned();
        let mut file = File::open(&path)
            .map_err(|cause| Error::operation("read media source", cause.to_string()))?;
        if !file
            .metadata()
            .map_err(|cause| Error::operation("read media source", cause.to_string()))?
            .is_file()
        {
            return Err(Error::operation(
                "read media source",
                "Source must be a regular file",
            ));
        }
        let metadata = hypr_fs_sync_core::media::metadata_from_reader(
            &filename,
            "application/octet-stream",
            &mut file,
        )
        .map_err(|cause| Error::operation("read media source", cause.to_string()))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|cause| Error::operation("read media source", cause.to_string()))?;
        Ok((
            filename,
            MediaUploadSource {
                sha256: metadata.sha256,
                size_bytes: metadata.size_bytes,
            },
            file,
        ))
    })
    .await
    .map_err(|cause| Error::operation("read media source", cause.to_string()))?;
    result
}
async fn wait(
    authority: &MediaAuthority,
    mut status: MediaRequestStatus,
) -> Result<MediaRequestStatus> {
    let request = status.declaration.request_id.clone();
    while matches!(status.phase, MediaPhase::Running | MediaPhase::Ready) {
        tokio::time::sleep(Duration::from_millis(300)).await;
        status = authority
            .status(&request)
            .await
            .map_err(|cause| original_error(cause, &request))?;
    }
    Ok(status)
}
fn terminal(status: &MediaRequestStatus) -> Result<()> {
    if status.phase == MediaPhase::Succeeded {
        return Ok(());
    }
    let request = &status.declaration.request_id;
    if let Some(cause) = status.media.as_ref().and_then(|m| m.error.clone()) {
        return Err(original_error(cause, request));
    }
    let reason = match status.phase {
        MediaPhase::Receiving => {
            "Original media request awaits source; explicitly recover it with --source FILE"
        }
        MediaPhase::Review => {
            "Original media request requires review; inspect its status and explicitly recover retained output"
        }
        MediaPhase::Cancelled => "Original media request was cancelled",
        _ => "Original media request did not complete",
    };
    Err(review(request, reason))
}
fn original_error(mut cause: ApplicationError, request: &str) -> Error {
    cause.request_id = Some(request.into());
    cause
        .mutation_outcome
        .get_or_insert(MutationOutcome::Unknown);
    cause.retryable = false;
    cause.message = format!(
        "{} (request {request}; inspect with 'loof --profile NAME sessions media {request}', recover with 'loof --profile NAME sessions recover {request}')",
        cause.message
    );
    cause.into()
}
fn review(request: &str, message: &str) -> Error {
    let mut cause = failure(ErrorCode::Unavailable, message);
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    original_error(cause, request)
}
fn announce(request: &str, json: bool) {
    if json {
        eprintln!(
            "{}",
            serde_json::json!({"phase":"media_declared","request_id":request})
        );
    } else {
        eprintln!("Media request {request}");
    }
}

#[cfg(test)]
#[path = "media_tests.rs"]
mod tests;

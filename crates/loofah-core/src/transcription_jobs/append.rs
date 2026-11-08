use super::*;
use futures_util::FutureExt;
use std::{fs::File, path::PathBuf};

pub(crate) struct CaptureSuffixInput {
    pub identity: CaptureIdentity,
    pub options: CaptureProcessingRequest,
    pub model: String,
    pub source_sha256: String,
    pub path: PathBuf,
    pub original: hypr_vault_write::recording_append::AppendSource,
}

struct Released(tokio::sync::watch::Sender<bool>);
impl Drop for Released {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
struct Owner {
    _file: File,
    _context: Arc<VaultContext>,
    _session: tokio::sync::OwnedRwLockReadGuard<()>,
    _original: hypr_vault_write::recording_append::AppendSource,
    _released: Released,
}

pub(crate) struct PreparedCaptureSuffix {
    prepared: Box<dyn PreparedTranscription>,
    request: TranscriptionJobRequest,
}
impl VaultContext {
    pub(crate) async fn prepare_capture_suffix_engine(
        self: &Arc<Self>,
        input: &CaptureSuffixInput,
        cancel: CancellationToken,
    ) -> Result<PreparedCaptureSuffix, ApplicationError> {
        let CaptureTarget::AppendSession { session_id, .. } = &input.identity.target else {
            return Err(unavailable());
        };
        // This DTO only supplies existing engine parameters. No absent-only job
        // is admitted and it grants no canonical publication authority.
        let request = TranscriptionJobRequest {
            session_id: session_id.clone(),
            expected_recording_sha256: input.source_sha256.clone(),
            expected_transcript: AbsentTranscriptExpectation::Absent,
            model: input.model.clone(),
            languages: input.options.languages.clone(),
            keywords: input.options.keywords.clone(),
        };
        let prepared = self
            .transcription_service()?
            .engine
            .prepare(&request, cancel)
            .await
            .map_err(|cause| {
                let mut error = error(
                    if matches!(
                        cause,
                        TranscriptionFailure::EngineUnavailable
                            | TranscriptionFailure::ResourceLimit
                    ) {
                        ErrorCode::Unavailable
                    } else {
                        ErrorCode::Conflict
                    },
                    &format!("Append engine preparation failed: {cause:?}"),
                );
                error.mutation_outcome = Some(MutationOutcome::NotCommitted);
                error
            })?;
        Ok(PreparedCaptureSuffix { prepared, request })
    }
    pub(crate) async fn transcribe_capture_suffix(
        self: &Arc<Self>,
        input: CaptureSuffixInput,
        prepared: PreparedCaptureSuffix,
        cancel: CancellationToken,
    ) -> Result<TranscriptWithData, ApplicationError> {
        let service = self.transcription_service()?;
        let CaptureTarget::AppendSession { session_id, .. } = &input.identity.target else {
            return Err(unavailable());
        };
        let expected_sha = input.source_sha256.clone();
        let original_path = input.path.clone();
        let PreparedCaptureSuffix { prepared, request } = prepared;
        if request.session_id != *session_id
            || request.expected_recording_sha256 != input.source_sha256
            || request.model != input.model
            || request.languages != input.options.languages
            || request.keywords != input.options.keywords
        {
            return Err(storage("Append engine admission changed"));
        }
        let work = TranscriptionWork {
            job_id: input.identity.capture_id,
            principal: input.identity.scope.member_id,
            request: request.clone(),
        };
        let access = service
            .authorizer
            .observe(&work)
            .await
            .map_err(engine_error)?;
        let stop = cancel.child_token();
        let stopping = stop.clone();
        let done = CancellationToken::new();
        let finished = done.clone();
        let access_cancelled = access.cancelled.clone();
        let access_interrupted = access.interrupted.clone();
        let shutdown = service.stop.clone();
        let bridge = tokio::spawn(async move {
            tokio::select! { _=access_cancelled.cancelled()=>stopping.cancel(), _=access_interrupted.cancelled()=>stopping.cancel(), _=shutdown.cancelled()=>stopping.cancel(), _=finished.cancelled()=>{} }
        });
        let result = async {
            let session = self.admit_session_work(session_id).await?;
            let note = self
                .read_note(session_id)
                .await?
                .markdown
                .unwrap_or_default();
            let memo = tokio::task::spawn_blocking(move || {
                if note.len() > 8 * 1024 * 1024 {
                    return Err(storage("Append note source exceeds the input budget"));
                }
                if note.is_empty() {
                    return Ok(String::new());
                }
                let value = hypr_tiptap::md_to_tiptap_json(&note).map_err(storage)?;
                let value = serde_json::to_string(&value).map_err(storage)?;
                if value.len() > 16 * 1024 * 1024 {
                    return Err(storage("Append note source exceeds the input budget"));
                }
                Ok(value)
            })
            .await
            .map_err(storage)??;
            let metadata = TranscriptionMetadata {
                session_id: session_id.clone(),
                user_id: work.principal.clone(),
                created_at: chrono::Utc::now().to_rfc3339(),
                memo_md: memo,
                audio: input.original.audio(),
                materialized: Arc::new(OnceLock::new()),
            };
            let (released, mut release) = tokio::sync::watch::channel(false);
            let owner = Owner {
                _file: File::open(&input.path).map_err(storage)?,
                _context: self.clone(),
                _session: session,
                _original: input.original,
                _released: Released(released),
            };
            let source = TrustedTranscriptionSource::capture_suffix(input.path, Box::new(owner));
            let inference = TranscriptionInput {
                source,
                metadata: metadata.clone(),
                request,
            };
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                prepared.transcribe(inference, stop.clone())
            })) {
                Ok(future) => std::panic::AssertUnwindSafe(future).catch_unwind().await,
                Err(panic) => Err(panic),
            };
            // A returned/panicking host future is not proof that native source I/O
            // ended. The opaque owner moves into the actual engine operation.
            while !*release.borrow() {
                if release.changed().await.is_err() {
                    break;
                }
            }
            let transcript = result
                .map_err(|_| storage("Append inference worker was lost"))?
                .map_err(engine_error)?;
            if stop.is_cancelled() {
                return Err(engine_error(TranscriptionFailure::Interrupted));
            }
            if transcript.session_id != metadata.session_id
                || transcript.user_id != metadata.user_id
                || transcript.created_at != metadata.created_at
                || transcript.memo_md != metadata.memo_md
                || transcript.started_at != metadata.started_at_ms()
                || transcript.ended_at.is_some()
                || uuid::Uuid::parse_str(&transcript.id).is_err()
            {
                return Err(engine_error(TranscriptionFailure::InvalidOutput));
            }
            let pin = self.clone();
            tokio::task::spawn_blocking(move || {
                let _pin = pin;
                crate::capture_append::verify_file_sha256(&original_path, &expected_sha)
            })
            .await
            .map_err(storage)??;
            let _grant = service
                .authorizer
                .authorize(&work)
                .await
                .map_err(engine_error)?;
            Ok(transcript)
        }
        .await;
        done.cancel();
        let _ = bridge.await;
        drop(access);
        result
    }
}
fn engine_error(cause: TranscriptionFailure) -> ApplicationError {
    let mut error = error(
        ErrorCode::Unavailable,
        &format!("Append suffix inference failed: {cause:?}"),
    );
    error.mutation_outcome = Some(MutationOutcome::Unknown);
    error
}

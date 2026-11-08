use super::*;
use futures_util::FutureExt;
use std::{fs::File, path::PathBuf};

pub(crate) struct LocalInferenceInput {
    pub request_id: String,
    pub session_id: String,
    pub options: crate::local_capture_intents::LocalTranscriptionOptions,
    pub source_sha256: String,
    pub path: PathBuf,
    pub audio: SessionAudio,
    pub owner: Box<dyn Send>,
}
pub(crate) struct PreparedLocalInference {
    pub(super) prepared: Box<dyn PreparedTranscription>,
    pub(super) request: TranscriptionJobRequest,
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
    _original: Box<dyn Send>,
    _released: Released,
}
impl VaultContext {
    pub(crate) async fn prepare_local_capture_engine(
        self: &Arc<Self>,
        input: LocalInferenceInput,
        cancel: CancellationToken,
    ) -> Result<(PreparedLocalInference, LocalInferenceInput), ApplicationError> {
        let request = TranscriptionJobRequest {
            session_id: input.session_id.clone(),
            expected_recording_sha256: input.source_sha256.clone(),
            expected_transcript: AbsentTranscriptExpectation::Absent,
            model: input.options.model.clone(),
            languages: input.options.languages.clone(),
            keywords: input.options.keywords.clone(),
        };
        let prepared = self
            .transcription_service()?
            .engine
            .prepare(&request, cancel)
            .await
            .map_err(engine_error)?;
        Ok((PreparedLocalInference { prepared, request }, input))
    }
    pub(crate) async fn transcribe_local_capture_source(
        self: &Arc<Self>,
        input: LocalInferenceInput,
        prepared: PreparedLocalInference,
        cancel: CancellationToken,
    ) -> Result<TranscriptWithData, ApplicationError> {
        let service = self.transcription_service()?;
        let PreparedLocalInference { prepared, request } = prepared;
        if request.session_id != input.session_id
            || request.expected_recording_sha256 != input.source_sha256
            || request.model != input.options.model
            || request.languages != input.options.languages
            || request.keywords != input.options.keywords
        {
            return Err(storage("Local inference declaration changed"));
        }
        let work = TranscriptionWork {
            job_id: input.request_id,
            principal: crate::local_capture_intents::TRANSCRIPTION_OWNER.into(),
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
        let revoked = access.cancelled.clone();
        let interrupted = access.interrupted.clone();
        let shutdown = service.stop.clone();
        let bridge = tokio::spawn(async move {
            tokio::select! {_=revoked.cancelled()=>stopping.cancel(),_=interrupted.cancelled()=>stopping.cancel(),_=shutdown.cancelled()=>stopping.cancel(),_=finished.cancelled()=>{}}
        });
        let result = async {
            let memo = local_memo(self, &input.session_id).await?;
            let metadata = TranscriptionMetadata {
                session_id: input.session_id,
                user_id: work.principal.clone(),
                created_at: chrono::Utc::now().to_rfc3339(),
                memo_md: memo,
                audio: input.audio,
                materialized: Arc::new(OnceLock::new()),
            };
            let (released, mut release) = tokio::sync::watch::channel(false);
            let owner = Owner {
                _file: File::open(&input.path).map_err(storage)?,
                _context: self.clone(),
                _original: input.owner,
                _released: Released(released),
            };
            let path = input.path.clone();
            let sha = input.source_sha256;
            let inference = TranscriptionInput {
                source: TrustedTranscriptionSource::capture_suffix(input.path, Box::new(owner)),
                metadata: metadata.clone(),
                request,
            };
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                prepared.transcribe(inference, stop.clone())
            })) {
                Ok(future) => std::panic::AssertUnwindSafe(future).catch_unwind().await,
                Err(panic) => Err(panic),
            };
            while !*release.borrow() {
                if release.changed().await.is_err() {
                    break;
                }
            }
            let transcript = result
                .map_err(|_| storage("Local inference worker lost"))?
                .map_err(engine_error)?;
            if stop.is_cancelled() {
                return Err(engine_error(TranscriptionFailure::Interrupted));
            }
            ledger::validate_result(&transcript)?;
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
                crate::capture_append::verify_file_sha256(&path, &sha)
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
pub(crate) async fn local_memo(
    context: &VaultContext,
    session: &str,
) -> Result<String, ApplicationError> {
    let note = context
        .read_note(session)
        .await?
        .markdown
        .unwrap_or_default();
    tokio::task::spawn_blocking(move || {
        if note.len() > 8 * 1024 * 1024 {
            return Err(capacity());
        }
        if note.is_empty() {
            return Ok(String::new());
        }
        let json = hypr_tiptap::md_to_tiptap_json(&note).map_err(storage)?;
        let memo = serde_json::to_string(&json).map_err(storage)?;
        if memo.len() > 16 * 1024 * 1024 {
            return Err(capacity());
        }
        Ok(memo)
    })
    .await
    .map_err(storage)?
}
fn engine_error(cause: TranscriptionFailure) -> ApplicationError {
    error(
        ErrorCode::Unavailable,
        &format!("Local transcription failed: {cause:?}"),
    )
}

use hypr_loofah_core::{
    VaultContext,
    local_cli::{LocalCli, LocalCliArtifact, LocalMediaFuture, LocalMediaHost},
    manual_media_intents::{ManualMediaKind, ManualMediaSummaryDeclaration},
    settings::AppConfig,
};
use hypr_loofah_protocol::LocalMediaState as MediaState;
use hypr_loofah_protocol::*;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};
use tokio_util::sync::CancellationToken;

pub(crate) mod commands;
mod source;
#[cfg(test)]
mod tests;

#[derive(Default)]
pub struct LocalMediaState {
    service: OnceLock<Arc<Service>>,
    stop: CancellationToken,
}
pub(crate) struct Service {
    pub context: Arc<VaultContext>,
    pub cli: Arc<LocalCli>,
    host: Arc<NativeMediaHost>,
    tasks: Mutex<HashMap<String, Arc<Task>>>,
    stop: CancellationToken,
}
struct Task {
    fingerprint: String,
    status: Mutex<LocalMediaStatus>,
    admitted: CancellationToken,
    done: CancellationToken,
    cancel: CancellationToken,
    safe: std::sync::atomic::AtomicBool,
}
pub(crate) struct NativeMediaHost {
    context: Arc<VaultContext>,
    stop: CancellationToken,
    requests: Mutex<HashMap<String, CancellationToken>>,
}
impl LocalMediaHost for NativeMediaHost {
    fn cancellation(&self, request: &str) -> CancellationToken {
        self.requests
            .lock()
            .ok()
            .and_then(|tasks| tasks.get(request).cloned())
            .unwrap_or_else(|| self.stop.child_token())
    }
    fn normalize(
        &self,
        source: LocalCliArtifact,
        filename: String,
        temporary: PathBuf,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<LocalCliArtifact> {
        hypr_loofah_local_media::LocalMediaEngine::new()
            .normalize(source, filename, temporary, progress)
    }
    fn transcribe(
        &self,
        request_id: String,
        source: LocalCliArtifact,
        filename: String,
        session_id: String,
        audio: hypr_vault_read::SessionAudio,
        config: AppConfig,
        temporary: PathBuf,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<TranscriptWithData> {
        let context = self.context.clone();
        let stop = self.cancellation(&request_id);
        Box::pin(async move {
            progress(LocalCliProgress {
                schema_version: "2".into(),
                phase: "transcribing audio".into(),
                processed: 0,
                total: None,
                added: 0,
                updated: 0,
                deleted: 0,
                elapsed_ms: 0,
            });
            context
                .transcribe_local_media_artifact(
                    request_id, source, filename, session_id, audio, config, temporary, stop,
                )
                .await
        })
    }
}
impl LocalMediaState {
    pub fn initialized(&self) -> bool {
        self.service.get().is_some()
    }
    pub fn initialize(&self, context: Arc<VaultContext>) -> Result<(), ApplicationError> {
        if self.stop.is_cancelled() {
            return Err(unavailable());
        }
        if let Some(service) = self.service.get() {
            return if Arc::ptr_eq(&service.context, &context) {
                Ok(())
            } else {
                Err(unavailable())
            };
        }
        let host = Arc::new(NativeMediaHost {
            context: context.clone(),
            stop: self.stop.clone(),
            requests: Mutex::new(HashMap::new()),
        });
        let cli = Arc::new(LocalCli::new(context.clone())?.with_media_host(host.clone()));
        let service = Arc::new(Service {
            context,
            cli,
            host,
            tasks: Mutex::new(HashMap::new()),
            stop: self.stop.clone(),
        });
        self.service.set(service).map_err(|_| unavailable())
    }
    pub(crate) fn service_for(
        &self,
        context: &Arc<VaultContext>,
    ) -> Result<Arc<Service>, ApplicationError> {
        self.service
            .get()
            .filter(|service| !self.stop.is_cancelled() && Arc::ptr_eq(&service.context, context))
            .cloned()
            .ok_or_else(unavailable)
    }
    pub fn request_shutdown(&self) {
        self.stop.cancel();
    }
    pub async fn drain(&self, deadline: std::time::Duration) -> bool {
        self.request_shutdown();
        let Some(service) = self.service.get() else {
            return true;
        };
        let tasks = match service.tasks.lock() {
            Ok(tasks) => tasks.values().cloned().collect::<Vec<_>>(),
            Err(_) => return false,
        };
        tokio::time::timeout(deadline, async {
            for task in tasks {
                task.done.cancelled().await;
                if !task.safe.load(std::sync::atomic::Ordering::Acquire) {
                    return false;
                }
            }
            true
        })
        .await
        .unwrap_or(false)
    }
    pub(crate) fn media_host(
        &self,
        context: &Arc<VaultContext>,
    ) -> Result<Arc<dyn LocalMediaHost>, ApplicationError> {
        Ok(self.service_for(context)?.host.clone())
    }
}
impl Service {
    pub async fn submit(
        self: &Arc<Self>,
        request: String,
        operation: LocalCliOperation,
        artifact: Option<LocalCliArtifact>,
    ) -> Result<LocalMediaStatus, ApplicationError> {
        validate_request(&request)?;
        let (session, kind, fresh) = match &operation {
            LocalCliOperation::Import { into, .. } => (
                match into {
                    Some(id) => id.clone(),
                    None => self
                        .context
                        .created_session_id("local-cli-import", &request)?,
                },
                LocalMediaKind::ImportAudio,
                into.is_none(),
            ),
            LocalCliOperation::ImportIntoChecked { session_id, .. } => {
                (session_id.clone(), LocalMediaKind::ImportAudio, false)
            }
            LocalCliOperation::TranscribeChecked { session_id, .. } => {
                (session_id.clone(), LocalMediaKind::Transcribe, false)
            }
            LocalCliOperation::ImportSubtitles { session_id, .. } => {
                (session_id.clone(), LocalMediaKind::ImportSubtitles, false)
            }
            _ => return Err(invalid("Unsupported manual media operation")),
        };
        validate_session_id(&session).map_err(|_| invalid("Invalid media session"))?;
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&(
            &operation,
            artifact
                .as_ref()
                .map(|input| (&input.sha256, input.size_bytes)),
        ))
        .map_err(|_| invalid("Invalid media declaration"))?;
        let fingerprint = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let task = {
            let mut tasks = self.tasks.lock().map_err(|_| unavailable())?;
            if self.stop.is_cancelled() {
                return Err(unavailable());
            }
            if let Some(task) = tasks.get(&request) {
                if task.fingerprint != fingerprint {
                    return Err(invalid("Original media request changed"));
                }
                Some(task.clone())
            } else {
                tasks.retain(|_, task| {
                    !task.done.is_cancelled()
                        || !task.safe.load(std::sync::atomic::Ordering::Acquire)
                });
                if tasks.len() >= 16 {
                    return Err(unavailable());
                }
                None
            }
        };
        if let Some(task) = task {
            return wait_admission(task).await;
        }
        let task = Arc::new(Task {
            fingerprint,
            status: Mutex::new(LocalMediaStatus {
                request_id: request.clone(),
                session_id: session.clone(),
                kind,
                state: MediaState::Running,
                transcript_id: None,
                word_count: None,
                error: None,
            }),
            admitted: CancellationToken::new(),
            done: CancellationToken::new(),
            cancel: self.stop.child_token(),
            safe: std::sync::atomic::AtomicBool::new(false),
        });
        // Insert and launch without awaiting so shutdown cannot observe an unowned request.
        {
            let mut tasks = self.tasks.lock().map_err(|_| unavailable())?;
            if self.stop.is_cancelled() {
                return Err(unavailable());
            }
            if tasks.contains_key(&request) {
                return Err(invalid("Original media request is already being admitted"));
            }
            self.host
                .requests
                .lock()
                .map_err(|_| unavailable())?
                .insert(request.clone(), task.cancel.clone());
            tasks.insert(request.clone(), task.clone());
        }
        let expected_created_at = match &operation {
            LocalCliOperation::ImportIntoChecked { created_at, .. } => created_at.clone(),
            _ => None,
        };
        let service = self.clone();
        let owned = task.clone();
        tokio::spawn(async move {
            use futures_util::FutureExt;
            let result = std::panic::AssertUnwindSafe(async {
                let config = service.context.settings.snapshot();
                let summary = config
                    .current_llm_provider
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .zip(
                        config
                            .current_llm_model
                            .as_deref()
                            .filter(|value| !value.trim().is_empty()),
                    )
                    .map(|_| CaptureProcessingSummary {
                        target: DocumentTarget::Summary,
                        title_policy: TitlePolicy::GenerateIfEmpty,
                    });
                service
                    .context
                    .declare_manual_media_summary(ManualMediaSummaryDeclaration {
                        request_id: request.clone(),
                        session_id: session,
                        fresh_session: fresh,
                        expected_created_at,
                        kind: match kind {
                            LocalMediaKind::ImportAudio => ManualMediaKind::AudioImport,
                            LocalMediaKind::Transcribe => ManualMediaKind::Transcription,
                            LocalMediaKind::ImportSubtitles => ManualMediaKind::Subtitles,
                        },
                        summary,
                        only_if_empty: kind != LocalMediaKind::ImportSubtitles,
                    })
                    .await?;
                let admitted = owned.admitted.clone();
                let result = service
                    .cli
                    .execute(
                        request.clone(),
                        operation,
                        artifact,
                        Arc::new(move |_| admitted.cancel()),
                    )
                    .await;
                match result {
                    Ok(_) => {
                        // Summary admission failure cannot roll back retained raw media or transcript.
                        let _ = service
                            .context
                            .complete_manual_media_summary(request.clone())
                            .await;
                        service.persisted_status(&request, Some(kind)).await
                    }
                    Err(cause) => Err(cause),
                }
            })
            .catch_unwind()
            .await;
            let safe = result.is_ok();
            if let Ok(mut status) = owned.status.lock() {
                match result {
                    Ok(Ok(stored)) => *status = stored,
                    Ok(Err(cause)) => set_failure(&mut status, cause),
                    Err(_) => set_failure(
                        &mut status,
                        unknown(
                            "Original media worker lost; inspect retained status before recovery",
                        ),
                    ),
                }
            }
            owned.safe.store(safe, std::sync::atomic::Ordering::Release);
            owned.done.cancel();
            if let Ok(mut requests) = service.host.requests.lock() {
                requests.remove(&request);
            }
        });
        wait_admission(task).await
    }
    async fn persisted_status(
        &self,
        request: &str,
        kind: Option<LocalMediaKind>,
    ) -> Result<LocalMediaStatus, ApplicationError> {
        match kind {
            Some(LocalMediaKind::ImportSubtitles) => self.cli.subtitle_status(request).await,
            Some(_) => self.cli.media_status(request).await,
            None => match self.cli.media_status(request).await {
                Ok(status) => Ok(status),
                Err(cause) if cause.code == ErrorCode::NotFound => {
                    self.cli.subtitle_status(request).await
                }
                Err(cause) => Err(cause),
            },
        }
    }
    pub async fn status(&self, request: &str) -> Result<LocalMediaStatus, ApplicationError> {
        validate_request(request)?;
        if let Some(task) = self
            .tasks
            .lock()
            .map_err(|_| unavailable())?
            .get(request)
            .cloned()
        {
            return task
                .status
                .lock()
                .map(|value| value.clone())
                .map_err(|_| unavailable());
        }
        self.persisted_status(request, None).await
    }
    pub async fn list(
        &self,
        session: Option<&str>,
    ) -> Result<Vec<LocalMediaStatus>, ApplicationError> {
        let mut statuses = self.cli.media_statuses(session).await?;
        statuses.extend(self.cli.subtitle_statuses(session).await?);
        let tasks = self.tasks.lock().map_err(|_| unavailable())?;
        for task in tasks.values() {
            let status = task.status.lock().map_err(|_| unavailable())?.clone();
            if session.is_some_and(|id| id != status.session_id) {
                continue;
            }
            statuses.retain(|value| value.request_id != status.request_id);
            statuses.push(status);
        }
        statuses.sort_by(|a, b| a.request_id.cmp(&b.request_id));
        Ok(statuses)
    }
    pub async fn cancel(&self, request: &str) -> Result<LocalMediaStatus, ApplicationError> {
        validate_request(request)?;
        if let Some(task) = self
            .tasks
            .lock()
            .map_err(|_| unavailable())?
            .get(request)
            .cloned()
        {
            task.cancel.cancel();
        }
        self.status(request).await
    }
    pub async fn recover(
        self: &Arc<Self>,
        request: String,
    ) -> Result<LocalMediaStatus, ApplicationError> {
        let mut status = self.status(&request).await?;
        if status.state == MediaState::Running {
            return Ok(status);
        }
        let kind = status.kind;
        let operation = if kind == LocalMediaKind::ImportSubtitles {
            LocalCliOperation::RecoverSubtitles {
                request_id: request.clone(),
            }
        } else {
            LocalCliOperation::RecoverMedia {
                request_id: request.clone(),
            }
        };
        status.state = MediaState::Running;
        status.error = None;
        let task = Arc::new(Task {
            fingerprint: String::new(),
            status: Mutex::new(status),
            admitted: CancellationToken::new(),
            done: CancellationToken::new(),
            cancel: self.stop.child_token(),
            safe: std::sync::atomic::AtomicBool::new(false),
        });
        {
            let mut tasks = self.tasks.lock().map_err(|_| unavailable())?;
            if self.stop.is_cancelled() {
                return Err(unavailable());
            }
            if tasks
                .get(&request)
                .is_some_and(|old| !old.done.is_cancelled())
            {
                return Err(unavailable());
            }
            tasks.insert(request.clone(), task.clone());
        }
        let service = self.clone();
        let owned = task.clone();
        tokio::spawn(async move {
            use futures_util::FutureExt;
            let result = std::panic::AssertUnwindSafe(async {
                service
                    .cli
                    .execute(
                        uuid::Uuid::new_v4().to_string(),
                        operation,
                        None,
                        Arc::new(|_| {}),
                    )
                    .await?;
                let _ = service
                    .context
                    .complete_manual_media_summary(request.clone())
                    .await;
                service.persisted_status(&request, Some(kind)).await
            })
            .catch_unwind()
            .await;
            let safe = result.is_ok();
            if let Ok(mut status) = owned.status.lock() {
                match result {
                    Ok(Ok(stored)) => *status = stored,
                    Ok(Err(cause)) => set_failure(&mut status, cause),
                    Err(_) => {
                        set_failure(&mut status, unknown("Original media recovery worker lost"))
                    }
                }
            }
            owned.safe.store(safe, std::sync::atomic::Ordering::Release);
            owned.done.cancel();
        });
        wait_admission(task).await
    }
}
async fn wait_admission(task: Arc<Task>) -> Result<LocalMediaStatus, ApplicationError> {
    tokio::select! {_=task.admitted.cancelled()=>{},_=task.done.cancelled()=>{}}
    let status = task.status.lock().map_err(|_| unavailable())?.clone();
    if !task.admitted.is_cancelled()
        && let Some(cause) = status.error
    {
        return Err(cause);
    }
    Ok(status)
}
fn set_failure(status: &mut LocalMediaStatus, cause: ApplicationError) {
    status.state = if cause.mutation_outcome == Some(MutationOutcome::Unknown) {
        MediaState::Review
    } else if cause.code == ErrorCode::Cancelled {
        MediaState::Cancelled
    } else {
        MediaState::Failed
    };
    status.error = Some(cause);
}
pub(crate) fn validate_request(request: &str) -> Result<(), ApplicationError> {
    uuid::Uuid::parse_str(request)
        .ok()
        .filter(|id| !id.is_nil() && id.to_string() == request)
        .map(|_| ())
        .ok_or_else(|| invalid("Original media request UUID is required"))
}
fn unavailable() -> ApplicationError {
    hypr_loofah_client::failure(
        ErrorCode::Unavailable,
        "Original Local media owner is unavailable",
    )
}
fn invalid(message: &str) -> ApplicationError {
    hypr_loofah_client::failure(ErrorCode::Validation, message)
}
fn unknown(message: &str) -> ApplicationError {
    let mut cause = hypr_loofah_client::failure(ErrorCode::Unavailable, message);
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    cause
}

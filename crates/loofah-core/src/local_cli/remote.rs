use super::*;
use crate::{settings::AppConfig, transcription_jobs::ledger};
use std::{collections::HashMap, path::PathBuf};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

mod journal;
mod retirement;
mod source;
pub use source::MediaSourceReader;
mod status;
mod summary;
#[cfg(test)]
mod tests;
use journal::Admission;
pub(crate) use journal::sample_media_uploads;

#[derive(Clone, Copy)]
pub struct MediaLimits {
    pub source_bytes: u64,
    pub retained_bytes: u64,
    pub requests: usize,
}
impl Default for MediaLimits {
    fn default() -> Self {
        Self {
            source_bytes: 8 * 1024 * 1024 * 1024,
            retained_bytes: 32 * 1024 * 1024 * 1024,
            requests: 256,
        }
    }
}
struct Task {
    cancel: CancellationToken,
    join: tokio::task::JoinHandle<()>,
}
pub struct MediaDispatcher {
    context: Arc<VaultContext>,
    principal: String,
    authorization: Arc<dyn scope::MediaAuthorizer>,
    root: PathBuf,
    limits: MediaLimits,
    capacity: Arc<tokio::sync::Semaphore>,
    admission: Mutex<()>,
    tasks: Mutex<HashMap<String, Task>>,
    request_gates: Mutex<HashMap<String, std::sync::Weak<Mutex<()>>>>,
    journal_gates: Mutex<HashMap<String, std::sync::Weak<tokio::sync::RwLock<()>>>>,
    receivers: Mutex<HashMap<String, (CancellationToken, tokio::sync::watch::Receiver<bool>)>>,
    stop: CancellationToken,
    source_reads: Arc<tokio::sync::Semaphore>,
    source_gates: Mutex<HashMap<String, std::sync::Weak<tokio::sync::RwLock<()>>>>,
    #[cfg(test)]
    source_write_pause: std::sync::Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
    #[cfg(test)]
    retirement_pause: std::sync::Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
    #[cfg(test)]
    declaration_pause: std::sync::Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}
impl MediaDispatcher {
    pub fn new(
        context: Arc<VaultContext>,
        principal: String,
        authorization: Arc<dyn scope::MediaAuthorizer>,
        limits: MediaLimits,
    ) -> Result<Arc<Self>, ApplicationError> {
        Self::with_capacity(
            context,
            principal,
            authorization,
            limits,
            Arc::new(tokio::sync::Semaphore::new(2)),
        )
    }
    pub fn with_capacity(
        context: Arc<VaultContext>,
        principal: String,
        authorization: Arc<dyn scope::MediaAuthorizer>,
        limits: MediaLimits,
        capacity: Arc<tokio::sync::Semaphore>,
    ) -> Result<Arc<Self>, ApplicationError> {
        let scope = Arc::new(scope::MediaExecutionScope::new(
            &context.events.vault_id(),
            principal.clone(),
            AppConfig::default(),
            authorization.clone(),
        )?);
        let cli = LocalCli::new(context.clone())?.with_media_scope(scope);
        let root = context
            .operations
            .as_ref()
            .ok_or_else(|| unavailable("Media storage unavailable"))?
            .root
            .join(cli.media_directory("local-media"));
        Ok(Arc::new(Self {
            context,
            principal,
            authorization,
            root,
            limits,
            capacity,
            admission: Mutex::new(()),
            tasks: Mutex::new(HashMap::new()),
            request_gates: Mutex::new(HashMap::new()),
            journal_gates: Mutex::new(HashMap::new()),
            receivers: Mutex::new(HashMap::new()),
            stop: CancellationToken::new(),
            source_reads: Arc::new(tokio::sync::Semaphore::new(4)),
            source_gates: Default::default(),
            #[cfg(test)]
            source_write_pause: Default::default(),
            #[cfg(test)]
            retirement_pause: Default::default(),
            #[cfg(test)]
            declaration_pause: Default::default(),
        }))
    }
    fn cli(
        &self,
        input: &MediaDeclaration,
        stop: CancellationToken,
    ) -> Result<Arc<LocalCli>, ApplicationError> {
        let config = AppConfig {
            current_stt_provider: Some("server".into()),
            current_stt_model: input.transcription.as_ref().map(|t| t.model.clone()),
            meeting_languages: input.transcription.as_ref().map(|t| t.languages.clone()),
            ..Default::default()
        };
        let mut scope = scope::MediaExecutionScope::new(
            &self.context.events.vault_id(),
            self.principal.clone(),
            config,
            self.authorization.clone(),
        )?;
        scope.baseline = match &input.operation {
            MediaOperation::ImportAudio { expected, .. } => expected.clone(),
            MediaOperation::ImportSubtitles { expected, .. } => Some(expected.clone()),
            _ => None,
        };
        Ok(Arc::new(
            LocalCli::new(self.context.clone())?
                .with_media_scope(Arc::new(scope))
                .with_media_host(Arc::new(PortableHost {
                    context: self.context.clone(),
                    principal: self.principal.clone(),
                    stop,
                })),
        ))
    }
    pub async fn declare(
        self: &Arc<Self>,
        input: MediaDeclaration,
    ) -> Result<MediaAdmission, ApplicationError> {
        validate_media_declaration(&input).map_err(|s| error(ErrorCode::Validation, s))?;
        if self.stop.is_cancelled() {
            return Err(unavailable("Media service is stopping"));
        }
        let _request_gate = self.request_gate(&input.request_id).await?;
        let _gate = self.admission.lock().await;
        drop(self.authorization.authorize().await?);
        if let Some(existing) = self.load(&input.request_id).await? {
            if existing.declaration != input {
                return Err(conflict("Original media declaration changed"));
            }
            return self.admission_status(existing).await;
        }
        if input
            .operation
            .upload()
            .is_some_and(|(_, source)| source.size_bytes > self.limits.source_bytes)
        {
            return Err(error(
                ErrorCode::Validation,
                "Media source exceeds the service budget",
            ));
        }
        let records = self.inventory().await?;
        let total = records
            .iter()
            .filter_map(|r| r.declaration.operation.upload())
            .try_fold(0_u64, |total, (_, source)| {
                total.checked_add(source.size_bytes)
            })
            .ok_or_else(|| unavailable("Media source quota exhausted"))?;
        let size = input
            .operation
            .upload()
            .map_or(0, |(_, source)| source.size_bytes);
        if records.len() >= self.limits.requests
            || total.saturating_add(size) > self.limits.retained_bytes
        {
            return Err(unavailable("Media source quota exhausted"));
        }
        self.check_baseline(&input).await?;
        let model_sha256 = self.model_binding(&input)?;
        let mut record = Admission::new(
            self.context.events.vault_id(),
            self.principal.clone(),
            input,
            model_sha256,
        );
        let ready = record.phase;
        record.phase = MediaPhase::Review;
        self.save(record.clone(), true).await?;
        self.declare_summary(&record.declaration).await?;
        record.phase = ready;
        self.save(record.clone(), false).await?;
        #[cfg(test)]
        {
            let pause = self.declaration_pause.lock().unwrap().take();
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _ = release.await;
            }
        }
        if record.phase == MediaPhase::Ready {
            self.dispatch(record.clone()).await?;
        }
        self.admission_status(record).await
    }
    fn model_binding(&self, input: &MediaDeclaration) -> Result<Option<String>, ApplicationError> {
        let Some(options) = &input.transcription else {
            return Ok(None);
        };
        self.context
            .operator_transcription_model()
            .filter(|model| model.alias == options.model)
            .map(|model| Some(model.sha256))
            .ok_or_else(|| unavailable("Original media transcription model is unavailable"))
    }
    fn check_model_binding(&self, record: &Admission) -> Result<(), ApplicationError> {
        if record.declaration.transcription.is_some() && record.model_sha256.is_none()
            || self.model_binding(&record.declaration)? != record.model_sha256
        {
            return Err(conflict(
                "Original media transcription model changed; retained source requires review",
            ));
        }
        Ok(())
    }
    async fn check_baseline(&self, input: &MediaDeclaration) -> Result<(), ApplicationError> {
        match &input.operation {
            MediaOperation::ImportAudio {
                session_id: Some(id),
                expected: Some(expected),
                ..
            } => {
                if expected.recording.is_some() || &self.source(id).await? != expected {
                    return Err(conflict(
                        "Original import target changed or already has audio",
                    ));
                }
            }
            MediaOperation::ImportAudio {
                session_id: None, ..
            } => {
                let cli = self.cli(input, self.stop.child_token())?;
                let id = self.context.created_session_id(
                    &cli.media_namespace("local-cli-import"),
                    &input.request_id,
                )?;
                match self.context.read_metadata(&id).await {
                    Err(cause) if cause.code == ErrorCode::NotFound => {}
                    _ => return Err(conflict("Fresh media session identity already exists")),
                }
            }
            MediaOperation::Transcribe { expected } => {
                if self.source(&expected.session_id).await?.recording.as_ref() != Some(expected) {
                    return Err(conflict("Original transcription source changed"));
                }
            }
            MediaOperation::ImportSubtitles {
                session_id,
                expected,
                ..
            } => {
                if &self.source(session_id).await? != expected {
                    return Err(conflict("Original subtitle source changed"));
                }
            }
            _ => return Err(error(ErrorCode::Validation, "Invalid media declaration")),
        }
        Ok(())
    }
    async fn dispatch(self: &Arc<Self>, mut record: Admission) -> Result<(), ApplicationError> {
        if self.stop.is_cancelled() {
            return Err(unavailable("Media service is stopping"));
        }
        record.ensure_not_retiring()?;
        if record.phase != MediaPhase::Ready {
            return Err(conflict("Original media request cannot be dispatched"));
        }
        let checked = async {
            self.check_model_binding(&record)?;
            self.check_baseline(&record.declaration).await
        }
        .await;
        if let Err(cause) = checked {
            record.phase = MediaPhase::Review;
            record.refusal = Some(cause.clone());
            self.save(record, false).await?;
            return Err(cause);
        }
        let operation = operation(&record.declaration)?;
        self.start_task(record, operation, true).await
    }
    async fn start_task(
        self: &Arc<Self>,
        mut record: Admission,
        operation: LocalCliOperation,
        needs_source: bool,
    ) -> Result<(), ApplicationError> {
        record.ensure_not_retiring()?;
        let access = self.authorization.observe().await?;
        let cancel = self.stop.child_token();
        let cli = self.cli(&record.declaration, cancel.clone())?;
        record.refusal = None;
        record.phase = MediaPhase::Running;
        if record.dispatch.is_some() {
            record.dispatch = Some(journal::DispatchEvidence::Dispatched);
        }
        self.save(record.clone(), false).await?;
        let owner = self.clone();
        let request = record.declaration.request_id.clone();
        let request_key = request.clone();
        let stop = cancel.clone();
        let cancel_work = cancel.clone();
        let mut tasks = self.tasks.lock().await;
        if self.stop.is_cancelled() {
            return Err(unavailable("Media service is stopping"));
        }
        if tasks
            .get(&request_key)
            .is_some_and(|task| !task.join.is_finished())
        {
            return Err(conflict("Original media work is still draining"));
        }
        let join = tokio::spawn(async move {
            let revoked = access.cancelled.clone();
            let interrupted = access.interrupted.clone();
            let done = CancellationToken::new();
            let finished = done.clone();
            let watcher = tokio::spawn(async move {
                tokio::select! { _ = revoked.cancelled() => stop.cancel(), _ = interrupted.cancelled() => stop.cancel(), _ = finished.cancelled() => {} }
            });
            let _result: Result<LocalCliOutput, ApplicationError> = async {
                let permit = tokio::select! {
                    _ = cancel_work.cancelled() => return Err(error(ErrorCode::Cancelled, "Media request stopped before dispatch")),
                    permit = owner.capacity.clone().acquire_owned() => permit.map_err(|_| unavailable("Media capacity stopped"))?,
                };
                let _permit = permit;
                let artifact = if needs_source { owner.artifact(&record).await? } else { None };
                cli.execute(request.clone(), operation, artifact, Arc::new(|_| {})).await
            }.await;
            if _result.is_ok() {
                let _summary = owner.complete_summary(&request).await;
            }
            done.cancel();
            let _ = watcher.await;
            drop(access);
            record.phase = if _result.as_ref().is_err_and(|cause| {
                cause.code == ErrorCode::Cancelled
                    && cause.mutation_outcome != Some(MutationOutcome::Unknown)
            }) {
                MediaPhase::Cancelled
            } else {
                MediaPhase::Review
            };
            if let Err(cause) = _result {
                record.refusal = Some(cause);
            }
            // Canonical media/subtitle receipts remain the sole result authority.
            let _ = owner.save(record, false).await;
        });
        tasks.insert(request_key, Task { cancel, join });
        Ok(())
    }
    async fn journal_gate(&self, request: &str) -> Arc<tokio::sync::RwLock<()>> {
        let mut gates = self.journal_gates.lock().await;
        gates.retain(|_, gate| gate.strong_count() > 0);
        gates
            .get(request)
            .and_then(std::sync::Weak::upgrade)
            .unwrap_or_else(|| {
                let gate = Arc::new(tokio::sync::RwLock::new(()));
                gates.insert(request.into(), Arc::downgrade(&gate));
                gate
            })
    }
    async fn request_mutex(&self, request: &str) -> Result<Arc<Mutex<()>>, ApplicationError> {
        journal::validate_request(request)?;
        let mut gates = self.request_gates.lock().await;
        gates.retain(|_, gate| gate.strong_count() > 0);
        if !gates.contains_key(request) && gates.len() >= self.limits.requests {
            return Err(unavailable("Media request capacity exhausted"));
        }
        Ok(gates
            .get(request)
            .and_then(std::sync::Weak::upgrade)
            .unwrap_or_else(|| {
                let gate = Arc::new(Mutex::new(()));
                gates.insert(request.into(), Arc::downgrade(&gate));
                gate
            }))
    }
    async fn request_gate(
        &self,
        request: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, ApplicationError> {
        Ok(self.request_mutex(request).await?.lock_owned().await)
    }
    async fn try_request_gate(
        &self,
        request: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, ApplicationError> {
        self.request_mutex(request)
            .await?
            .try_lock_owned()
            .map_err(|_| conflict("Original media work is still in use"))
    }
    pub fn request_shutdown(&self) {
        self.stop.cancel();
    }
    pub async fn shutdown(&self) {
        self.request_shutdown();
        let _source_drain = self.source_reads.acquire_many(4).await;
        let receivers = std::mem::take(&mut *self.receivers.lock().await);
        for (_, (_, mut done)) in receivers {
            while !*done.borrow() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
        let tasks = std::mem::take(&mut *self.tasks.lock().await);
        for (_, task) in tasks {
            let _ = task.join.await;
        }
    }
}
fn operation(input: &MediaDeclaration) -> Result<LocalCliOperation, ApplicationError> {
    Ok(match &input.operation {
        MediaOperation::ImportAudio {
            session_id,
            expected,
            filename,
            content_type,
            created_at,
            initial,
            transcribe,
            ..
        } => {
            let extension = hypr_fs_sync_core::audio::import_source_extension(
                filename,
                content_type.as_deref(),
            );
            let filename = format!("source.{extension}");
            match (session_id, expected) {
                (Some(id), Some(expected)) => LocalCliOperation::ImportIntoChecked {
                    session_id: id.clone(),
                    filename,
                    transcribe: *transcribe,
                    expected_metadata_revision: expected.metadata.revision.clone(),
                    created_at: normalized_date(created_at)?,
                },
                (None, None) => {
                    let mut metadata = initial
                        .as_ref()
                        .map(|initial| initial.metadata.clone())
                        .unwrap_or_default();
                    metadata.created_at =
                        normalized_date(&created_at.clone().or(metadata.created_at))?;
                    LocalCliOperation::Import {
                        filename,
                        title: initial
                            .as_ref()
                            .and_then(|initial| initial.title.clone())
                            .or_else(|| {
                                Some(
                                    input
                                        .operation
                                        .upload()
                                        .unwrap()
                                        .0
                                        .rsplit_once('.')
                                        .map_or(input.operation.upload().unwrap().0, |(base, _)| {
                                            base
                                        })
                                        .into(),
                                )
                            }),
                        into: None,
                        transcribe: *transcribe,
                        metadata,
                    }
                }
                _ => return Err(conflict("Original import baseline unavailable")),
            }
        }
        MediaOperation::Transcribe { expected } => LocalCliOperation::TranscribeChecked {
            session_id: expected.session_id.clone(),
            expected: expected.clone(),
        },
        MediaOperation::ImportSubtitles {
            session_id,
            filename,
            ..
        } => LocalCliOperation::ImportSubtitles {
            session_id: session_id.clone(),
            filename: filename.clone(),
        },
    })
}
struct PortableHost {
    context: Arc<VaultContext>,
    principal: String,
    stop: CancellationToken,
}
impl LocalMediaHost for PortableHost {
    fn cancellation(&self, _: &str) -> CancellationToken {
        self.stop.clone()
    }
    fn normalize(
        &self,
        source: LocalCliArtifact,
        filename: String,
        temporary: PathBuf,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<LocalCliArtifact> {
        normalize_media_artifact(source, filename, temporary, progress)
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
        _: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<LocalMediaTranscription> {
        let context = self.context.clone();
        let principal = self.principal.clone();
        let cancel = self.stop.clone();
        Box::pin(async move {
            context
                .transcribe_scoped_media_artifact(
                    principal, request_id, source, filename, session_id, audio, config, temporary,
                    cancel,
                )
                .await
        })
    }
}
fn unavailable(message: &str) -> ApplicationError {
    error(ErrorCode::Unavailable, message)
}
fn conflict(message: &str) -> ApplicationError {
    error(ErrorCode::Conflict, message)
}

fn normalized_date(value: &Option<String>) -> Result<Option<String>, ApplicationError> {
    value
        .as_deref()
        .map(|value| {
            chrono::DateTime::parse_from_rfc3339(value)
                .map(|date| {
                    date.with_timezone(&chrono::Utc)
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                })
                .map_err(|_| error(ErrorCode::Validation, "Invalid imported audio date"))
        })
        .transpose()
}

//! Local capture declarations survive the producer, renderer and process. Only
//! trusted native completion tokens can admit post-processing of their source.
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use hypr_loofah_protocol::{
    ApplicationError, CaptureProcessingSummary, ErrorCode, SummaryJobRequest,
};
use hypr_vault_write::transcript_publication::ProcessingSourceBinding;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify, OwnedRwLockReadGuard};
use tokio_util::sync::CancellationToken;

use crate::{
    VaultContext,
    transcription_jobs::ledger,
    vault::{error, storage_error},
};

mod journal;
mod recovery;
#[cfg(test)]
mod tests;
mod worker;

pub const TRANSCRIPTION_OWNER: &str = "00000000-0000-0000-0000-000000000000";
pub const SUMMARY_OWNER: &str = "desktop";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct LocalTranscriptionOptions {
    pub model: String,
    pub languages: Vec<String>,
    pub keywords: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct LocalCaptureDeclaration {
    pub request_id: String,
    pub session_id: String,
    pub append: bool,
    pub batch_requested: bool,
    pub transcription: Option<LocalTranscriptionOptions>,
    pub summary: Option<CaptureProcessingSummary>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct LocalCapturePersistence {
    pub live_transcription_active: bool,
    pub needs_batch_repair: bool,
    pub new_transcript_id: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LocalCaptureState {
    Capturing,
    WaitingForPersistence,
    Transcribing,
    Summarizing,
    Succeeded,
    Skipped,
    NotStarted,
    Review,
    Conflict,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct LocalCaptureStatus {
    pub declaration: LocalCaptureDeclaration,
    pub state: LocalCaptureState,
    pub transcription_owner: String,
    pub summary_owner: String,
    pub transcript_id: Option<String>,
    pub summary_job_id: Option<String>,
    pub message: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    vault_id: String,
    status: LocalCaptureStatus,
    had_transcript_before: bool,
    summary: Option<SummaryJobRequest>,
    physical: Option<Result<(), String>>,
    persistence: Option<Result<LocalCapturePersistence, String>>,
    source: Option<ProcessingSourceBinding>,
    inference_dispatched: bool,
    result_sha256: Option<String>,
    published_revision: Option<String>,
}
struct Entry {
    record: Record,
    admission: Option<Arc<OwnedRwLockReadGuard<()>>>,
    running: bool,
}
pub struct LocalCaptureIntents {
    context: Arc<VaultContext>,
    root: PathBuf,
    state: Mutex<BTreeMap<String, Entry>>,
    stop: CancellationToken,
    changed: Notify,
}
#[derive(Clone)]
pub struct LocalCaptureIntent {
    service: Arc<LocalCaptureIntents>,
    request: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalCaptureChildOrigin {
    request_id: String,
    session_id: String,
    source: ProcessingSourceBinding,
}
impl LocalCaptureChildOrigin {
    pub(crate) fn validate_request(
        &self,
        request: &SummaryJobRequest,
    ) -> Result<(), ApplicationError> {
        uuid(&self.request_id)?;
        if self.session_id != request.session_id
            || !hypr_loofah_protocol::is_transcription_sha256(&self.source.recording_sha256)
            || !hypr_loofah_protocol::is_transcription_sha256(&self.source.transcript_revision)
        {
            return Err(review("Local summary origin changed"));
        }
        Ok(())
    }
}

impl LocalCaptureIntents {
    pub async fn open(context: Arc<VaultContext>) -> Result<Arc<Self>, ApplicationError> {
        if context.lease.is_none() {
            return Err(review("Local capture requires the original vault owner"));
        }
        let root = context
            .operations
            .as_ref()
            .ok_or_else(|| review("Local operation storage unavailable"))?
            .root
            .join("local-capture-intents");
        let directory = root.clone();
        let vault = context.events.vault_id();
        let pin = context.clone();
        let records = tokio::task::spawn_blocking(move || {
            let _pin = pin;
            journal::load(&directory, &vault)
        })
        .await
        .map_err(review)??;
        let service = Arc::new(Self {
            context: context.clone(),
            root,
            state: Mutex::new(
                records
                    .into_iter()
                    .map(|record| {
                        (
                            record.status.declaration.request_id.clone(),
                            Entry {
                                record,
                                admission: None,
                                running: false,
                            },
                        )
                    })
                    .collect(),
            ),
            stop: CancellationToken::new(),
            changed: Notify::new(),
        });
        context
            .local_capture_intents
            .set(Arc::downgrade(&service))
            .map_err(|_| review("Local capture owner already configured"))?;
        Ok(service)
    }
    pub fn request_shutdown(&self) {
        self.stop.cancel();
        self.changed.notify_waiters();
    }
    pub async fn shutdown(&self) {
        self.request_shutdown();
        loop {
            let changed = self.changed.notified();
            let state = self.state.lock().await;
            if !state
                .values()
                .any(|entry| entry.running || entry.admission.is_some())
            {
                return;
            }
            drop(state);
            changed.await;
        }
    }
    pub async fn statuses(&self) -> Vec<LocalCaptureStatus> {
        self.state
            .lock()
            .await
            .values()
            .map(|entry| entry.record.status.clone())
            .collect()
    }
    pub async fn status(&self, request: &str) -> Result<LocalCaptureStatus, ApplicationError> {
        self.state
            .lock()
            .await
            .get(request)
            .map(|entry| entry.record.status.clone())
            .ok_or_else(|| error(ErrorCode::NotFound, "Local capture intent not found"))
    }
    pub async fn declare(
        self: &Arc<Self>,
        input: LocalCaptureDeclaration,
    ) -> Result<LocalCaptureIntent, ApplicationError> {
        validate(&input)?;
        let this = self.clone();
        tokio::spawn(async move { this.declare_owned(input).await })
            .await
            .map_err(review)?
    }
    async fn declare_owned(
        self: &Arc<Self>,
        input: LocalCaptureDeclaration,
    ) -> Result<LocalCaptureIntent, ApplicationError> {
        if self.stop.is_cancelled() {
            return Err(review("Local capture owner is stopping"));
        }
        let admission = self.context.admit_session_work(&input.session_id).await?;
        self.context
            .ensure_recording_publication_ready(&input.session_id)?;
        self.context.read_note(&input.session_id).await?;
        let recording = self
            .context
            .store
            .open_recording(&input.session_id)
            .await
            .map_err(|e| storage_error(e, false))?;
        if matches!(
            recording,
            Some(hypr_vault_write::media::RecordingSnapshot::Busy)
        ) {
            return Err(review("Recording already has an active producer"));
        }
        if matches!(
            recording,
            Some(hypr_vault_write::media::RecordingSnapshot::File(_))
        ) != input.append
        {
            return Err(error(
                ErrorCode::Conflict,
                "Recording changed before Local capture declaration",
            ));
        }
        drop(recording);
        let had_transcript_before = self
            .context
            .session_transcripts(&input.session_id)
            .await?
            .iter()
            .any(|t| !t.words.is_empty());
        let summary = match &input.summary {
            Some(summary) => Some(SummaryJobRequest {
                session_id: input.session_id.clone(),
                target: summary.target.clone(),
                expected_revision: self
                    .context
                    .read_document(&input.session_id, &summary.target)
                    .await?
                    .revision,
                title_policy: summary.title_policy,
                only_if_empty: !input.append,
            }),
            None => None,
        };
        let mut state = self.state.lock().await;
        if self.stop.is_cancelled()
            || state.contains_key(&input.request_id)
            || state.len() >= 4096
            || state.values().any(|entry| {
                entry.record.status.declaration.session_id == input.session_id
                    && (entry.admission.is_some() || entry.running || unresolved(&entry.record))
            })
        {
            return Err(review(
                "An original Local capture request requires completion or review",
            ));
        }
        let request = input.request_id.clone();
        let record = Record {
            version: 1,
            vault_id: self.context.events.vault_id(),
            status: LocalCaptureStatus {
                declaration: input,
                state: LocalCaptureState::Capturing,
                transcription_owner: TRANSCRIPTION_OWNER.into(),
                summary_owner: SUMMARY_OWNER.into(),
                transcript_id: None,
                summary_job_id: None,
                message: None,
            },
            had_transcript_before,
            summary,
            physical: None,
            persistence: None,
            source: None,
            inference_dispatched: false,
            result_sha256: None,
            published_revision: None,
        };
        self.persist(&record, true).await?;
        state.insert(
            request.clone(),
            Entry {
                record,
                admission: Some(Arc::new(admission)),
                running: false,
            },
        );
        Ok(LocalCaptureIntent {
            service: self.clone(),
            request,
        })
    }
    async fn persist(&self, record: &Record, create: bool) -> Result<(), ApplicationError> {
        let root = self.root.clone();
        let record = record.clone();
        let pin = self.context.clone();
        tokio::task::spawn_blocking(move || {
            let _pin = pin;
            journal::persist(&root, &record, create)
        })
        .await
        .map_err(review)?
    }
    async fn update(
        &self,
        request: &str,
        change: impl FnOnce(&mut Record),
    ) -> Result<Record, ApplicationError> {
        let mut state = self.state.lock().await;
        let entry = state
            .get_mut(request)
            .ok_or_else(|| review("Local intent unavailable"))?;
        let mut next = entry.record.clone();
        change(&mut next);
        self.persist(&next, false).await?;
        entry.record = next.clone();
        Ok(next)
    }
    pub(crate) async fn ensure_idle(&self, session: &str) -> Result<(), ApplicationError> {
        if self.state.lock().await.values().any(|entry| {
            entry.record.status.declaration.session_id == session
                && (entry.admission.is_some() || entry.running || unresolved(&entry.record))
        }) {
            return Err(review("Local capture intent requires completion or review"));
        }
        Ok(())
    }
}
impl LocalCaptureIntent {
    pub async fn physical_finished(
        &self,
        outcome: Result<(), String>,
    ) -> Result<(), ApplicationError> {
        self.complete(Some(outcome), None, false).await
    }
    pub async fn persistence_finished(
        &self,
        outcome: Result<LocalCapturePersistence, String>,
    ) -> Result<(), ApplicationError> {
        self.complete(None, Some(outcome), false).await
    }
    pub async fn not_started(&self) -> Result<(), ApplicationError> {
        self.complete(None, None, true).await
    }
    async fn complete(
        &self,
        physical: Option<Result<(), String>>,
        persistence: Option<Result<LocalCapturePersistence, String>>,
        not_started: bool,
    ) -> Result<(), ApplicationError> {
        let this = self.clone();
        tokio::spawn(async move {
            let mut state = this.service.state.lock().await;
            let entry = state
                .get_mut(&this.request)
                .ok_or_else(|| review("Original Local intent unavailable"))?;
            let mut record = entry.record.clone();
            if let Some(value) = &physical {
                if let Some(old) = &record.physical {
                    return if old == value {
                        Ok(())
                    } else {
                        Err(review("Local physical completion changed"))
                    };
                }
            }
            if let Some(value) = &persistence {
                if let Some(old) = &record.persistence {
                    return if old == value {
                        Ok(())
                    } else {
                        Err(review("Local persistence completion changed"))
                    };
                }
                if let Ok(value) = value {
                    if let Some(id) = &value.new_transcript_id {
                        uuid(id)?;
                    }
                }
            }
            if not_started && record.status.state == LocalCaptureState::NotStarted {
                return Ok(());
            }
            if physical
                .as_ref()
                .and_then(|v| v.as_ref().err())
                .is_some_and(|e| e.len() > 4096)
                || persistence
                    .as_ref()
                    .and_then(|v| v.as_ref().err())
                    .is_some_and(|e| e.len() > 4096)
            {
                return Err(review("Native completion detail exceeds its budget"));
            }
            if entry.admission.is_none() {
                return Err(review("Original Local producer is no longer admitted"));
            }
            if not_started {
                if record.physical.is_some() || record.persistence.is_some() {
                    return Err(review("Cannot undo a started Local capture"));
                }
                record.status.state = LocalCaptureState::NotStarted;
            } else {
                if let Some(value) = physical {
                    if record.physical.as_ref().is_some_and(|old| old != &value) {
                        return Err(review("Local physical completion changed"));
                    }
                    record.physical = Some(value);
                }
                if let Some(value) = persistence {
                    if record.persistence.as_ref().is_some_and(|old| old != &value) {
                        return Err(review("Local persistence completion changed"));
                    }
                    record.persistence = Some(value);
                }
                record.status.state = LocalCaptureState::WaitingForPersistence;
            }
            let ready = record.physical.is_some() && record.persistence.is_some();
            if ready
                && (!matches!(record.physical, Some(Ok(())))
                    || !matches!(record.persistence, Some(Ok(_))))
            {
                record.status.state = LocalCaptureState::Review;
                record.status.message = Some(
                    "Native completion or persistence failed; no inference was dispatched".into(),
                );
            }
            this.service.persist(&record, false).await?;
            entry.record = record;
            let start = ready
                && !not_started
                && entry.record.status.state != LocalCaptureState::Review
                && !entry.running;
            if not_started || ready && !start {
                entry.admission.take();
            }
            if start {
                entry.running = true;
            }
            drop(state);
            this.service.changed.notify_waiters();
            if start {
                this.service.clone().start(this.request);
            }
            Ok(())
        })
        .await
        .map_err(review)?
    }
}
fn unresolved(record: &Record) -> bool {
    matches!(
        record.status.state,
        LocalCaptureState::Capturing
            | LocalCaptureState::WaitingForPersistence
            | LocalCaptureState::Transcribing
            | LocalCaptureState::Summarizing
            | LocalCaptureState::Review
    )
}
fn uuid(value: &str) -> Result<uuid::Uuid, ApplicationError> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil() && id.to_string() == value)
        .ok_or_else(|| {
            error(
                ErrorCode::Validation,
                "Local capture requires a canonical request UUID",
            )
        })
}
fn validate(input: &LocalCaptureDeclaration) -> Result<(), ApplicationError> {
    uuid(&input.request_id)?;
    hypr_loofah_protocol::validate_session_id(&input.session_id)
        .map_err(|_| error(ErrorCode::Validation, "Invalid Local capture session"))?;
    if input.batch_requested && input.transcription.is_none() {
        return Err(error(
            ErrorCode::Validation,
            "Batch capture requires frozen transcription options",
        ));
    }
    if let Some(options) = &input.transcription {
        hypr_loofah_protocol::validate_capture_processing_request(
            &hypr_loofah_protocol::CaptureProcessingRequest {
                capture_fingerprint: "0".repeat(64),
                model: Some(options.model.clone()),
                languages: options.languages.clone(),
                keywords: options.keywords.clone(),
                summary: input.summary.clone(),
            },
        )
        .map_err(|e| error(ErrorCode::Validation, e))?;
    }
    Ok(())
}
fn review(cause: impl std::fmt::Display) -> ApplicationError {
    let mut e = error(
        ErrorCode::Conflict,
        &format!("Local capture requires review: {cause}"),
    );
    e.mutation_outcome = Some(hypr_loofah_protocol::MutationOutcome::Unknown);
    e
}

impl VaultContext {
    pub(crate) async fn guard_local_capture_summary(
        &self,
        owner: &str,
        request: &str,
        origin: Option<&LocalCaptureChildOrigin>,
    ) -> Result<
        Option<hypr_vault_write::transcript_publication::ProcessingSourceGuard>,
        ApplicationError,
    > {
        let Some(origin) = origin else {
            return Ok(None);
        };
        if owner != SUMMARY_OWNER || request != origin.request_id {
            return Err(review("Local summary child identity changed"));
        }
        let service = self
            .local_capture_intents
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| review("Local summary parent unavailable"))?;
        let state = service.state.lock().await;
        let entry = state
            .get(request)
            .ok_or_else(|| review("Local summary parent missing"))?;
        if entry.record.status.state != LocalCaptureState::Summarizing
            || entry.record.status.declaration.session_id != origin.session_id
            || entry.record.source.as_ref() != Some(&origin.source)
            || service.stop.is_cancelled()
        {
            return Err(review("Local summary parent is not ready"));
        }
        drop(state);
        self.store
            .guard_processing_source(&origin.session_id, Some(origin.source.clone()))
            .await
            .map_err(|cause| storage_error(cause, false))?
            .map(Some)
            .ok_or_else(|| review("Local summary source disappeared"))
    }
}

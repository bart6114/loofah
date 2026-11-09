use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use hypr_loofah_protocol::{
    ErrorCode, TranscriptionJobRequest, TranscriptionJobSnapshot, TranscriptionJobState,
};
use serde::{Deserialize, Serialize};

use crate::submissions::{check_file, lock, prepare_directory, scope_key};
use crate::{Result, failure};

const MAX_RECEIPTS: usize = 128;
const MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionSubmissionReceipt {
    pub scope: String,
    pub request_id: String,
    pub input: TranscriptionJobRequest,
    pub created_at_ms: u64,
    pub job: Option<TranscriptionJobSnapshot>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    schema_version: u32,
    receipts: Vec<TranscriptionSubmissionReceipt>,
}

pub struct TranscriptionSubmissions {
    root: PathBuf,
    io: Arc<tokio::sync::Semaphore>,
    #[cfg(test)]
    fail_after_write: std::sync::atomic::AtomicBool,
}

impl TranscriptionSubmissions {
    pub fn new(root: PathBuf) -> Result<Arc<Self>> {
        if !root.is_absolute()
            || root.file_name().is_none()
            || root
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(failure(
                ErrorCode::Validation,
                "Job recovery storage must be absolute",
            ));
        }
        Ok(Arc::new(Self {
            root,
            io: Arc::new(tokio::sync::Semaphore::new(2)),
            #[cfg(test)]
            fail_after_write: std::sync::atomic::AtomicBool::new(false),
        }))
    }

    pub async fn prepare(
        self: &Arc<Self>,
        scope: String,
        input: TranscriptionJobRequest,
    ) -> Result<TranscriptionSubmissionReceipt> {
        validate_input(&input)?;
        self.run(scope.clone(), move |stored| {
            if let Some(existing) = stored.receipts.iter().find(|receipt| {
                receipt.input.session_id == input.session_id && blocks_new_request(receipt)
            }) {
                return if existing.input == input { Ok((existing.clone(), existing.job.is_none())) }
                else { Err(failure(ErrorCode::Conflict, "Recover the previous transcription request before starting another")) };
            }
            if stored.receipts.len() >= MAX_RECEIPTS {
                return Err(failure(ErrorCode::ResourceLimit, "Device transcription request storage is full. Forget finished requests before continuing."));
            }
            let receipt = TranscriptionSubmissionReceipt {
                scope,
                request_id: uuid::Uuid::new_v4().to_string(),
                input,
                created_at_ms: SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| damaged())?.as_millis().try_into().map_err(|_| damaged())?,
                job: None,
            };
            stored.receipts.push(receipt.clone());
            Ok((receipt, true))
        }).await
    }

    pub async fn list(
        self: &Arc<Self>,
        scope: String,
        session_id: Option<String>,
    ) -> Result<Vec<TranscriptionSubmissionReceipt>> {
        if session_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 512)
        {
            return Err(failure(
                ErrorCode::Validation,
                "Invalid job recovery session",
            ));
        }
        self.run(scope, move |stored| {
            let mut receipts: Vec<_> = stored
                .receipts
                .iter()
                .filter(|receipt| {
                    session_id
                        .as_ref()
                        .is_none_or(|id| id == &receipt.input.session_id)
                })
                .cloned()
                .collect();
            receipts.sort_by(|a, b| {
                (b.created_at_ms, &b.request_id).cmp(&(a.created_at_ms, &a.request_id))
            });
            Ok((receipts, false))
        })
        .await
    }

    pub async fn load(
        self: &Arc<Self>,
        scope: String,
        request_id: String,
    ) -> Result<Option<TranscriptionSubmissionReceipt>> {
        validate_uuid(&request_id)?;
        self.run(scope, move |stored| {
            let receipt = stored
                .receipts
                .iter()
                .find(|receipt| receipt.request_id == request_id)
                .cloned();
            // A previous attempt may have failed after rename but before directory sync.
            // Reconfirm durability before an unresolved receipt can cause a paid request.
            let confirm = receipt
                .as_ref()
                .is_some_and(|receipt| receipt.job.is_none());
            Ok((receipt, confirm))
        })
        .await
    }

    pub async fn observe(
        self: &Arc<Self>,
        scope: String,
        request_id: String,
        snapshot: TranscriptionJobSnapshot,
    ) -> Result<TranscriptionSubmissionReceipt> {
        validate_uuid(&request_id)?;
        self.run(scope, move |stored| {
            if stored.receipts.iter().any(|receipt| {
                receipt.request_id != request_id
                    && receipt
                        .job
                        .as_ref()
                        .is_some_and(|job| job.id == snapshot.id)
            }) {
                return Err(failure(
                    ErrorCode::Conflict,
                    "This transcription job already belongs to another saved request",
                ));
            }
            let receipt = stored
                .receipts
                .iter_mut()
                .find(|receipt| receipt.request_id == request_id)
                .ok_or_else(not_found)?;
            validate_snapshot(&receipt.input, &snapshot)?;
            if let Some(previous) = &receipt.job {
                if previous.id != snapshot.id {
                    return Err(failure(
                        ErrorCode::Conflict,
                        "This submission already belongs to another job",
                    ));
                }
                // Wall clocks are not causal. A late status read cannot revive a completed
                // job or replace a cancellation with an older running snapshot.
                if previous.state.is_terminal() || rank(&snapshot) < rank(previous) {
                    return Ok((receipt.clone(), false));
                }
            }
            let changed = receipt.job.as_ref() != Some(&snapshot);
            receipt.job = Some(snapshot);
            Ok((receipt.clone(), changed))
        })
        .await
    }

    pub async fn forget(self: &Arc<Self>, scope: String, request_id: String) -> Result<bool> {
        validate_uuid(&request_id)?;
        self.run(scope, move |stored| {
            let before = stored.receipts.len();
            stored
                .receipts
                .retain(|receipt| receipt.request_id != request_id);
            let changed = before != stored.receipts.len();
            Ok((changed, changed))
        })
        .await
    }

    async fn run<T: Send + 'static>(
        self: &Arc<Self>,
        scope: String,
        action: impl FnOnce(&mut Stored) -> Result<(T, bool)> + Send + 'static,
    ) -> Result<T> {
        if scope.is_empty() || scope.len() > 4096 || scope.chars().any(char::is_control) {
            return Err(failure(
                ErrorCode::Validation,
                "Invalid job account and vault scope",
            ));
        }
        let permit = self.io.clone().try_acquire_owned().map_err(|_| busy())?;
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut action = Some(action);
            let result = (|| {
                let directory = store.root.join(scope_key(&scope));
                prepare_directory(&store.root)?;
                prepare_directory(&directory)?;
                let _lock = lock(&directory.join(".lock"))?;
                let path = directory.join("receipts.json");
                let mut stored = read(&path, &scope)?;
                let (result, changed) = action.take().unwrap()(&mut stored)?;
                if changed {
                    let bytes = serde_json::to_string(&stored).map_err(|_| damaged())?;
                    if bytes.len() as u64 > MAX_BYTES {
                        return Err(failure(ErrorCode::ResourceLimit, "Device transcription request storage exceeds its byte limit. Saved requests were retained."));
                    }
                    check_file(&path)?;
                    hypr_storage::fs::atomic_write(&path, &bytes).map_err(io_error)?;
                    #[cfg(test)]
                    if store
                        .fail_after_write
                        .swap(false, std::sync::atomic::Ordering::SeqCst)
                    {
                        return Err(io_error(std::io::Error::other(
                            "submission directory sync fixture",
                        )));
                    }
                    #[cfg(unix)]
                    File::open(&directory)
                        .and_then(|file| file.sync_all())
                        .map_err(io_error)?;
                }
                Ok(result)
            })();
            drop(action);
            drop(store);
            drop(permit);
            result
        })
        .await
        .map_err(|_| damaged())?
    }
}

fn validate_uuid(id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id)
        .ok()
        .filter(|uuid| !uuid.is_nil() && uuid.to_string() == id)
        .map(|_| ())
        .ok_or_else(|| failure(ErrorCode::Validation, "Invalid job recovery identifier"))
}

fn blocks_new_request(receipt: &TranscriptionSubmissionReceipt) -> bool {
    receipt
        .job
        .as_ref()
        .is_none_or(|job| !job.state.is_terminal() || job.state == TranscriptionJobState::Uncertain)
}

fn validate_input(input: &TranscriptionJobRequest) -> Result<()> {
    hypr_loofah_protocol::validate_transcription_job_request(input)
        .map_err(|message| failure(ErrorCode::Validation, message))
}

fn validate_snapshot(
    input: &TranscriptionJobRequest,
    snapshot: &TranscriptionJobSnapshot,
) -> Result<()> {
    validate_uuid(&snapshot.id)?;
    if snapshot.session_id != input.session_id
        || snapshot.created_at.len() > 128
        || snapshot.updated_at.len() > 128
        || snapshot.publication.as_ref().is_some_and(|publication| {
            !hypr_loofah_protocol::is_transcription_sha256(&publication.revision)
                || hypr_loofah_protocol::validate_session_id(&publication.transcript_id).is_err()
        })
    {
        return Err(failure(
            ErrorCode::Conflict,
            "Job response does not match this submission",
        ));
    }
    Ok(())
}

fn rank(job: &TranscriptionJobSnapshot) -> (u8, u8) {
    use hypr_loofah_protocol::TranscriptionPhase;
    (
        match job.state {
            TranscriptionJobState::Queued => 0,
            TranscriptionJobState::Running => 1,
            TranscriptionJobState::Cancelling => 2,
            _ => 3,
        },
        match job.phase {
            None | Some(TranscriptionPhase::Preparing) => 0,
            Some(TranscriptionPhase::Transcribing) => 1,
            Some(TranscriptionPhase::Publishing) => 2,
        },
    )
}

fn read(path: &Path, scope: &str) -> Result<Stored> {
    check_file(path)?;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Stored {
                schema_version: 1,
                receipts: Vec::new(),
            });
        }
        Err(error) => return Err(io_error(error)),
    };
    if file.metadata().map_err(io_error)?.len() > MAX_BYTES {
        return Err(damaged());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(damaged());
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| damaged())?;
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err(failure(
            ErrorCode::IncompatibleVersion,
            "These saved job requests require another Loofah version. Their file was kept.",
        ));
    }
    let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| damaged())?;
    if serde_json::to_value(&stored).map_err(|_| damaged())? != value
        || stored.receipts.len() > MAX_RECEIPTS
    {
        return Err(damaged());
    }
    let mut ids = std::collections::HashSet::new();
    let mut active = std::collections::HashSet::new();
    let mut jobs = std::collections::HashSet::new();
    for receipt in &stored.receipts {
        validate_uuid(&receipt.request_id).map_err(|_| damaged())?;
        validate_input(&receipt.input).map_err(|_| damaged())?;
        if receipt.scope != scope
            || !ids.insert(&receipt.request_id)
            || receipt.created_at_ms > 9_007_199_254_740_991
        {
            return Err(damaged());
        }
        if let Some(snapshot) = &receipt.job {
            if !jobs.insert(&snapshot.id) {
                return Err(damaged());
            }
            validate_snapshot(&receipt.input, snapshot).map_err(|_| damaged())?;
        }
        if blocks_new_request(receipt) && !active.insert(&receipt.input.session_id) {
            return Err(damaged());
        }
    }
    Ok(stored)
}

fn not_found() -> hypr_loofah_protocol::ApplicationError {
    failure(ErrorCode::NotFound, "Saved transcription request not found")
}
fn damaged() -> hypr_loofah_protocol::ApplicationError {
    failure(
        ErrorCode::Internal,
        "Saved transcription requests could not be read safely. Their files were kept.",
    )
}
fn busy() -> hypr_loofah_protocol::ApplicationError {
    hypr_loofah_protocol::ApplicationError {
        retryable: true,
        ..failure(
            ErrorCode::Unavailable,
            "Another device request is using job recovery storage. Try again.",
        )
    }
}
fn io_error(error: std::io::Error) -> hypr_loofah_protocol::ApplicationError {
    tracing::error!(%error, "device job recovery I/O failed");
    failure(
        ErrorCode::Internal,
        "Cannot read or save transcription requests on this device",
    )
}

#[cfg(test)]
mod tests;

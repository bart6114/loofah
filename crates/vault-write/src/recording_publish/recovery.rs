use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use super::{
    RecordingPublicationPlan, RecordingPublicationRequest,
    io::{self, Receipt, State},
};
use crate::{SessionStore, StoreError};

const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
const RECEIPT_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordingPublicationDiscoveryLimits {
    pub max_directory_entries: usize,
    pub max_total_receipt_bytes: u64,
    pub max_results: usize,
}

impl Default for RecordingPublicationDiscoveryLimits {
    fn default() -> Self {
        Self {
            max_directory_entries: 16_384,
            max_total_receipt_bytes: 64 * 1024 * 1024,
            max_results: 256,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingPublicationPhase {
    Pending,
    MetadataWritten,
}

#[derive(Clone, Debug)]
pub struct PendingRecordingPublication {
    receipt: Receipt,
    filename: String,
    operations: PathBuf,
    store: Arc<tokio::sync::Mutex<()>>,
}

impl PendingRecordingPublication {
    pub fn request(&self) -> &RecordingPublicationRequest {
        &self.receipt.request
    }
    pub fn phase(&self) -> RecordingPublicationPhase {
        match self.receipt.state {
            State::MetadataWritten => RecordingPublicationPhase::MetadataWritten,
            _ => RecordingPublicationPhase::Pending,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingPublicationReviewKind {
    InvalidFilename,
    UnsafeReceipt,
    OversizedReceipt,
    InvalidReceipt,
    WrongVault,
    FilenameBinding,
    Uncertain,
    ReceiptChanged,
    SourceOrMetadataChanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingPublicationDiscoveryIssue {
    pub filename: String,
    pub session_id: Option<String>,
    pub kind: RecordingPublicationReviewKind,
}

#[derive(Debug, Default)]
pub struct RecordingPublicationInventory {
    pub pending: Vec<PendingRecordingPublication>,
    pub issues: Vec<RecordingPublicationDiscoveryIssue>,
    pub scanned_entries: usize,
    pub scanned_receipts: usize,
    pub budgeted_receipt_bytes: u64,
}

pub enum RecordingPublicationRecoveryPreparation {
    Prepared(Box<RecordingPublicationPlan>),
    NoLongerPending,
    Busy,
    RequiresReview(RecordingPublicationReviewKind),
}

pub struct RecordingPublicationRecovery(SessionStore);

impl RecordingPublicationRecovery {
    pub async fn discover(
        &self,
        operations: &Path,
        vault: &str,
        limits: RecordingPublicationDiscoveryLimits,
    ) -> Result<RecordingPublicationInventory, StoreError> {
        self.0
            .discover_recording_publications(operations, vault, limits)
            .await
    }

    pub async fn prepare(
        &self,
        operations: &Path,
        vault: &str,
        pending: PendingRecordingPublication,
    ) -> Result<RecordingPublicationRecoveryPreparation, StoreError> {
        self.0
            .prepare_recording_publication_recovery(operations, vault, pending)
            .await
    }
}

impl SessionStore {
    pub fn recording_publication_recovery(
        &self,
    ) -> Result<RecordingPublicationRecovery, StoreError> {
        self.ensure_ready()?;
        Ok(RecordingPublicationRecovery(self.clone()))
    }

    /// Only recovery of existing publication intents may run before the managed store is ready.
    pub fn recording_publication_startup_recovery(
        &self,
        owner: &Arc<std::fs::File>,
    ) -> Result<RecordingPublicationRecovery, StoreError> {
        if !self
            .vault_lease
            .get()
            .is_some_and(|original| Arc::ptr_eq(original, owner))
        {
            return Err(StoreError::Conflict(
                "Recording recovery requires the original vault owner".into(),
            ));
        }
        let mut recovery = self.clone();
        recovery.startup_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
        Ok(RecordingPublicationRecovery(recovery))
    }

    /// The scan is complete or fails closed; limits never produce a partial-success inventory.
    /// Only publication receipts in this trusted, dedicated operations directory are inspected.
    /// Core must coordinate admission/read gates after this snapshot is released. Neither an
    /// empty inventory nor a recovered handle is a lease covering later media/STT admission.
    pub async fn discover_recording_publications(
        &self,
        operations: &Path,
        expected_vault: &str,
        limits: RecordingPublicationDiscoveryLimits,
    ) -> Result<RecordingPublicationInventory, StoreError> {
        validate_scope(operations, expected_vault)?;
        if !(1..=65_536).contains(&limits.max_directory_entries)
            || !(1..=256 * 1024 * 1024).contains(&limits.max_total_receipt_bytes)
            || !(1..=1024).contains(&limits.max_results)
        {
            return Err(StoreError::Serialize(
                "Invalid recording discovery limits".into(),
            ));
        }
        let store = self.clone();
        let operations = operations.to_path_buf();
        let vault = expected_vault.to_owned();
        tokio::spawn(async move {
            store.ensure_ready()?;
            // Publication receipt transitions hold this same guard. Do not acquire a
            // session audio lock while retaining this directory snapshot.
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            store
                .clone()
                .spawn_blocking(move || {
                    let _snapshot = snapshot;
                    scan(&operations, &vault, limits, store.write_lock.clone())
                })
                .await
                .map_err(io::worker_error)?
        })
        .await
        .map_err(io::worker_error)?
    }

    /// This prepares only a previously discovered immutable intent. The caller can recheck,
    /// acquire a fresh authorization grant, then retain that grant through owned commit.
    /// `NoLongerPending` is historical completion, not current publication/status proof.
    pub async fn prepare_recording_publication_recovery(
        &self,
        operations: &Path,
        expected_vault: &str,
        pending: PendingRecordingPublication,
    ) -> Result<RecordingPublicationRecoveryPreparation, StoreError> {
        validate_scope(operations, expected_vault)?;
        if expected_vault != pending.request().vault_id
            || !Arc::ptr_eq(&self.write_lock, &pending.store)
        {
            return Ok(RecordingPublicationRecoveryPreparation::RequiresReview(
                RecordingPublicationReviewKind::WrongVault,
            ));
        }
        let store = self.clone();
        let operations = operations.to_path_buf();
        tokio::spawn(async move {
            let root = operations.clone();
            let resolved = store
                .spawn_blocking(move || checked_root(&root))
                .await
                .map_err(io::worker_error)??;
            if resolved.as_ref() != Some(&pending.operations)
                || io::receipt_path(&operations, pending.request())
                    .file_name()
                    .and_then(|name| name.to_str())
                    != Some(&pending.filename)
            {
                return Ok(RecordingPublicationRecoveryPreparation::RequiresReview(
                    RecordingPublicationReviewKind::ReceiptChanged,
                ));
            }
            let audio = match store
                .lock_session_audio(&pending.request().session_id)
                .await
            {
                Ok(audio) => audio,
                Err(StoreError::Conflict(_)) => {
                    return Ok(RecordingPublicationRecoveryPreparation::Busy);
                }
                Err(cause) => return Err(cause),
            };
            let plan = match super::prepare(
                store,
                operations,
                pending.request().clone(),
                None,
                Some(audio),
                Some(pending.receipt),
            )
            .await
            {
                Ok(Some(plan)) => plan,
                Ok(None) => return Ok(RecordingPublicationRecoveryPreparation::NoLongerPending),
                Err(StoreError::Conflict(_)) => {
                    return Ok(RecordingPublicationRecoveryPreparation::RequiresReview(
                        RecordingPublicationReviewKind::ReceiptChanged,
                    ));
                }
                Err(StoreError::Serialize(_)) => {
                    return Ok(RecordingPublicationRecoveryPreparation::RequiresReview(
                        RecordingPublicationReviewKind::InvalidReceipt,
                    ));
                }
                Err(cause) => return Err(cause),
            };
            match plan.finished.as_ref() {
                Some(super::RecordingPublication::Busy) => {
                    Ok(RecordingPublicationRecoveryPreparation::Busy)
                }
                Some(_) => Ok(RecordingPublicationRecoveryPreparation::RequiresReview(
                    RecordingPublicationReviewKind::SourceOrMetadataChanged,
                )),
                None => Ok(RecordingPublicationRecoveryPreparation::Prepared(Box::new(
                    plan,
                ))),
            }
        })
        .await
        .map_err(io::worker_error)?
    }
}

fn validate_scope(operations: &Path, vault: &str) -> Result<(), StoreError> {
    if !operations.is_absolute()
        || vault.is_empty()
        || vault.len() > 512
        || vault.chars().any(char::is_control)
    {
        return Err(StoreError::Serialize(
            "Invalid recording discovery scope".into(),
        ));
    }
    Ok(())
}

fn checked_root(root: &Path) -> Result<Option<PathBuf>, StoreError> {
    io::check_parents(&root.join("scope"))?;
    match std::fs::symlink_metadata(root) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            root.canonicalize().map(Some).map_err(io::io_error)
        }
        Ok(_) => Err(StoreError::Io(
            "Recording operations directory is not regular".into(),
        )),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(cause) => Err(io::io_error(cause)),
    }
}

fn scan(
    root: &Path,
    vault: &str,
    limits: RecordingPublicationDiscoveryLimits,
    store: Arc<tokio::sync::Mutex<()>>,
) -> Result<RecordingPublicationInventory, StoreError> {
    let Some(resolved) = checked_root(root)? else {
        return Ok(RecordingPublicationInventory::default());
    };
    let mut result = RecordingPublicationInventory::default();
    let mut result_bytes = 0usize;
    for entry in std::fs::read_dir(root).map_err(io::io_error)? {
        let entry = entry.map_err(io::io_error)?;
        result.scanned_entries += 1;
        if result.scanned_entries > limits.max_directory_entries {
            return Err(resource_limit());
        }
        let filename = entry.file_name().to_string_lossy().into_owned();
        if !filename
            .to_ascii_lowercase()
            .starts_with("recording-publication-")
        {
            continue;
        }
        result.scanned_receipts += 1;
        if !valid_filename(&filename) {
            push_issue(
                &mut result,
                &mut result_bytes,
                &filename,
                RecordingPublicationReviewKind::InvalidFilename,
                None,
                limits,
            )?;
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(io::io_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            push_issue(
                &mut result,
                &mut result_bytes,
                &filename,
                RecordingPublicationReviewKind::UnsafeReceipt,
                None,
                limits,
            )?;
            continue;
        }
        if metadata.len() > RECEIPT_BYTES {
            result.budgeted_receipt_bytes = result
                .budgeted_receipt_bytes
                .checked_add(RECEIPT_BYTES + 1)
                .filter(|size| *size <= limits.max_total_receipt_bytes)
                .ok_or_else(resource_limit)?;
            push_issue(
                &mut result,
                &mut result_bytes,
                &filename,
                RecordingPublicationReviewKind::OversizedReceipt,
                None,
                limits,
            )?;
            continue;
        }
        let remaining = limits.max_total_receipt_bytes - result.budgeted_receipt_bytes;
        let raw = io::read_receipt_bytes(&path, remaining)?.ok_or_else(|| {
            StoreError::Io("Recording receipt disappeared during discovery".into())
        })?;
        result.budgeted_receipt_bytes = result
            .budgeted_receipt_bytes
            .checked_add(raw.len() as u64)
            .filter(|size| *size <= limits.max_total_receipt_bytes)
            .ok_or_else(resource_limit)?;
        let receipt = match io::parse_receipt(&raw) {
            Ok(receipt) => receipt,
            Err(StoreError::Serialize(_)) => {
                push_issue(
                    &mut result,
                    &mut result_bytes,
                    &filename,
                    RecordingPublicationReviewKind::InvalidReceipt,
                    None,
                    limits,
                )?;
                continue;
            }
            Err(cause) => return Err(cause),
        };
        if receipt.validate(&receipt.request).is_err() {
            push_issue(
                &mut result,
                &mut result_bytes,
                &filename,
                RecordingPublicationReviewKind::InvalidReceipt,
                None,
                limits,
            )?;
            continue;
        }
        if receipt.request.vault_id != vault {
            push_issue(
                &mut result,
                &mut result_bytes,
                &filename,
                RecordingPublicationReviewKind::WrongVault,
                None,
                limits,
            )?;
            continue;
        }
        if io::receipt_path(root, &receipt.request).file_name() != path.file_name() {
            push_issue(
                &mut result,
                &mut result_bytes,
                &filename,
                RecordingPublicationReviewKind::FilenameBinding,
                None,
                limits,
            )?;
            continue;
        }
        match receipt.state {
            State::Committed => {}
            State::Uncertain => push_issue(
                &mut result,
                &mut result_bytes,
                &filename,
                RecordingPublicationReviewKind::Uncertain,
                Some(receipt.request.session_id.clone()),
                limits,
            )?,
            State::Pending | State::MetadataWritten => {
                let bytes = serde_json::to_vec(&receipt)
                    .map_err(io::serialize_error)?
                    .len();
                check_result_budget(&result, &mut result_bytes, bytes, limits)?;
                result.pending.push(PendingRecordingPublication {
                    receipt,
                    filename,
                    operations: resolved.clone(),
                    store: store.clone(),
                });
            }
        }
    }
    result.pending.sort_by(|a, b| a.filename.cmp(&b.filename));
    result.issues.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(result)
}

fn valid_filename(name: &str) -> bool {
    name.strip_prefix("recording-publication-")
        .and_then(|name| name.strip_suffix(".json"))
        .is_some_and(io::valid_hash)
}
fn check_result_budget(
    result: &RecordingPublicationInventory,
    bytes: &mut usize,
    extra: usize,
    limits: RecordingPublicationDiscoveryLimits,
) -> Result<(), StoreError> {
    if result.pending.len() + result.issues.len() >= limits.max_results {
        return Err(resource_limit());
    }
    *bytes = bytes
        .checked_add(extra)
        .filter(|value| *value <= MAX_RESULT_BYTES)
        .ok_or_else(resource_limit)?;
    Ok(())
}
fn resource_limit() -> StoreError {
    StoreError::ResourceLimit(
        "Recording discovery exceeds its scan or result budget; review retained receipts".into(),
    )
}

fn push_issue(
    result: &mut RecordingPublicationInventory,
    bytes: &mut usize,
    filename: &str,
    kind: RecordingPublicationReviewKind,
    session_id: Option<String>,
    limits: RecordingPublicationDiscoveryLimits,
) -> Result<(), StoreError> {
    check_result_budget(
        result,
        bytes,
        filename.len() + session_id.as_ref().map_or(0, String::len),
        limits,
    )?;
    result.issues.push(RecordingPublicationDiscoveryIssue {
        filename: filename.to_owned(),
        session_id,
        kind,
    });
    Ok(())
}
pub(super) fn same_intent(current: &Receipt, expected: &Receipt) -> bool {
    current.schema_version == expected.schema_version
        && current.request == expected.request
        && current.stage_name == expected.stage_name
        && current.before_metadata_sha256 == expected.before_metadata_sha256
        && current.after_metadata_sha256 == expected.after_metadata_sha256
        && current.published_started_at == expected.published_started_at
        && current.published_ended_at == expected.published_ended_at
}

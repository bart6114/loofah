use super::*;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read, path::Path};

const MAX_TOMBSTONES: usize = 4096;
const MAX_TOMBSTONE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    request: MediaRetirementRequest,
    files: Vec<SourceFile>,
    source_directory_present: bool,
    completed: bool,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFile {
    name: String,
    bytes: u64,
    sha256: String,
    volume: u64,
    index: u64,
}
impl Intent {
    pub(super) fn validate(&self, record: &Admission) -> Result<(), ApplicationError> {
        validate_media_retirement_request(&self.request).map_err(invalid)?;
        if self.request.expected_revision != record.revision
            || self.request.expected_declaration_sha256 != declaration_hash(record)?
            || self.request.expected_source != source(record)
            || self.files.len() > 2
            || self.files.iter().any(|file| {
                !matches!(file.name.as_str(), "source" | "partial")
                    || !is_transcription_sha256(&file.sha256)
                    || file.bytes > source(record).map_or(0, |source| source.size_bytes)
            })
            || self
                .files
                .windows(2)
                .any(|pair| pair[0].name >= pair[1].name)
            || (!self.source_directory_present && !self.files.is_empty())
        {
            return Err(conflict("Media retirement identity requires review"));
        }
        Ok(())
    }
}
fn unknown(message: &str) -> ApplicationError {
    let mut cause = error(ErrorCode::Conflict, message);
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    cause
}
fn invalid(message: &str) -> ApplicationError {
    error(ErrorCode::Validation, message)
}
fn source(record: &Admission) -> Option<MediaUploadSource> {
    record
        .declaration
        .operation
        .upload()
        .map(|(_, source)| source.clone())
}
fn declaration_hash(record: &Admission) -> Result<String, ApplicationError> {
    media_declaration_sha256(&record.declaration).map_err(invalid)
}
fn retired_path(root: &Path, request: &str) -> PathBuf {
    root.join("retired")
        .join(format!("{request}.admission.json"))
}
fn read_record(path: &Path) -> Result<Option<Admission>, ApplicationError> {
    if !ledger::check_file(path)? {
        return Ok(None);
    }
    stamp(&File::open(path).map_err(|_| unavailable("Media receipt unavailable"))?)?;
    serde_json::from_slice(&ledger::read(path, 1024 * 1024)?)
        .map(Some)
        .map_err(|_| conflict("Media retirement receipt requires review"))
}
pub(super) fn load_record(
    root: &Path,
    active: &Path,
) -> Result<Option<Admission>, ApplicationError> {
    let original = read_record(active)?;
    let archive = root.join("retired");
    if !ledger::existing_directory(&archive)? {
        return Ok(original);
    }
    let retired = read_record(&archive.join(active.file_name().unwrap()))?;
    match retired {
        Some(retired) => {
            if !retired
                .retirement
                .as_ref()
                .is_some_and(|intent| intent.completed)
            {
                return Err(conflict("Media retirement receipt is incomplete"));
            }
            if let Some(mut original) = original {
                let Some(intent) = &mut original.retirement else {
                    return Err(conflict(
                        "Media retirement receipt conflicts with active admission",
                    ));
                };
                intent.completed = true;
                if ledger::encode_bounded_result(&original, 1024 * 1024)?
                    != ledger::encode_bounded_result(&retired, 1024 * 1024)?
                {
                    return Err(conflict("Media retirement receipt changed"));
                }
            }
            Ok(Some(retired))
        }
        None => Ok(original),
    }
}
impl MediaDispatcher {
    pub(super) async fn source_gate(&self, request: &str) -> Arc<tokio::sync::RwLock<()>> {
        let mut gates = self.source_gates.lock().await;
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
    async fn ensure_retirement_drained(&self, request: &str) -> Result<(), ApplicationError> {
        if self
            .tasks
            .lock()
            .await
            .get(request)
            .is_some_and(|task| !task.join.is_finished())
            || self
                .receivers
                .lock()
                .await
                .get(request)
                .is_some_and(|(_, done)| !*done.borrow())
        {
            return Err(conflict("Original media work is still draining"));
        }
        Ok(())
    }
    async fn retirement_eligibility(&self, record: &Admission) -> Result<(), ApplicationError> {
        let request = &record.declaration.request_id;
        self.ensure_retirement_drained(request).await?;
        let status = self.status_record(record.clone()).await?;
        let never = record.dispatch == Some(journal::DispatchEvidence::NeverDispatched)
            && matches!(
                record.phase,
                MediaPhase::Receiving | MediaPhase::Ready | MediaPhase::Cancelled
            );
        if !never && status.phase != MediaPhase::Succeeded {
            return Err(conflict(
                "Only confirmed successful or never-dispatched media may retire",
            ));
        }
        let summary = self.summary_status(request).await?;
        use crate::manual_media_intents::ManualMediaSummaryState;
        match summary.state {
            ManualMediaSummaryState::PendingMedia if never && summary.job.is_none() => {}
            ManualMediaSummaryState::Skipped if summary.job.is_none() => {}
            ManualMediaSummaryState::Child
                if summary
                    .job
                    .as_ref()
                    .is_some_and(|job| job.state == ProcessingJobState::Succeeded) => {}
            _ => {
                return Err(conflict(
                    "Original media summary requires completion or review",
                ));
            }
        }
        self.context
            .ensure_session_jobs_idle(&summary.session_id)
            .await?;
        Ok(())
    }
    pub async fn retirement_status(
        &self,
        request: &str,
    ) -> Result<MediaRetirementStatus, ApplicationError> {
        journal::validate_request(request)?;
        let gate = self.try_request_gate(request).await;
        let _grant = self.authorization.authorize().await?;
        let record = self
            .load(request)
            .await?
            .ok_or_else(|| error(ErrorCode::NotFound, "Media request not found"))?;
        let reason = if record.retirement.is_none() {
            match gate {
                Err(cause) => Some(cause.message),
                Ok(_gate) => match self.source_gate(request).await.try_write_owned() {
                    Err(_) => Some("Original media source is still in use".into()),
                    Ok(_source) => self
                        .retirement_eligibility(&record)
                        .await
                        .err()
                        .map(|cause| cause.message),
                },
            }
        } else {
            None
        };
        self.retirement_snapshot(&record, reason).await
    }
    async fn retirement_snapshot(
        &self,
        record: &Admission,
        reason: Option<String>,
    ) -> Result<MediaRetirementStatus, ApplicationError> {
        let request = record.declaration.request_id.clone();
        let path = self.root.join(format!("{request}.admission.json"));
        let owner = self.context.clone();
        let complete = self.root.join(format!("{request}.source")).join("source");
        let expected = source(record);
        let (active, exportable) = tokio::task::spawn_blocking(move || {
            let _owner = owner;
            let exportable = expected.is_some_and(|expected| {
                ledger::check_file(&complete).unwrap_or(false)
                    && std::fs::metadata(&complete)
                        .is_ok_and(|meta| meta.len() == expected.size_bytes)
            });
            ledger::check_file(&path).map(|active| (active, exportable))
        })
        .await
        .map_err(|_| unavailable("Media retirement reader stopped"))??;
        let completed = record
            .retirement
            .as_ref()
            .is_some_and(|intent| intent.completed)
            && !active;
        let allowed_dispositions = if record.retirement.is_some() || reason.is_some() {
            vec![]
        } else if source(record).is_none() {
            vec![MediaSourceDisposition::NoUpload]
        } else if exportable {
            vec![
                MediaSourceDisposition::Exported,
                MediaSourceDisposition::Discard,
            ]
        } else {
            vec![MediaSourceDisposition::Discard]
        };
        Ok(MediaRetirementStatus {
            request_id: request,
            revision: record.revision.clone(),
            declaration_sha256: declaration_hash(record)?,
            source: source(record),
            state: if completed {
                MediaRetirementState::Retired
            } else if record.retirement.is_some() {
                MediaRetirementState::Retiring
            } else if reason.is_some() {
                MediaRetirementState::Blocked
            } else {
                MediaRetirementState::Eligible
            },
            retirement: record
                .retirement
                .as_ref()
                .map(|intent| intent.request.clone()),
            reason,
            quota_released: completed,
            allowed_dispositions,
        })
    }
    pub async fn retire(
        self: &Arc<Self>,
        request: String,
        input: MediaRetirementRequest,
    ) -> Result<MediaRetirementStatus, ApplicationError> {
        journal::validate_request(&request)?;
        validate_media_retirement_request(&input).map_err(invalid)?;
        let owner = self.clone();
        tokio::spawn(async move { owner.retire_owned(request, input).await })
            .await
            .map_err(|_| unknown("Original media retirement requires explicit retry"))?
    }
    async fn retire_owned(
        self: &Arc<Self>,
        request: String,
        input: MediaRetirementRequest,
    ) -> Result<MediaRetirementStatus, ApplicationError> {
        let _request = self.try_request_gate(&request).await?;
        let _retirement_owner = self
            .source_reads
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable("Media retirement capacity unavailable"))?;
        drop(self.authorization.authorize().await?);
        if self.stop.is_cancelled() {
            return Err(unavailable("Media service is stopping"));
        }
        let mut record = self
            .load(&request)
            .await?
            .ok_or_else(|| error(ErrorCode::NotFound, "Media request not found"))?;
        if input.expected_revision != record.revision
            || input.expected_declaration_sha256 != declaration_hash(&record)?
            || input.expected_source != source(&record)
        {
            return Err(conflict("Original media retirement declaration changed"));
        }
        if let Some(intent) = &record.retirement {
            if intent.request != input {
                return Err(conflict("Original media retirement request changed"));
            }
            if intent.completed {
                let historical = self.retirement_snapshot(&record, None).await?;
                if historical.quota_released {
                    drop(self.authorization.authorize().await?);
                    return Ok(historical);
                }
            }
        }
        let source_gate = self
            .source_gate(&request)
            .await
            .try_write_owned()
            .map_err(|_| conflict("Original media source is still in use"))?;
        let _summary_request = self.context.admit_session_lifecycle_request(
            "manual-media-summary",
            &self.principal,
            uuid::Uuid::parse_str(&request).unwrap(),
        )?;
        let session = match record.declaration.operation.session_id() {
            Some(session) => session.to_owned(),
            None => {
                let cli = self.cli(&record.declaration, self.stop.child_token())?;
                self.context
                    .created_session_id(&cli.media_namespace("local-cli-import"), &request)?
            }
        };
        let session_guard = self.context.try_session_lifecycle_change(&session)?;
        self.ensure_retirement_drained(&request).await?;
        self.context.ensure_session_jobs_idle(&session).await?;
        if let Some(intent) = &record.retirement {
            if intent.request != input {
                return Err(conflict("Original media retirement request changed"));
            }
        } else {
            self.retirement_eligibility(&record).await?;
            let root = self.root.clone();
            let request = request.clone();
            let descriptor = source(&record);
            let disposition = input.disposition;
            let context = self.context.clone();
            let (present, files) = tokio::task::spawn_blocking(move || {
                let _context = context;
                source_inventory(
                    &root.join(format!("{request}.source")),
                    descriptor.as_ref(),
                    disposition,
                )
            })
            .await
            .map_err(|_| unavailable("Media source inspection stopped"))??;
            if record.dispatch == Some(journal::DispatchEvidence::NeverDispatched) {
                record.phase = MediaPhase::Cancelled;
            }
            record.version = 3;
            // Legacy success is a durable completion proof, never a never-dispatched assertion.
            if record.dispatch.is_none() {
                record.dispatch = Some(journal::DispatchEvidence::Dispatched);
            }
            record.retirement = Some(Intent {
                request: input,
                files,
                source_directory_present: present,
                completed: false,
            });
            record.validate(self, &request)?;
            let _admission = self.admission.lock().await;
            self.check_archive_capacity(&record).await?;
            let _grant = self.authorization.authorize().await?;
            self.save(record.clone(), false).await?;
        }
        let root = self.root.clone();
        let original = record.clone();
        let context = self.context.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            let _context = context;
            prepare_cleanup(&root, &original)
        })
        .await
        .map_err(|_| unknown("Media source verification requires explicit retry"))??;
        let grant = self.authorization.authorize().await?;
        let journal_guard = self.journal_gate(&request).await.write_owned().await;
        let root = self.root.clone();
        let context = self.context.clone();
        let original = record.clone();
        #[cfg(test)]
        let pause = self.retirement_pause.lock().unwrap().take();
        tokio::task::spawn_blocking(move || {
            let _pins = (context, grant, source_gate, session_guard, journal_guard);
            #[cfg(test)]
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _ = release.recv();
            }
            finish_prepared(&root, original, prepared, |_| Ok(()))
        })
        .await
        .map_err(|_| unknown("Media retirement filesystem work requires review"))??;
        record.retirement.as_mut().unwrap().completed = true;
        if let Some(task) = self.tasks.lock().await.remove(&request) {
            let _ = task.join.await;
        }
        self.receivers.lock().await.remove(&request);
        self.retirement_snapshot(&record, None).await
    }
    async fn check_archive_capacity(&self, record: &Admission) -> Result<(), ApplicationError> {
        let directory = self.root.join("retired");
        let pending = self.inventory().await?;
        let reserved: Vec<_> = pending
            .iter()
            .filter(|record| record.retirement.is_some())
            .collect();
        let reserved_count = reserved.len();
        let reserved_bytes = reserved.iter().try_fold(0_u64, |sum, record| {
            ledger::encode_bounded_result(record, 1024 * 1024)
                .map(|bytes| sum.saturating_add(bytes.len() as u64))
        })?;
        let needed = ledger::encode_bounded_result(record, 1024 * 1024)?.len() as u64;
        let context = self.context.clone();
        tokio::task::spawn_blocking(move || {
            let _context = context;
            let mut count = reserved_count;
            let mut bytes = needed.saturating_add(reserved_bytes);
            if ledger::existing_directory(&directory)? {
                for entry in std::fs::read_dir(&directory)
                    .map_err(|_| unavailable("Media receipt budget unavailable"))?
                {
                    let path = entry
                        .map_err(|_| unavailable("Media receipt budget unavailable"))?
                        .path();
                    let request = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .and_then(|name| name.strip_suffix(".admission.json"))
                        .ok_or_else(|| conflict("Unexpected media retirement archive entry"))?;
                    journal::validate_request(request)?;
                    if !ledger::check_file(&path)? {
                        return Err(conflict("Media retirement archive requires review"));
                    }
                    stamp(
                        &File::open(&path).map_err(|_| unavailable("Media receipt unavailable"))?,
                    )?;
                    count += 1;
                    bytes = bytes.saturating_add(
                        std::fs::metadata(path)
                            .map_err(|_| unavailable("Media receipt budget unavailable"))?
                            .len(),
                    );
                    if count >= MAX_TOMBSTONES || bytes > MAX_TOMBSTONE_BYTES {
                        return Err(unavailable("Media retirement receipt budget exhausted"));
                    }
                }
            }
            if count >= MAX_TOMBSTONES || bytes > MAX_TOMBSTONE_BYTES {
                return Err(unavailable("Media retirement receipt budget exhausted"));
            }
            Ok(())
        })
        .await
        .map_err(|_| unavailable("Media receipt budget reader stopped"))?
    }
}
fn stamp(file: &File) -> Result<(u64, u64), ApplicationError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file
            .metadata()
            .map_err(|_| unavailable("Media source unavailable"))?;
        if m.nlink() != 1 {
            return Err(conflict("Media source is shared"));
        }
        Ok((m.dev(), m.ino()))
    }
    #[cfg(windows)]
    {
        let m = winapi_util::file::information(file)
            .map_err(|_| unavailable("Media source unavailable"))?;
        if m.number_of_links() != 1 {
            return Err(conflict("Media source is shared"));
        }
        Ok((m.volume_serial_number(), m.file_index()))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(unavailable("Media retirement unsupported on this platform"))
    }
}
struct CheckedSource {
    descriptor: SourceFile,
    file: File,
    modified: std::time::SystemTime,
}
fn inspect(path: &Path, limit: u64) -> Result<SourceFile, ApplicationError> {
    Ok(inspect_owned(path, limit)?.descriptor)
}
fn inspect_owned(path: &Path, limit: u64) -> Result<CheckedSource, ApplicationError> {
    use sha2::{Digest, Sha256};
    if !ledger::check_file(path)? {
        return Err(conflict("Original media source disappeared"));
    }
    let mut file = File::open(path).map_err(|_| unavailable("Media source unavailable"))?;
    let before = file
        .metadata()
        .map_err(|_| unavailable("Media source unavailable"))?;
    if before.len() > limit {
        return Err(conflict("Media source exceeds original declaration"));
    }
    let modified = before
        .modified()
        .map_err(|_| unavailable("Media source modification evidence unavailable"))?;
    let (volume, index) = stamp(&file)?;
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| unavailable("Media source unavailable"))?;
        if count == 0 {
            break;
        }
        bytes = bytes.saturating_add(count as u64);
        if bytes > limit {
            return Err(conflict("Media source exceeds original declaration"));
        }
        hash.update(&buffer[..count]);
    }
    let after = file
        .metadata()
        .map_err(|_| unavailable("Media source unavailable"))?;
    if before.len() != bytes
        || after.len() != bytes
        || after
            .modified()
            .map_err(|_| unavailable("Media source modification evidence unavailable"))?
            != modified
        || stamp(&File::open(path).map_err(|_| unavailable("Media source unavailable"))?)?
            != (volume, index)
    {
        return Err(conflict("Original media source changed"));
    }
    let descriptor = SourceFile {
        name: path
            .file_name()
            .unwrap()
            .to_str()
            .ok_or_else(|| conflict("Media source name invalid"))?
            .into(),
        bytes,
        sha256: ledger::hex(&hash.finalize()),
        volume,
        index,
    };
    Ok(CheckedSource {
        descriptor,
        file,
        modified,
    })
}
fn source_inventory(
    directory: &Path,
    expected: Option<&MediaUploadSource>,
    disposition: MediaSourceDisposition,
) -> Result<(bool, Vec<SourceFile>), ApplicationError> {
    if !ledger::existing_directory(directory)? {
        if disposition == MediaSourceDisposition::Exported {
            return Err(conflict(
                "Verified original source must be exported before retirement",
            ));
        }
        return Ok((false, vec![]));
    }
    let mut files = Vec::new();
    for entry in
        std::fs::read_dir(directory).map_err(|_| unavailable("Media source unavailable"))?
    {
        let path = entry
            .map_err(|_| unavailable("Media source unavailable"))?
            .path();
        if !matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("source" | "partial")
        ) || files.len() >= 2
        {
            return Err(conflict("Unexpected media source file requires review"));
        }
        let expected = expected.ok_or_else(|| conflict("Unexpected uploaded source"))?;
        let file = inspect(&path, expected.size_bytes)?;
        if file.bytes > expected.size_bytes
            || (file.name == "source"
                && (file.bytes != expected.size_bytes || file.sha256 != expected.sha256))
        {
            return Err(conflict("Original media source changed"));
        }
        files.push(file);
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    if disposition == MediaSourceDisposition::Exported
        && !files.iter().any(|file| file.name == "source")
    {
        return Err(conflict("Verified original source is unavailable"));
    }
    Ok((true, files))
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum CleanupCheckpoint {
    SourceRemoved,
    ReceiptSaved,
}
fn prepare_cleanup(
    root: &Path,
    record: &Admission,
) -> Result<Vec<CheckedSource>, ApplicationError> {
    let intent = record.retirement.as_ref().unwrap();
    let directory = root.join(format!("{}.source", record.declaration.request_id));
    let mut checked = Vec::new();
    if ledger::existing_directory(&directory)? {
        for entry in std::fs::read_dir(&directory)
            .map_err(|_| unavailable("Media source cleanup unavailable"))?
        {
            let path = entry
                .map_err(|_| unavailable("Media source cleanup unavailable"))?
                .path();
            if intent.completed || !intent.source_directory_present || checked.len() >= 2 {
                return Err(conflict("Retired media source reappeared"));
            }
            let actual =
                inspect_owned(&path, source(record).map_or(0, |source| source.size_bytes))?;
            if !intent.files.contains(&actual.descriptor) {
                return Err(conflict("Media retirement source changed"));
            }
            checked.push(actual);
        }
    }
    Ok(checked)
}
#[cfg(test)]
fn finish_with(
    root: &Path,
    record: Admission,
    checkpoint: impl Fn(CleanupCheckpoint) -> Result<(), ApplicationError>,
) -> Result<(), ApplicationError> {
    let prepared = prepare_cleanup(root, &record)?;
    finish_prepared(root, record, prepared, checkpoint)
}
fn finish_prepared(
    root: &Path,
    mut record: Admission,
    prepared: Vec<CheckedSource>,
    checkpoint: impl Fn(CleanupCheckpoint) -> Result<(), ApplicationError>,
) -> Result<(), ApplicationError> {
    if !ledger::existing_directory(root)? {
        return Err(conflict("Original media storage disappeared"));
    }
    let request = &record.declaration.request_id;
    let active = root.join(format!("{request}.admission.json"));
    let current = load_record(root, &active)?
        .ok_or_else(|| conflict("Original media admission disappeared"))?;
    if ledger::encode_bounded_result(&current, 1024 * 1024)?
        != ledger::encode_bounded_result(&record, 1024 * 1024)?
    {
        return Err(conflict("Original retirement admission changed"));
    }
    let directory = root.join(format!("{request}.source"));
    if ledger::existing_directory(&directory)? {
        let mut found = 0;
        for entry in std::fs::read_dir(&directory)
            .map_err(|_| unavailable("Media source cleanup unavailable"))?
        {
            let path = entry
                .map_err(|_| unavailable("Media source cleanup unavailable"))?
                .path();
            if !prepared.iter().any(|source| {
                path.file_name().and_then(|name| name.to_str())
                    == Some(source.descriptor.name.as_str())
            }) {
                return Err(conflict(
                    "Media source appeared after retirement verification",
                ));
            }
            found += 1;
        }
        if found != prepared.len() {
            return Err(conflict(
                "Media source changed after retirement verification",
            ));
        }
        for source in prepared {
            let original = source.descriptor;
            let path = directory.join(&original.name);
            if !ledger::check_file(&path)? {
                return Err(conflict("Media retirement source changed"));
            }
            let current = source
                .file
                .metadata()
                .map_err(|_| unavailable("Media source unavailable"))?;
            if stamp(&source.file)? != (original.volume, original.index)
                || stamp(&File::open(&path).map_err(|_| unavailable("Media source unavailable"))?)?
                    != (original.volume, original.index)
                || current.len() != original.bytes
                || current
                    .modified()
                    .map_err(|_| unavailable("Media source modification evidence unavailable"))?
                    != source.modified
            {
                return Err(conflict(
                    "Media retirement source changed after verification",
                ));
            }
            // The owned descriptor and exclusive source gate outlive the exact unlink.
            std::fs::remove_file(&path)
                .map_err(|_| unknown("Media source retirement incomplete"))?;
            ledger::sync_parent(&path)?;
            checkpoint(CleanupCheckpoint::SourceRemoved)?;
        }
        // Keep the empty directory: retirement owns exact files, not a replacement directory.
    } else if !prepared.is_empty() {
        return Err(conflict("Original media source directory disappeared"));
    }
    record.retirement.as_mut().unwrap().completed = true;
    let archive = retired_path(root, request);
    let bytes = ledger::encode_bounded_result(&record, 1024 * 1024)?;
    if ledger::check_file(&archive)? {
        if ledger::read(&archive, 1024 * 1024)? != bytes {
            return Err(conflict("Media retirement receipt changed"));
        }
    } else {
        ledger::write_new(&archive, &bytes)?;
    }
    checkpoint(CleanupCheckpoint::ReceiptSaved)?;
    if ledger::check_file(&active)? {
        std::fs::remove_file(&active)
            .map_err(|_| unknown("Media retirement quota release incomplete"))?;
        ledger::sync_parent(&active)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

use hypr_loofah_protocol::{ProcessingTitleResult, ProcessingTitleStatus};

use super::*;

const MAX_TITLE_RECORD_BYTES: u64 = 16 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BodyApplied {
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TitleStage {
    Generating {},
    Candidate {
        candidate: String,
    },
    Applying {
        candidate: String,
    },
    Finished {
        status: ProcessingTitleStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        candidate: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<ProcessingJobFailure>,
    },
}

impl TitleStage {
    pub fn finished(result: ProcessingTitleResult) -> Self {
        Self::Finished {
            status: result.status,
            candidate: result.candidate,
            failure: result.failure,
        }
    }

    fn candidate(&self) -> Option<&str> {
        match self {
            Self::Generating {} => None,
            Self::Candidate { candidate } | Self::Applying { candidate } => Some(candidate),
            Self::Finished { candidate, .. } => candidate.as_deref(),
        }
    }

    fn result(&self, terminal: bool) -> ProcessingTitleResult {
        let (status, failure) = match self {
            Self::Generating {} | Self::Candidate { .. } => (
                if terminal {
                    ProcessingTitleStatus::Interrupted
                } else {
                    ProcessingTitleStatus::Generating
                },
                None,
            ),
            Self::Applying { .. } => (
                if terminal {
                    ProcessingTitleStatus::Unconfirmed
                } else {
                    ProcessingTitleStatus::Applying
                },
                None,
            ),
            Self::Finished {
                status, failure, ..
            } => (*status, *failure),
        };
        ProcessingTitleResult {
            status,
            candidate: self.candidate().map(str::to_owned),
            failure,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleRecord {
    schema_version: u32,
    vault_id: String,
    job_id: String,
    request: SummaryJobRequest,
    body_revision: String,
    stage: TitleStage,
}

impl JobJournal {
    pub(super) async fn mark_body_applied(
        &self,
        id: String,
        revision: String,
    ) -> Result<(), ApplicationError> {
        let id = parse_uuid(&id)?;
        if !valid_revision(&revision) {
            return Err(storage());
        }
        self.run(move |state, events| {
            let mut record = state.record(&id)?.clone();
            if let Some(body) = &record.body_applied {
                return if body.revision == revision {
                    Ok(())
                } else {
                    Err(transition())
                };
            }
            if record.request.title_policy != TitlePolicy::GenerateIfEmpty
                || !matches!(
                    record.snapshot.state,
                    ProcessingJobState::Running | ProcessingJobState::Cancelling
                )
                || record.snapshot.phase != Some(ProcessingPhase::Applying)
                || result::read_result(state, &record)?.is_none()
            {
                return Err(transition());
            }
            record.body_applied = Some(BodyApplied { revision });
            state.update(record, events)?;
            Ok(())
        })
        .await
    }

    pub(super) async fn advance_title(
        &self,
        id: String,
        next: TitleStage,
    ) -> Result<(), ApplicationError> {
        let id = parse_uuid(&id)?;
        self.run(move |state, events| {
            let record = state.record(&id)?;
            if matches!(next, TitleStage::Applying { .. })
                && record.snapshot.state == ProcessingJobState::Cancelling
            {
                return Err(rejected(
                    ErrorCode::Cancelled,
                    "Title application was cancelled",
                ));
            }
            if !matches!(
                record.snapshot.state,
                ProcessingJobState::Running | ProcessingJobState::Cancelling
            ) || record.body_applied.is_none()
            {
                return Err(transition());
            }
            write(state, record, next)?;
            events.publish_for(&record.namespace, Change::JobChanged { job_id: id });
            Ok(())
        })
        .await
    }
}

pub(super) fn valid_revision(revision: &str) -> bool {
    !revision.is_empty() && revision.len() <= 128 && !revision.chars().any(char::is_control)
}

fn valid_candidate(candidate: &str) -> bool {
    matches!(super::title_generation::normalize_candidate(candidate), Ok(Some(value)) if value == candidate)
}

fn validate_stage(stage: &TitleStage) -> Result<(), ApplicationError> {
    if stage
        .candidate()
        .is_some_and(|candidate| !valid_candidate(candidate))
    {
        return Err(storage());
    }
    if let TitleStage::Finished {
        status,
        candidate,
        failure,
    } = stage
        && (matches!(
            status,
            ProcessingTitleStatus::Generating | ProcessingTitleStatus::Applying
        ) || (*status == ProcessingTitleStatus::Failed) != failure.is_some()
            || *failure == Some(ProcessingJobFailure::Interrupted)
            || (*status == ProcessingTitleStatus::Applied && candidate.is_none())
            || (matches!(
                status,
                ProcessingTitleStatus::Empty | ProcessingTitleStatus::Unsupported
            ) && candidate.is_some()))
    {
        return Err(storage());
    }
    Ok(())
}

fn valid_transition(previous: Option<&TitleStage>, next: &TitleStage) -> bool {
    if previous == Some(next) {
        return true;
    }
    if let Some(previous) = previous {
        if matches!(previous, TitleStage::Finished { .. }) {
            return false;
        }
        if previous.candidate().is_some() && previous.candidate() != next.candidate() {
            return false;
        }
    }
    match next {
        TitleStage::Generating {} => previous.is_none(),
        TitleStage::Candidate { .. } => matches!(previous, Some(TitleStage::Generating {})),
        TitleStage::Applying { .. } => matches!(previous, Some(TitleStage::Candidate { .. })),
        TitleStage::Finished {
            status: ProcessingTitleStatus::Applied,
            ..
        } => matches!(previous, Some(TitleStage::Applying { .. })),
        TitleStage::Finished { .. } => true,
    }
}

fn read(state: &State, job: &Record) -> Result<Option<TitleStage>, ApplicationError> {
    let root = state.root.join("titles");
    match root.symlink_metadata() {
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        _ => return Err(storage()),
    }
    let path = root.join(format!("{}.json", job.snapshot.id));
    let metadata = match path.symlink_metadata() {
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= MAX_TITLE_RECORD_BYTES =>
        {
            metadata
        }
        _ => return Err(storage()),
    };
    let file = File::open(path).map_err(|_| storage())?;
    if !file.metadata().map_err(|_| storage())?.is_file() {
        return Err(storage());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_TITLE_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| storage())?;
    if bytes.len() as u64 > MAX_TITLE_RECORD_BYTES {
        return Err(storage());
    }
    let record: TitleRecord = serde_json::from_slice(&bytes).map_err(|_| storage())?;
    if record.schema_version != 1
        || record.vault_id != state.vault_id
        || record.job_id != job.snapshot.id
        || record.request != job.request
        || job.request.title_policy != TitlePolicy::GenerateIfEmpty
        || job.body_applied.as_ref().map(|body| body.revision.as_str())
            != Some(record.body_revision.as_str())
    {
        return Err(storage());
    }
    validate_stage(&record.stage)?;
    Ok(Some(record.stage))
}

fn write(state: &State, job: &Record, next: TitleStage) -> Result<(), ApplicationError> {
    let body = job.body_applied.as_ref().ok_or_else(transition)?;
    validate_stage(&next)?;
    let previous = read(state, job)?;
    if !valid_transition(previous.as_ref(), &next) {
        return Err(transition());
    }
    if previous.as_ref() == Some(&next) {
        return Ok(());
    }
    let record = TitleRecord {
        schema_version: 1,
        vault_id: state.vault_id.clone(),
        job_id: job.snapshot.id.clone(),
        request: job.request.clone(),
        body_revision: body.revision.clone(),
        stage: next,
    };
    let bytes = serde_json::to_string(&record).map_err(|_| storage())?;
    if bytes.len() as u64 > MAX_TITLE_RECORD_BYTES {
        return Err(capacity());
    }
    let root = state.root.join("titles");
    ensure_directory(&root)?;
    hypr_storage::fs::atomic_write(&root.join(format!("{}.json", record.job_id)), &bytes)
        .map_err(|_| storage())?;
    #[cfg(unix)]
    File::open(root)
        .and_then(|file| file.sync_all())
        .map_err(|_| storage())?;
    Ok(())
}

pub(super) fn project(state: &State, job: &Record) -> Option<ProcessingTitleResult> {
    job.body_applied.as_ref()?;
    Some(match read(state, job) {
        Ok(Some(stage)) => stage.result(job.snapshot.state.is_terminal()),
        Ok(None) => ProcessingTitleResult {
            status: if job.snapshot.state.is_terminal() {
                ProcessingTitleStatus::Interrupted
            } else {
                ProcessingTitleStatus::Generating
            },
            candidate: None,
            failure: None,
        },
        Err(_) => ProcessingTitleResult {
            status: ProcessingTitleStatus::Unconfirmed,
            candidate: None,
            failure: None,
        },
    })
}

pub(super) fn interrupt(state: &State, job: &Record) {
    if let Ok(Some(stage)) = read(state, job)
        && !matches!(stage, TitleStage::Finished { .. })
    {
        let _ = write(state, job, TitleStage::finished(stage.result(true)));
    }
}

pub(super) fn checkpoint(_stage: &str) {
    #[cfg(test)]
    if std::env::var("LOOFAH_TITLE_CRASH_STAGE").as_deref() == Ok(_stage) {
        std::process::exit(91);
    }
}

#[cfg(test)]
mod tests;

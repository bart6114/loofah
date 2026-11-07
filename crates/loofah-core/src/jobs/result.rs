use super::*;

const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_RESULT_BYTES: u64 = (MAX_OUTPUT_BYTES * 6) as u64 + MAX_RECORD_BYTES;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedDocument {
    pub markdown: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_tags: Option<Vec<hypr_vault_write::ScoredTagSuggestion>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultRecord {
    schema_version: u32,
    vault_id: String,
    job_id: String,
    request: SummaryJobRequest,
    output: GeneratedDocument,
}

impl JobJournal {
    pub async fn retain_result(
        &self,
        id: String,
        output: GeneratedDocument,
    ) -> Result<(), ApplicationError> {
        let id = parse_uuid(&id)?;
        validate_output(&output)?;
        self.run(move |state, _| {
            let job = state.record(&id)?.clone();
            let existing = read_result(state, &job)?;
            if let Some(existing) = existing {
                return if existing == output {
                    Ok(())
                } else {
                    Err(transition())
                };
            }
            if !matches!(
                job.snapshot.state,
                ProcessingJobState::Running | ProcessingJobState::Cancelling
            ) {
                return Err(transition());
            }
            let record = ResultRecord {
                schema_version: 1,
                vault_id: state.vault_id.clone(),
                job_id: id,
                request: job.request,
                output,
            };
            let result = (|| {
                let root = state.root.join("results");
                ensure_directory(&root)?;
                let bytes = serde_json::to_string(&record).map_err(|_| storage())?;
                if bytes.len() as u64 > MAX_RESULT_BYTES {
                    return Err(capacity());
                }
                let path = root.join(format!("{}.json", record.job_id));
                hypr_storage::fs::atomic_write(&path, &bytes).map_err(|_| storage())?;
                #[cfg(unix)]
                File::open(root)
                    .and_then(|file| file.sync_all())
                    .map_err(|_| storage())?;
                Ok(())
            })();
            if result.is_err() {
                state.failed = true;
            }
            result
        })
        .await
    }

    pub async fn result(&self, id: String) -> Result<Option<GeneratedDocument>, ApplicationError> {
        let id = parse_uuid(&id)?;
        self.run(move |state, _| read_result(state, state.record(&id)?))
            .await
    }

    pub(super) async fn work(&self, id: String) -> Result<worker::JobWork, ApplicationError> {
        let id = parse_uuid(&id)?;
        self.run(move |state, _| {
            let record = state.record(&id)?;
            Ok(worker::JobWork {
                principal: record.namespace.clone(),
                request: record.request.clone(),
            })
        })
        .await
    }

    pub(super) async fn queued(&self) -> Result<Vec<ProcessingJobSnapshot>, ApplicationError> {
        self.run(|state, _| {
            let mut jobs: Vec<_> = state
                .records
                .values()
                .filter(|record| record.snapshot.state == ProcessingJobState::Queued)
                .map(|record| record.snapshot.clone())
                .collect();
            jobs.sort_by(|a, b| (&a.created_at, &a.id).cmp(&(&b.created_at, &b.id)));
            Ok(jobs)
        })
        .await
    }

    pub(super) async fn begin_apply(&self, id: String) -> Result<bool, ApplicationError> {
        let id = parse_uuid(&id)?;
        self.run(move |state, events| {
            let mut record = state.record(&id)?.clone();
            if record.snapshot.state != ProcessingJobState::Running {
                return Ok(false);
            }
            record.snapshot.phase = Some(ProcessingPhase::Applying);
            state.update(record, events)?;
            Ok(true)
        })
        .await
    }
}

pub(super) fn read_result(
    state: &State,
    job: &Record,
) -> Result<Option<GeneratedDocument>, ApplicationError> {
    let root = state.root.join("results");
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
                && metadata.len() <= MAX_RESULT_BYTES =>
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
    file.take(MAX_RESULT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| storage())?;
    if bytes.len() as u64 > MAX_RESULT_BYTES {
        return Err(storage());
    }
    let record: ResultRecord = serde_json::from_slice(&bytes).map_err(|_| storage())?;
    if record.schema_version != 1
        || record.vault_id != state.vault_id
        || record.job_id != job.snapshot.id
        || record.request != job.request
    {
        return Err(storage());
    }
    validate_output(&record.output).map_err(|_| storage())?;
    Ok(Some(record.output))
}

pub(super) fn validate_output(output: &GeneratedDocument) -> Result<(), ApplicationError> {
    if output.markdown.trim().is_empty() || output.markdown.len() > MAX_OUTPUT_BYTES {
        return Err(rejected(
            ErrorCode::Validation,
            "Generated document is empty or too large",
        ));
    }
    if output.suggested_tags.as_ref().is_some_and(|tags| {
        tags.len() > 3
            || tags.iter().any(|tag| {
                tag.name.trim().is_empty()
                    || tag.name.encode_utf16().count() > 120
                    || !tag.confidence.is_finite()
                    || !(0.0..=1.0).contains(&tag.confidence)
            })
    }) {
        return Err(rejected(
            ErrorCode::Validation,
            "Invalid generated tag suggestions",
        ));
    }
    Ok(())
}

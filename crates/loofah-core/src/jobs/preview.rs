use super::*;
use hypr_loofah_protocol::{
    MAX_PROCESSING_PREVIEW_BYTES, ProcessingJobPreview, ProcessingPreviewSnapshot,
};
use std::time::{Duration, Instant};
use tokio::sync::watch;

const INTERVAL: Duration = Duration::from_millis(500);
const RETAINED: usize = 32;
const FILE_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct GenerationPreview(Arc<Observer>);
struct Observer {
    state: Mutex<ObserverState>,
    updates: watch::Sender<Option<ProcessingPreviewSnapshot>>,
}
struct ObserverState {
    epoch: String,
    sequence: u64,
    emitted: Instant,
}
impl GenerationPreview {
    pub(super) fn channel() -> (Self, watch::Receiver<Option<ProcessingPreviewSnapshot>>) {
        let (updates, receiver) = watch::channel(None);
        (
            Self(Arc::new(Observer {
                state: Mutex::new(ObserverState {
                    epoch: uuid::Uuid::new_v4().to_string(),
                    sequence: 0,
                    emitted: Instant::now(),
                }),
                updates,
            })),
            receiver,
        )
    }
    pub(super) fn reset(&self) {
        let mut state = self.0.state.lock().unwrap();
        state.epoch = uuid::Uuid::new_v4().to_string();
        state.sequence = 1;
        state.emitted = Instant::now();
        self.0.updates.send_replace(Some(ProcessingPreviewSnapshot {
            cursor: format!("{}:1", state.epoch),
            markdown: String::new(),
            truncated: false,
        }));
    }
    pub(super) fn update(&self, markdown: &str, final_update: bool) {
        let mut state = self.0.state.lock().unwrap();
        if !final_update && state.emitted.elapsed() < INTERVAL {
            return;
        }
        let prefix = prefix(markdown, MAX_PROCESSING_PREVIEW_BYTES);
        let truncated = prefix.len() < markdown.len();
        let same =
            self.0.updates.borrow().as_ref().is_some_and(|previous| {
                previous.markdown == prefix && previous.truncated == truncated
            });
        if same {
            return;
        }
        state.sequence = state.sequence.saturating_add(1);
        state.emitted = Instant::now();
        self.0.updates.send_replace(Some(ProcessingPreviewSnapshot {
            cursor: format!("{}:{}", state.epoch, state.sequence),
            markdown: prefix.into(),
            truncated,
        }));
    }
    pub(super) fn current(&self) -> Option<ProcessingPreviewSnapshot> {
        self.0.updates.borrow().clone()
    }
}
fn prefix(value: &str, limit: usize) -> &str {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
fn valid_cursor(value: &str) -> bool {
    value.split_once(':').is_some_and(|(epoch, sequence)| {
        parse_uuid(epoch).is_ok() && sequence.parse::<u64>().is_ok_and(|sequence| sequence > 0)
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPreview {
    schema_version: u32,
    vault_id: String,
    namespace: String,
    request_id: String,
    job_id: String,
    request: SummaryJobRequest,
    preview: ProcessingPreviewSnapshot,
}
impl JobJournal {
    pub(super) async fn checkpoint_preview(
        &self,
        id: String,
        preview: ProcessingPreviewSnapshot,
    ) -> Result<(), ApplicationError> {
        let id = parse_uuid(&id)?;
        if !valid_cursor(&preview.cursor) || preview.markdown.len() > MAX_PROCESSING_PREVIEW_BYTES {
            return Err(capacity());
        }
        self.run(move |state, events| {
            let job = state.record(&id)?.clone();
            if !matches!(
                job.snapshot.state,
                ProcessingJobState::Running | ProcessingJobState::Cancelling
            ) {
                return Err(transition());
            }
            let root = state.root.join("previews");
            ensure_directory(&root)?;
            let path = root.join(format!("{id}.json"));
            if !regular_file(&path)? {
                reserve(state, &root, &id)?;
            }
            let mut record = StoredPreview {
                schema_version: 1,
                vault_id: state.vault_id.clone(),
                namespace: job.namespace,
                request_id: job.request_id,
                job_id: id.clone(),
                request: job.request,
                preview,
            };
            let bytes = loop {
                let bytes = serde_json::to_string(&record).map_err(|_| storage())?;
                if bytes.len() <= FILE_BYTES {
                    break bytes;
                }
                if record.preview.markdown.is_empty() {
                    return Err(capacity());
                }
                let length = record.preview.markdown.len() / 2;
                record.preview.markdown = prefix(&record.preview.markdown, length).into();
                record.preview.truncated = true;
            };
            hypr_storage::fs::atomic_write(&path, &bytes).map_err(|_| storage())?;
            #[cfg(unix)]
            File::open(root)
                .and_then(|file| file.sync_all())
                .map_err(|_| storage())?;
            events.publish_preview_for(&record.namespace, id);
            Ok(())
        })
        .await
    }
    pub async fn owned_preview(
        &self,
        namespace: String,
        id: String,
    ) -> Result<ProcessingJobPreview, ApplicationError> {
        validate_namespace(&namespace)?;
        let id = parse_uuid(&id)?;
        self.run(move |state, _| {
            let job = state.owned(&namespace, &id)?;
            Ok(ProcessingJobPreview {
                vault_id: state.vault_id.clone(),
                job: job.snapshot.clone(),
                preview: read(state, job).ok().flatten(),
            })
        })
        .await
    }
}
fn regular_file(path: &Path) -> Result<bool, ApplicationError> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(true),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(false),
        _ => Err(storage()),
    }
}
fn reserve(state: &State, root: &Path, current: &str) -> Result<(), ApplicationError> {
    let mut retained = Vec::new();
    for (scanned, entry) in std::fs::read_dir(root).map_err(|_| storage())?.enumerate() {
        if scanned >= RETAINED * 2 {
            return Err(capacity());
        }
        let entry = entry.map_err(|_| storage())?;
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
            continue;
        };
        if parse_uuid(id).is_err() || !regular_file(&entry.path())? {
            return Err(storage());
        }
        let job = state.record(id)?;
        let meta = entry.metadata().map_err(|_| storage())?;
        if meta.len() > FILE_BYTES as u64 {
            return Err(capacity());
        }
        retained.push((
            meta.modified().map_err(|_| storage())?,
            id.to_owned(),
            job.snapshot.state.is_terminal(),
        ));
    }
    retained.sort_unstable();
    let excess = retained.len().saturating_add(1).saturating_sub(RETAINED);
    let removable: Vec<_> = retained
        .into_iter()
        .filter(|(_, id, terminal)| *terminal && id != current)
        .take(excess)
        .collect();
    if removable.len() != excess {
        return Err(capacity());
    }
    for (_, id, _) in removable {
        std::fs::remove_file(root.join(format!("{id}.json"))).map_err(|_| storage())?;
    }
    Ok(())
}
fn read(
    state: &State,
    job: &Record,
) -> Result<Option<ProcessingPreviewSnapshot>, ApplicationError> {
    let root = state.root.join("previews");
    if !root.try_exists().map_err(|_| storage())? {
        return Ok(None);
    }
    crate::transcription_jobs::ledger::existing_directory(&root)?;
    let path = root.join(format!("{}.json", job.snapshot.id));
    if !regular_file(&path)? {
        return Ok(None);
    }
    let bytes = crate::transcription_jobs::ledger::read(&path, FILE_BYTES as u64)?;
    let record: StoredPreview = serde_json::from_slice(&bytes).map_err(|_| storage())?;
    if record.schema_version != 1
        || record.vault_id != state.vault_id
        || record.namespace != job.namespace
        || record.request_id != job.request_id
        || record.job_id != job.snapshot.id
        || record.request != job.request
        || !valid_cursor(&record.preview.cursor)
        || record.preview.markdown.len() > MAX_PROCESSING_PREVIEW_BYTES
    {
        return Err(storage());
    }
    Ok(Some(record.preview))
}

#[cfg(test)]
mod tests;

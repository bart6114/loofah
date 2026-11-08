use super::*;
use crate::session_lifecycle_gate::session_lifecycle_busy;

impl JobJournal {
    pub(super) async fn replay_submission(
        &self,
        namespace: String,
        request_id: String,
        request: SummaryJobRequest,
    ) -> Result<Option<ProcessingJobSnapshot>, ApplicationError> {
        validate_namespace(&namespace)?;
        validate_request(&request)?;
        let request_id = parse_uuid(&request_id)?;
        self.run(move |state, _| {
            let Some(record) = state.records.get(&key(&namespace, &request_id)) else {
                return Ok(None);
            };
            if record.request != request {
                return Err(rejected(
                    ErrorCode::Conflict,
                    "The job request ID was already used for different input",
                ));
            }
            Ok(Some(record.snapshot.clone()))
        })
        .await
    }
    pub(crate) async fn ensure_session_idle(&self, id: String) -> Result<(), ApplicationError> {
        self.run(move |state, _| {
            if state.records.values().any(|record| {
                record.request.session_id == id && !record.snapshot.state.is_terminal()
            }) {
                return Err(session_lifecycle_busy());
            }
            Ok(())
        })
        .await
    }
}

pub(crate) async fn ensure_stored_session_idle(
    root: PathBuf,
    vault: String,
    id: String,
) -> Result<(), ApplicationError> {
    tokio::task::spawn_blocking(move || {
        match root.symlink_metadata() {
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            _ => return Err(storage()),
        }
        let mut ids = std::collections::HashSet::new();
        for (scanned, entry) in std::fs::read_dir(&root).map_err(|_| storage())?.enumerate() {
            if scanned >= 20_000 {
                return Err(capacity());
            }
            let entry = entry.map_err(|_| storage())?;
            let name = entry.file_name();
            let Some(key) = name
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .filter(|key| is_key(key))
            else {
                continue;
            };
            if ids.len() >= MAX_JOBS {
                return Err(capacity());
            }
            let record = read_record(&entry.path())?;
            validate_record(&vault, key, &record)?;
            if !ids.insert(record.snapshot.id.clone()) {
                return Err(storage());
            }
            if record.request.session_id == id && !record.snapshot.state.is_terminal() {
                return Err(session_lifecycle_busy());
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| storage())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn idle_checks_include_all_members_and_scan_disabled_jobs_without_mutating_them() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("jobs");
        let journal = JobJournal::open(directory.clone(), EventHub::new("vault".into(), 16))
            .await
            .unwrap();
        let job = journal
            .submit(
                "alice".into(),
                uuid::Uuid::new_v4().to_string(),
                SummaryJobRequest {
                    title_policy: Default::default(),
                    session_id: "note".into(),
                    target: DocumentTarget::Summary,
                    expected_revision: "revision".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            journal
                .ensure_session_idle("note".into())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert!(journal.ensure_session_idle("other".into()).await.is_ok());
        let file = std::fs::read_dir(&directory)
            .unwrap()
            .map(Result::unwrap)
            .find(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            })
            .unwrap()
            .path();
        let before = std::fs::read(&file).unwrap();
        assert_eq!(
            ensure_stored_session_idle(directory.clone(), "vault".into(), "note".into())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(std::fs::read(&file).unwrap(), before);
        journal.cancel_owned("alice".into(), job.id).await.unwrap();
        drop(journal);
        assert!(
            ensure_stored_session_idle(directory.clone(), "vault".into(), "note".into())
                .await
                .is_ok()
        );
        let mut invalid: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        invalid["schema_version"] = serde_json::json!(999);
        std::fs::write(&file, serde_json::to_vec(&invalid).unwrap()).unwrap();
        let original = std::fs::read(&file).unwrap();
        assert!(
            ensure_stored_session_idle(directory, "vault".into(), "note".into())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(file).unwrap(), original);
    }
}

use super::*;

/// Immutable byte revisions. No descriptor/operation guard survives inference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessingSourceBinding {
    pub recording_sha256: String,
    pub transcript_revision: String,
    pub transcript_present: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn fixture() -> (tempfile::TempDir, SessionStore, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        let store = SessionStore::new(path.clone());
        let meta=serde_json::from_value(serde_json::json!({"id":"s","title":"Title","created_at":"2026-11-08T00:00:00Z","tags":[]})).unwrap();
        store.create_note_session(&meta, "Notes").await.unwrap();
        let directory = path.join("sessions/s");
        std::fs::write(directory.join("audio.wav"), b"immutable source").unwrap();
        std::fs::write(directory.join("transcript.json"), br#"{"transcripts":[]}"#).unwrap();
        (root, store, directory)
    }
    #[tokio::test]
    async fn source_guards_allow_ordinary_note_edits_and_detect_both_byte_revisions() {
        let (_root, store, directory) = fixture().await;
        let baseline = store
            .guard_processing_source("s", None)
            .await
            .unwrap()
            .unwrap()
            .binding()
            .clone();
        store
            .save_note("s", "Changed note", Some("Changed title"))
            .await
            .unwrap();
        let guard = store
            .guard_processing_source("s", Some(baseline.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(guard.binding(), &baseline);
        drop(guard);
        std::fs::write(
            directory.join("transcript.json"),
            br#"{"transcripts":[],"future":true}"#,
        )
        .unwrap();
        assert!(matches!(
            store
                .guard_processing_source("s", Some(baseline.clone()))
                .await,
            Err(StoreError::Conflict(_))
        ));
        std::fs::write(directory.join("transcript.json"), br#"{"transcripts":[]}"#).unwrap();
        std::fs::write(directory.join("audio.wav"), b"replaced source!").unwrap();
        assert!(matches!(
            store.guard_processing_source("s", Some(baseline)).await,
            Err(StoreError::Conflict(_))
        ));
    }
    #[tokio::test]
    async fn guard_retains_transcript_serialization_without_holding_global_write_lock() {
        let (_root, store, _) = fixture().await;
        let guard = store
            .guard_processing_source("s", None)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.save_note("s", "Responsive", None),
        )
        .await
        .unwrap()
        .unwrap();
        let waiting = store.transcript_operations.lock("s");
        tokio::pin!(waiting);
        tokio::select! {biased;_= &mut waiting=>panic!("source guard lost transcript ownership"),_=tokio::task::yield_now()=>{}}
        drop(guard);
        tokio::time::timeout(std::time::Duration::from_secs(2), waiting)
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn absent_transcript_binding_distinguishes_presence_even_for_equal_revision_domain_bytes()
    {
        let (_root, store, directory) = fixture().await;
        std::fs::remove_file(directory.join("transcript.json")).unwrap();
        let baseline = store
            .guard_processing_source("s", None)
            .await
            .unwrap()
            .unwrap()
            .binding()
            .clone();
        assert!(!baseline.transcript_present);
        std::fs::write(
            directory.join("transcript.json"),
            b"loofah/transcript-revision/v1/absent",
        )
        .unwrap();
        let current = store
            .guard_processing_source("s", None)
            .await
            .unwrap()
            .unwrap()
            .binding()
            .clone();
        assert!(current.transcript_present);
        assert_eq!(baseline.transcript_revision, current.transcript_revision);
        assert!(matches!(
            store.guard_processing_source("s", Some(baseline)).await,
            Err(StoreError::Conflict(_))
        ));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn guard_never_follows_transcript_symlinks_or_touches_user_files() {
        let (_root, store, directory) = fixture().await;
        let target = directory.join("private-attachment.json");
        std::fs::write(&target, b"attachment").unwrap();
        std::fs::remove_file(directory.join("transcript.json")).unwrap();
        std::os::unix::fs::symlink(&target, directory.join("transcript.json")).unwrap();
        assert!(store.guard_processing_source("s", None).await.is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"attachment");
    }
}

/// Hold only across the checked generated-document apply, after fresh auth.
pub struct ProcessingSourceGuard {
    binding: ProcessingSourceBinding,
    _recording: FileSnapshot,
    _transcript_file: Option<FileSnapshot>,
    _audio: tokio::sync::OwnedMutexGuard<()>,
    _transcript: TranscriptOperation,
    _store: SessionStore,
}
impl ProcessingSourceGuard {
    pub fn binding(&self) -> &ProcessingSourceBinding {
        &self.binding
    }
}

impl SessionStore {
    pub async fn guard_processing_source(
        &self,
        id: &str,
        expected: Option<ProcessingSourceBinding>,
    ) -> Result<Option<ProcessingSourceGuard>, StoreError> {
        crate::validate_session_id(id)?;
        let store = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let audio = store.lock_session_audio(&id).await?;
            let transcript = store.transcript_operations.lock(&id).await;
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            let Some(meta) = store
                .metadata_snapshot_locked(&snapshot._guard, &id)
                .await?
            else {
                return Ok(None);
            };
            if busy(&store, &id, &meta.meta).await {
                return Err(StoreError::Conflict("Recording is busy".into()));
            }
            let directory = store
                .vault_base
                .join(store.session_dir_locked(&snapshot._guard, &id).await?);
            store
                .clone()
                .spawn_blocking(move || {
                    let recording = io::select(&directory)?.map(|(_, file)| file);
                    let transcript_file = io::snapshot(&directory.join("transcript.json"))?;
                    // Hashing does not hold the global write/snapshot lock.
                    drop(snapshot);
                    let Some(recording) = recording else {
                        return Ok(None);
                    };
                    if !recording.lock_for_recheck()? {
                        return Err(StoreError::Conflict("Processing source is busy".into()));
                    }
                    if let Some(file) = &transcript_file
                        && !file.lock_for_recheck()?
                    {
                        return Err(StoreError::Conflict("Processing source is busy".into()));
                    }
                    let binding = ProcessingSourceBinding {
                        recording_sha256: io::hash_bounded(recording.file(), u32::MAX as u64)?,
                        transcript_present: transcript_file.is_some(),
                        transcript_revision: match &transcript_file {
                            Some(file) => io::hash_bounded(file.file(), io::RESULT_LIMIT as u64)?,
                            None => io::absent_revision(),
                        },
                    };
                    let Some((_, current)) = io::select(&directory)? else {
                        return Err(StoreError::Conflict(
                            "Processing recording disappeared".into(),
                        ));
                    };
                    let current_transcript = io::snapshot(&directory.join("transcript.json"))?;
                    let same_transcript = match (&transcript_file, &current_transcript) {
                        (Some(a), Some(b)) => a.unchanged(b)?,
                        (None, None) => true,
                        _ => false,
                    };
                    if !recording.unchanged(&current)?
                        || !same_transcript
                        || expected
                            .as_ref()
                            .is_some_and(|expected| expected != &binding)
                    {
                        return Err(StoreError::Conflict("Processing source changed".into()));
                    }
                    Ok(Some(ProcessingSourceGuard {
                        binding,
                        _recording: recording,
                        _transcript_file: transcript_file,
                        _audio: audio,
                        _transcript: transcript,
                        _store: store,
                    }))
                })
                .await
                .map_err(io::error)?
        })
        .await
        .map_err(io::error)?
    }
}

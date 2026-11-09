use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptOnlyBinding {
    pub session_created_at: String,
    pub transcript_revision: String,
    pub transcript_present: bool,
}
pub struct TranscriptOnlyGuard {
    binding: TranscriptOnlyBinding,
    _transcript_file: Option<FileSnapshot>,
    _audio: tokio::sync::OwnedMutexGuard<()>,
    _transcript: TranscriptOperation,
    _store: SessionStore,
}
impl TranscriptOnlyGuard {
    pub fn binding(&self) -> &TranscriptOnlyBinding {
        &self.binding
    }
}
#[derive(Clone, Debug)]
pub struct CompletedTranscriptPublication {
    pub session_id: String,
    pub transcript_id: String,
    pub revision: String,
    pub recording_sha256: Option<String>,
    pub empty: bool,
}
impl SessionStore {
    pub async fn completed_transcript_publication(
        &self,
        operations: &Path,
        identity: TranscriptPublicationIdentity,
    ) -> Result<Option<CompletedTranscriptPublication>, StoreError> {
        identity.validate()?;
        if !operations.is_absolute() {
            return Err(StoreError::Serialize(
                "Publication root must be absolute".into(),
            ));
        }
        let path = io::paths(operations, &identity).0;
        self.spawn_blocking(move || {
            let Some(receipt) = io::read_receipt(&path, &identity)? else {
                return Ok(None);
            };
            if !matches!(receipt.state, State::Committed | State::Empty) {
                return Ok(None);
            }
            Ok(Some(CompletedTranscriptPublication {
                session_id: receipt.binding.session_id,
                transcript_id: receipt.transcript_id,
                revision: receipt.result_sha256,
                recording_sha256: receipt
                    .binding
                    .subtitle
                    .is_none()
                    .then_some(receipt.binding.recording_sha256),
                empty: receipt.state == State::Empty,
            }))
        })
        .await
        .map_err(io::error)?
    }
    pub async fn guard_transcript_only_source(
        &self,
        id: &str,
        expected: TranscriptOnlyBinding,
    ) -> Result<Option<TranscriptOnlyGuard>, StoreError> {
        crate::validate_session_id(id)?;
        if !io::valid_hash(&expected.transcript_revision)
            || chrono::DateTime::parse_from_rfc3339(&expected.session_created_at).is_err()
        {
            return Err(StoreError::Serialize(
                "Invalid transcript-only source binding".into(),
            ));
        }
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
            if meta.meta.created_at != expected.session_created_at
                || busy(&store, &id, &meta.meta).await
            {
                return Err(StoreError::Conflict(
                    "Transcript-only source is busy or changed".into(),
                ));
            }
            let directory = store
                .vault_base
                .join(store.session_dir_locked(&snapshot._guard, &id).await?);
            store
                .clone()
                .spawn_blocking(move || {
                    let file = io::snapshot(&directory.join("transcript.json"))?;
                    drop(snapshot);
                    if let Some(file) = &file
                        && !file.lock_for_recheck()?
                    {
                        return Err(StoreError::Conflict(
                            "Transcript-only source is busy".into(),
                        ));
                    }
                    let actual = TranscriptOnlyBinding {
                        session_created_at: meta.meta.created_at,
                        transcript_present: file.is_some(),
                        transcript_revision: match &file {
                            Some(file) => io::hash_bounded(file.file(), u64::MAX)?,
                            None => io::absent_revision(),
                        },
                    };
                    let current = io::snapshot(&directory.join("transcript.json"))?;
                    let unchanged = match (&file, &current) {
                        (Some(a), Some(b)) => a.unchanged(b)?,
                        (None, None) => true,
                        _ => false,
                    };
                    if actual != expected || !unchanged {
                        return Err(StoreError::Conflict(
                            "Transcript-only source changed".into(),
                        ));
                    }
                    Ok(Some(TranscriptOnlyGuard {
                        binding: actual,
                        _transcript_file: file,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn transcript_only_guard_needs_no_audio_preserves_note_edits_and_checks_raw_presence() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sessions/note");
        let store = SessionStore::new(root.path().to_owned());
        let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Note","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
        store.create_note_session(&meta, "Note").await.unwrap();
        let original = b"{\"transcripts\":[],\"future\":true}";
        std::fs::write(directory.join("transcript.json"), original).unwrap();
        let binding = TranscriptOnlyBinding {
            session_created_at: "2026-11-08T10:00:00Z".into(),
            transcript_present: true,
            transcript_revision: crate::sha256(original),
        };
        let guard = store
            .guard_transcript_only_source("note", binding.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(guard.binding(), &binding);
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.save_note("note", "Responsive note", None),
        )
        .await
        .unwrap()
        .unwrap();
        let waiting = store.transcript_operations.lock("note");
        tokio::pin!(waiting);
        tokio::select! {biased;_= &mut waiting=>panic!("source guard lost transcript ownership"),_=tokio::task::yield_now()=>{}}
        drop(guard);
        drop(waiting.await);
        std::fs::write(
            directory.join("transcript.json"),
            b"{\"transcripts\":[],\"future\":false}",
        )
        .unwrap();
        assert!(matches!(
            store
                .guard_transcript_only_source("note", binding.clone())
                .await,
            Err(StoreError::Conflict(_))
        ));
        std::fs::write(directory.join("transcript.json"), original).unwrap();
        let mut wrong = binding.clone();
        wrong.session_created_at = "2026-11-09T10:00:00Z".into();
        assert!(matches!(
            store.guard_transcript_only_source("note", wrong).await,
            Err(StoreError::Conflict(_))
        ));
        std::fs::remove_file(directory.join("transcript.json")).unwrap();
        assert!(matches!(
            store.guard_transcript_only_source("note", binding).await,
            Err(StoreError::Conflict(_))
        ));
    }
    #[tokio::test]
    async fn prospective_absent_document_revision_matches_only_the_original_session_target() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path().to_owned());
        let expected = SessionStore::absent_document_revision(
            "note",
            &hypr_loofah_protocol::DocumentTarget::Summary,
        )
        .unwrap();
        let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Note","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
        store.create_note_session(&meta, "Note").await.unwrap();
        let actual = store
            .read_recovered_document_snapshot(
                None,
                "note",
                &hypr_loofah_protocol::DocumentTarget::Summary,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.revision, expected);
        assert_ne!(
            SessionStore::absent_document_revision(
                "another",
                &hypr_loofah_protocol::DocumentTarget::Summary
            )
            .unwrap(),
            expected
        );
    }
}

use std::{io::Read, path::PathBuf};

use hypr_loofah_protocol::{
    AssignTranscriptSpeakersRequest, TranscriptEditorSaveOutcome, TranscriptEditorSnapshot,
};
use serde_json::Value;

use crate::{
    SessionStore, StoreError, file_snapshot::FileSnapshot, metadata_revision::UniqueValue, paths,
};

struct Source {
    snapshot: TranscriptEditorSnapshot,
    raw: Value,
    path: PathBuf,
    file: Option<FileSnapshot>,
}

impl SessionStore {
    pub async fn read_transcript_editor(
        &self,
        id: &str,
    ) -> Result<Option<TranscriptEditorSnapshot>, StoreError> {
        let store = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            crate::validate_session_id(&id)?;
            let Some(source) = store.transcript_editor_source(&id).await? else {
                return Ok(None);
            };
            let guard = store.lock_writes().await;
            if !store.transcript_editor_current(&guard, &source).await? {
                return Err(changed());
            }
            Ok(Some(source.snapshot))
        })
        .await
        .map_err(join)?
    }

    pub async fn assign_transcript_speakers_checked(
        &self,
        id: &str,
        input: AssignTranscriptSpeakersRequest,
    ) -> Result<Option<TranscriptEditorSaveOutcome>, StoreError> {
        self.assign_transcript_speakers_checked_owned(id, input, Box::new(()))
            .await
    }

    pub async fn assign_transcript_speakers_checked_owned(
        &self,
        id: &str,
        input: AssignTranscriptSpeakersRequest,
        owner: Box<dyn Send>,
    ) -> Result<Option<TranscriptEditorSaveOutcome>, StoreError> {
        hypr_loofah_protocol::validate_transcript_speakers(&input)
            .map_err(|cause| StoreError::Serialize(cause.into()))?;
        let store = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            crate::validate_session_id(&id)?;
            let _operation = store.transcript_operations.try_lock(&id).ok_or_else(busy)?;
            let Some(mut source) = store.transcript_editor_source(&id).await? else {
                return Ok(None);
            };
            if source.snapshot.revision != input.expected_revision {
                let guard = store.lock_writes().await;
                if !store.transcript_editor_current(&guard, &source).await? {
                    return Err(changed());
                }
                return Ok(Some(TranscriptEditorSaveOutcome::Conflict(source.snapshot)));
            }
            let limit = store.content_limits.transcript_read_bytes;
            let operation = _operation;
            let (source, bytes, parsed, summary, _operation, _owner) = store
                .spawn_blocking(move || {
                    for assignment in &input.assignments {
                        apply(&mut source.raw, assignment)?;
                    }
                    let bytes = encode(&source.raw, limit)?;
                    let parsed: hypr_fs_format::TranscriptJson =
                        serde_json::from_value(source.raw.clone()).map_err(json)?;
                    let summary =
                        crate::transcript::summarize_transcripts(&bytes, &parsed.transcripts);
                    Ok::<_, StoreError>((source, bytes, parsed, summary, operation, owner))
                })
                .await
                .map_err(join)??;
            let guard = store.lock_writes().await;
            if !store.transcript_editor_current(&guard, &source).await? {
                return Err(changed());
            }
            let Some(meta) = store.metadata_snapshot_locked(&guard, &id).await? else {
                return Ok(None);
            };
            if store.is_recording(&id)
                || store.live.lock().await.contains_key(&id)
                || meta
                    .meta
                    .extra
                    .get("audio_import_pending")
                    .and_then(Value::as_bool)
                    == Some(true)
            {
                return Err(busy());
            }
            let directory = store.session_dir_locked(&guard, &id).await?;
            let revision = revision(&id, &source.snapshot.session_created_at, true, &bytes);
            store
                .write_file_locked(&guard, paths::transcript_path_in(&directory), bytes)
                .await?;
            store.index_set_transcript_summary(&id, summary);
            store.notify_index_changed(crate::IndexEntity::Transcripts, vec![id.clone()]);
            Ok(Some(TranscriptEditorSaveOutcome::Saved(
                TranscriptEditorSnapshot {
                    revision,
                    transcripts: parsed.transcripts,
                    ..source.snapshot
                },
            )))
        })
        .await
        .map_err(join)?
    }

    async fn transcript_editor_source(&self, id: &str) -> Result<Option<Source>, StoreError> {
        self.ensure_ready()?;
        self.ensure_append_ready(id)?;
        let guard = self.lock_writes().await;
        let Some(meta) = self.metadata_snapshot_locked(&guard, id).await? else {
            return Ok(None);
        };
        let directory = self
            .vault_base
            .join(self.session_dir_locked(&guard, id).await?);
        drop(guard);
        let id = id.to_owned();
        let vault = self.vault_base.clone();
        let limit = self.content_limits.transcript_read_bytes;
        self.spawn_blocking(move || {
            let path = directory.join("transcript.json");
            check_directory(&vault)?;
            check_directory(&vault.join("sessions"))?;
            check_directory(&directory)?;
            let Some(file) = crate::media::open_regular_file(&path)? else {
                return Ok(Some(Source {
                    snapshot: TranscriptEditorSnapshot {
                        revision: revision(&id, &meta.meta.created_at, false, &[]),
                        session_id: id,
                        session_created_at: meta.meta.created_at,
                        transcripts: vec![],
                    },
                    raw: serde_json::json!({"transcripts": []}),
                    path,
                    file: None,
                }));
            };
            let file = FileSnapshot::new(file)?;
            if file.file().metadata().map_err(io)?.len() > limit as u64 {
                return Err(self::limit());
            }
            let mut bytes = Vec::new();
            file.file()
                .take((limit as u64).saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(io)?;
            if bytes.len() > limit {
                return Err(self::limit());
            }
            let raw: UniqueValue = serde_json::from_slice(&bytes).map_err(json)?;
            let parsed: hypr_fs_format::TranscriptJson =
                serde_json::from_value(raw.0.clone()).map_err(json)?;
            let mut ids = std::collections::HashSet::new();
            if parsed
                .transcripts
                .iter()
                .any(|transcript| transcript.session_id != id || !ids.insert(transcript.id.clone()))
            {
                return Err(StoreError::Serialize(
                    "Transcript identities are invalid".into(),
                ));
            }
            let snapshot = TranscriptEditorSnapshot {
                revision: revision(&id, &meta.meta.created_at, true, &bytes),
                session_id: id,
                session_created_at: meta.meta.created_at,
                transcripts: parsed.transcripts,
            };
            Ok(Some(Source {
                snapshot,
                raw: raw.0,
                path,
                file: Some(file),
            }))
        })
        .await
        .map_err(join)?
    }

    async fn transcript_editor_current(
        &self,
        guard: &crate::WriteGuard<'_>,
        source: &Source,
    ) -> Result<bool, StoreError> {
        self.ensure_ready()?;
        self.ensure_append_ready(&source.snapshot.session_id)?;
        let Some(meta) = self
            .metadata_snapshot_locked(guard, &source.snapshot.session_id)
            .await?
        else {
            return Ok(false);
        };
        if meta.meta.created_at != source.snapshot.session_created_at {
            return Ok(false);
        }
        let directory = self.vault_base.join(
            self.session_dir_locked(guard, &source.snapshot.session_id)
                .await?,
        );
        check_directory(&self.vault_base)?;
        check_directory(&self.vault_base.join("sessions"))?;
        check_directory(&directory)?;
        if directory.join("transcript.json") != source.path {
            return Ok(false);
        }
        match (&source.file, crate::media::open_regular_file(&source.path)?) {
            (None, None) => Ok(true),
            (Some(original), Some(current)) => original.unchanged(&FileSnapshot::new(current)?),
            _ => Ok(false),
        }
    }
}

fn apply(
    raw: &mut Value,
    input: &hypr_loofah_protocol::TranscriptSpeakerAssignment,
) -> Result<(), StoreError> {
    let transcripts = raw
        .get_mut("transcripts")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| StoreError::Serialize("Transcript array required".into()))?;
    let raw = transcripts
        .iter_mut()
        .find(|value| value.get("id").and_then(Value::as_str) == Some(&input.transcript_id))
        .ok_or_else(changed)?;
    let entry: hypr_fs_format::TranscriptWithData =
        serde_json::from_value(raw.clone()).map_err(json)?;
    let mut words = std::collections::HashSet::new();
    if entry
        .words
        .iter()
        .filter_map(|word| word.id.as_deref())
        .any(|id| !words.insert(id))
    {
        return Err(changed());
    }
    let anchors: Vec<_> = entry
        .words
        .iter()
        .filter(|word| word.id.as_deref() == Some(input.anchor_word_id.as_str()))
        .collect();
    if anchors.len() != 1 || anchors[0].channel != f64::from(input.channel) {
        return Err(changed());
    }
    let actual = crate::transcript::provider_speaker_index_for_word(
        &entry.speaker_hints,
        &input.anchor_word_id,
    );
    if actual != input.speaker_index.map(f64::from) {
        return Err(changed());
    }
    let next = crate::transcript::assigned_speaker_hints(
        &entry,
        input.channel,
        input.speaker_index,
        &input.speaker_label,
        &input.anchor_word_id,
    );
    let original = raw.get("speaker_hints").and_then(Value::as_array);
    let mut hints = Vec::with_capacity(next.len());
    let retained = next.len().saturating_sub(1);
    let mut cursor = 0;
    if let Some(original) = original {
        for (value, hint) in original.iter().zip(&entry.speaker_hints) {
            if cursor < retained && *hint == next[cursor] {
                hints.push(value.clone());
                cursor += 1;
            }
        }
    }
    if cursor != retained {
        return Err(changed());
    }
    if let Some(last) = next.last() {
        let replacements = original
            .into_iter()
            .flatten()
            .filter(|value| {
                value.get("id").and_then(Value::as_str) == last.id.as_deref()
                    && value.get("type").and_then(Value::as_str) == Some("speaker_label")
                    && value.get("word_id").and_then(Value::as_str)
                        == Some(input.anchor_word_id.as_str())
            })
            .collect::<Vec<_>>();
        if replacements.len() > 1 {
            return Err(changed());
        }
        let mut replacement = replacements
            .first()
            .map(|value| (**value).clone())
            .unwrap_or_else(|| serde_json::json!({}));
        let canonical = serde_json::to_value(last).map_err(json)?;
        replacement
            .as_object_mut()
            .ok_or_else(changed)?
            .extend(canonical.as_object().ok_or_else(changed)?.clone());
        hints.push(replacement);
    }
    raw.as_object_mut()
        .ok_or_else(changed)?
        .insert("speaker_hints".into(), hints.into());
    Ok(())
}
pub(crate) fn encode(value: &Value, budget: usize) -> Result<Vec<u8>, StoreError> {
    struct Output {
        bytes: Vec<u8>,
        budget: usize,
        exceeded: bool,
    }
    impl std::io::Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.budget.saturating_sub(self.bytes.len()) {
                self.exceeded = true;
                return Err(std::io::Error::other(
                    "Transcript exceeds its response limit",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Output {
        bytes: Vec::new(),
        budget,
        exceeded: false,
    };
    let result = serde_json::to_writer_pretty(&mut output, value);
    if output.exceeded {
        return Err(limit());
    }
    result.map_err(json)?;
    Ok(output.bytes)
}

fn revision(id: &str, created: &str, present: bool, bytes: &[u8]) -> String {
    let mut value =
        serde_json::to_vec(&("loofah/transcript-editor/v1", id, created, present)).unwrap();
    value.extend_from_slice(bytes);
    crate::sha256(&value)
}
fn check_directory(path: &std::path::Path) -> Result<(), StoreError> {
    let metadata = std::fs::symlink_metadata(path).map_err(io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StoreError::Io("Transcript directory is not regular".into()));
    }
    Ok(())
}
fn changed() -> StoreError {
    StoreError::Conflict(
        "The original transcript changed; reload before assigning its speakers".into(),
    )
}
fn busy() -> StoreError {
    StoreError::Conflict(
        "Finish recording or processing before assigning transcript speakers".into(),
    )
}
fn limit() -> StoreError {
    StoreError::ResourceLimit("Transcript exceeds its response limit".into())
}
fn io(cause: impl std::fmt::Display) -> StoreError {
    StoreError::Io(cause.to_string())
}
fn json(cause: impl std::fmt::Display) -> StoreError {
    StoreError::Serialize(cause.to_string())
}
fn join(cause: impl std::fmt::Display) -> StoreError {
    StoreError::Io(format!("Transcript worker stopped: {cause}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypr_loofah_protocol::TranscriptSpeakerAssignment;
    use serde_json::json;

    async fn fixture() -> (tempfile::TempDir, SessionStore, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path().join("vault"));
        let meta = serde_json::from_value(
            json!({"id":"note","title":"Title","created_at":"2026-11-08T10:00:00Z","tags":[]}),
        )
        .unwrap();
        store
            .create_note_session(&meta, "Private note")
            .await
            .unwrap();
        let path = root.path().join("vault/sessions/note/transcript.json");
        std::fs::write(&path, serde_json::to_vec(&json!({
            "future_root":{"keep":[1,true]},
            "transcripts":[{"id":"t","session_id":"note","future_entry":42,
                "words":[{"id":"w","text":"Original words","start_ms":0,"end_ms":100,"channel":0,"future_word":{"keep":true}}],
                "speaker_hints":[{"id":"future","word_id":"w","type":"future_hint","value":{"keep":true},"future_hint_field":43}]
            }]
        })).unwrap()).unwrap();
        (root, store, path)
    }
    fn input(revision: String) -> AssignTranscriptSpeakersRequest {
        AssignTranscriptSpeakersRequest {
            expected_revision: revision,
            assignments: vec![TranscriptSpeakerAssignment {
                transcript_id: "t".into(),
                channel: 0,
                speaker_index: None,
                speaker_label: "alice".into(),
                anchor_word_id: "w".into(),
            }],
        }
    }
    #[tokio::test]
    async fn checked_speaker_edit_preserves_unknown_root_entry_word_and_hint_fields() {
        let (_root, store, path) = fixture().await;
        let before: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let snapshot = store.read_transcript_editor("note").await.unwrap().unwrap();
        let outcome = store
            .assign_transcript_speakers_checked("note", input(snapshot.revision.clone()))
            .await
            .unwrap()
            .unwrap();
        let TranscriptEditorSaveOutcome::Saved(after) = outcome else {
            panic!("not saved");
        };
        assert_ne!(snapshot.revision, after.revision);
        let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["future_root"], before["future_root"]);
        assert_eq!(raw["transcripts"][0]["future_entry"], 42);
        assert_eq!(
            raw["transcripts"][0]["words"],
            before["transcripts"][0]["words"]
        );
        assert_eq!(
            raw["transcripts"][0]["speaker_hints"][0],
            before["transcripts"][0]["speaker_hints"][0]
        );
        assert_eq!(raw["transcripts"][0]["speaker_hints"][1]["value"], "alice");
        assert_eq!(
            store.session_transcript_metadata("note").speaker_labels,
            ["alice"]
        );
    }
    #[tokio::test]
    async fn changed_raw_or_recreated_session_never_adopts_the_new_baseline() {
        let (_root, store, path) = fixture().await;
        let snapshot = store.read_transcript_editor("note").await.unwrap().unwrap();
        let mut raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        raw["future_root"] = json!({"external":true});
        let external = serde_json::to_vec(&raw).unwrap();
        std::fs::write(&path, &external).unwrap();
        assert!(matches!(
            store
                .assign_transcript_speakers_checked("note", input(snapshot.revision.clone()))
                .await
                .unwrap(),
            Some(TranscriptEditorSaveOutcome::Conflict(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), external);
        let fresh = store.read_transcript_editor("note").await.unwrap().unwrap();
        let meta = path.with_file_name("_meta.json");
        let mut value: Value = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
        value["created_at"] = "2026-11-09T10:00:00Z".into();
        std::fs::write(meta, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            store
                .assign_transcript_speakers_checked("note", input(fresh.revision))
                .await
                .unwrap(),
            Some(TranscriptEditorSaveOutcome::Conflict(_))
        ));
        assert_eq!(std::fs::read(path).unwrap(), external);
    }
    #[tokio::test]
    async fn invalid_anchor_channel_or_index_and_atomic_second_assignment_leave_bytes_unchanged() {
        let (_root, store, path) = fixture().await;
        let snapshot = store.read_transcript_editor("note").await.unwrap().unwrap();
        let before = std::fs::read(&path).unwrap();
        for mode in 0..4 {
            let mut input = input(snapshot.revision.clone());
            match mode {
                0 => input.assignments[0].anchor_word_id = "missing".into(),
                1 => input.assignments[0].channel = 1,
                2 => input.assignments[0].speaker_index = Some(0),
                _ => {
                    let mut invalid = input.assignments[0].clone();
                    invalid.transcript_id = "missing".into();
                    input.assignments.push(invalid);
                }
            }
            assert!(
                store
                    .assign_transcript_speakers_checked("note", input)
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }
    #[tokio::test]
    async fn recording_live_and_owned_processing_refuse_without_queueing_or_discarding_data() {
        let (_root, store, path) = fixture().await;
        let snapshot = store.read_transcript_editor("note").await.unwrap().unwrap();
        let before = std::fs::read(&path).unwrap();
        store.note_recording_active("note");
        assert!(
            store
                .assign_transcript_speakers_checked("note", input(snapshot.revision.clone()))
                .await
                .is_err()
        );
        store.active_recordings.lock().unwrap().remove("note");
        let operation = store.transcript_operations.lock("note").await;
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            store.assign_transcript_speakers_checked("note", input(snapshot.revision)),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        drop(operation);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    #[tokio::test]
    async fn absent_transcript_is_readable_and_duplicate_json_is_never_rewritten() {
        let (_root, store, path) = fixture().await;
        std::fs::remove_file(&path).unwrap();
        let absent = store.read_transcript_editor("note").await.unwrap().unwrap();
        assert!(absent.transcripts.is_empty());
        assert!(
            store
                .assign_transcript_speakers_checked("note", input(absent.revision))
                .await
                .is_err()
        );
        assert!(!path.exists());
        let duplicate = br#"{"transcripts":[],"transcripts":[]}"#;
        std::fs::write(&path, duplicate).unwrap();
        assert!(store.read_transcript_editor("note").await.is_err());
        assert_eq!(std::fs::read(path).unwrap(), duplicate);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_transcript_cannot_read_or_replace_an_external_file() {
        let (root, store, path) = fixture().await;
        let external = root.path().join("external");
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&external, &original).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&external, &path).unwrap();
        assert!(store.read_transcript_editor("note").await.is_err());
        assert_eq!(std::fs::read(external).unwrap(), original);
    }
    #[tokio::test]
    async fn canonical_snapshot_does_not_wait_for_an_owned_transcription_operation() {
        let (_root, store, path) = fixture().await;
        let operation = store.transcript_operations.lock("note").await;
        let before = std::fs::read(&path).unwrap();
        let snapshot = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            store.read_transcript_editor("note"),
        )
        .await
        .expect("Canonical reads must not wait for inference")
        .unwrap()
        .unwrap();
        assert_eq!(snapshot.transcripts.len(), 1);
        assert_eq!(std::fs::read(path).unwrap(), before);
        drop(operation);
    }
    #[tokio::test]
    async fn output_budget_and_ambiguous_word_identity_leave_raw_bytes_unchanged() {
        let (_root, mut store, path) = fixture().await;
        let before = std::fs::read(&path).unwrap();
        store.content_limits.transcript_read_bytes = before.len();
        let snapshot = store.read_transcript_editor("note").await.unwrap().unwrap();
        assert!(matches!(
            store
                .assign_transcript_speakers_checked("note", input(snapshot.revision))
                .await,
            Err(StoreError::ResourceLimit(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        store.content_limits.transcript_read_bytes = 64 * 1024 * 1024;
        let mut value: Value = serde_json::from_slice(&before).unwrap();
        let duplicate = value["transcripts"][0]["words"][0].clone();
        value["transcripts"][0]["words"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        let ambiguous = serde_json::to_vec(&value).unwrap();
        std::fs::write(&path, &ambiguous).unwrap();
        let snapshot = store.read_transcript_editor("note").await.unwrap().unwrap();
        assert!(
            store
                .assign_transcript_speakers_checked("note", input(snapshot.revision))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), ambiguous);
    }
    #[tokio::test]
    async fn updating_the_same_speaker_hint_retains_unknown_hint_properties() {
        let (_root, store, path) = fixture().await;
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["transcripts"][0]["speaker_hints"].as_array_mut().unwrap().push(json!({"id":"w:speaker_label","word_id":"w","type":"speaker_label","value":"before","future":{"preserve":true}}));
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let snapshot = store.read_transcript_editor("note").await.unwrap().unwrap();
        store
            .assign_transcript_speakers_checked("note", input(snapshot.revision))
            .await
            .unwrap();
        let after: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(
            after["transcripts"][0]["speaker_hints"][1]["future"],
            json!({"preserve":true})
        );
        assert_eq!(
            after["transcripts"][0]["speaker_hints"][1]["value"],
            "alice"
        );
    }
    #[tokio::test]
    async fn caller_cancellation_keeps_original_owner_until_the_admitted_write_finishes() {
        struct Owner(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for Owner {
            fn drop(&mut self) {
                if let Some(done) = self.0.take() {
                    let _ = done.send(());
                }
            }
        }
        let (_root, store, path) = fixture().await;
        let snapshot = store.read_transcript_editor("note").await.unwrap().unwrap();
        let guard = store.lock_writes().await;
        let (finished, mut done) = tokio::sync::oneshot::channel();
        let caller_store = store.clone();
        let caller = tokio::spawn(async move {
            caller_store
                .assign_transcript_speakers_checked_owned(
                    "note",
                    input(snapshot.revision),
                    Box::new(Owner(Some(finished))),
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if store.transcript_operations.try_lock("note").is_none() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        caller.abort();
        let _ = caller.await;
        assert!(matches!(
            done.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        drop(guard);
        tokio::time::timeout(std::time::Duration::from_secs(2), done)
            .await
            .unwrap()
            .unwrap();
        let raw: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(raw["transcripts"][0]["speaker_hints"][1]["value"], "alice");
    }
}

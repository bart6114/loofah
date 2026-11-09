use super::{io, types::*};
use crate::{SessionStore, StoreError};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) struct Source {
    pub store: SessionStore,
    pub directory: PathBuf,
    pub receipt: SourceReceipt,
    pub recording: File,
    pub transcript: Option<Vec<u8>>,
}

pub(super) fn load(
    store: SessionStore,
    root: PathBuf,
    identity: &AppendIdentity,
    session: Option<&str>,
) -> Result<AppendSource, StoreError> {
    let receipt: SourceReceipt = io::json(&root.join("source.json"))?
        .ok_or_else(|| io::invalid("Original append source requires recovery"))?;
    io::validate_source(&receipt, identity, session)?;
    let recording = io::open(&root.join("original.audio"))?
        .ok_or_else(|| io::invalid("Retained original recording is missing"))?;
    if io::hash(&recording, io::AUDIO_LIMIT)?
        != (receipt.recording_sha256.clone(), receipt.recording_bytes)
    {
        return Err(io::invalid("Retained original recording changed"));
    }
    let transcript = io::read(&root.join("transcript.before.json"), io::TRANSCRIPT_LIMIT)?;
    if transcript.is_some() != receipt.transcript_present
        || io::revision(transcript.as_deref()) != receipt.transcript_revision
    {
        return Err(io::invalid("Retained original transcript changed"));
    }
    Ok(AppendSource(Arc::new(Source {
        store,
        directory: root,
        receipt,
        recording,
        transcript,
    })))
}

impl SessionStore {
    pub async fn prepare_recording_append_source(
        &self,
        operations: &Path,
        identity: AppendIdentity,
        session_id: &str,
    ) -> Result<AppendSource, StoreError> {
        identity.validate()?;
        crate::validate_session_id(session_id)?;
        let root = io::root(operations, &identity)?;
        let id = session_id.to_owned();
        let store = self.clone();
        tokio::spawn(async move {
            let original_store = store.clone();
            let original_root = root.clone();
            let original_identity = identity.clone();
            let original_id = id.clone();
            if let Some(source) = store
                .spawn_blocking(move || match std::fs::symlink_metadata(&original_root) {
                    Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                        Err(io::invalid("Append admission directory is unsafe"))
                    }
                    Ok(_) => load(
                        original_store,
                        original_root,
                        &original_identity,
                        Some(&original_id),
                    )
                    .map(Some),
                    Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(cause) => Err(io::error(cause)),
                })
                .await
                .map_err(io::error)??
            {
                return Ok(source);
            }
            let audio = store.lock_session_audio(&id).await?;
            let transcript = store.transcript_operations.lock(&id).await;
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            store.ensure_append_ready(&id)?;
            let Some(meta) = store
                .metadata_snapshot_locked(&snapshot._guard, &id)
                .await?
            else {
                return Err(io::invalid("Append session does not exist"));
            };
            if store.is_recording(&id)
                || store.live.lock().await.contains_key(&id)
                || meta
                    .meta
                    .extra
                    .get("audio_import_pending")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
            {
                return Err(io::invalid("Append session is busy"));
            }
            let metadata_audio = meta
                .meta
                .extra
                .get("audio")
                .map(|v| serde_json::from_value(v.clone()).map_err(io::error))
                .transpose()?
                .unwrap_or_default();
            let directory = store
                .vault_base
                .join(store.session_dir_locked(&snapshot._guard, &id).await?);
            let worker = store.clone();
            store
                .spawn_blocking(move || {
                    let _audio = audio;
                    let _transcript = transcript;
                    // An admission racing this one must recover its exact original bytes.
                    if root.exists() {
                        drop(snapshot);
                        return load(worker, root, &identity, Some(&id));
                    }
                    let (selection, recording) = io::select(&directory)?
                        .ok_or_else(|| io::invalid("Append requires an existing recording"))?;
                    let raw_transcript =
                        io::read(&directory.join("transcript.json"), io::TRANSCRIPT_LIMIT)?;
                    drop(snapshot);
                    if !recording.lock_for_recheck()? {
                        return Err(io::invalid("Append recording is busy"));
                    }
                    let (sha, length) = io::hash(recording.file(), io::AUDIO_LIMIT)?;
                    if length == 0 {
                        return Err(io::invalid("Append recording is empty"));
                    }
                    io::private_directory(&root)?;
                    io::copy(&root.join("original.audio"), recording.file(), &sha, length)?;
                    if let Some(bytes) = &raw_transcript {
                        io::retain_bytes(&root.join("transcript.before.json"), bytes)?;
                    }
                    let current = io::select(&directory)?
                        .ok_or_else(|| io::invalid("Append recording disappeared"))?;
                    if current.0 != selection
                        || !recording.unchanged(&current.1)?
                        || io::read(&directory.join("transcript.json"), io::TRANSCRIPT_LIMIT)?
                            != raw_transcript
                    {
                        return Err(io::invalid("Append source changed during retention"));
                    }
                    let receipt = SourceReceipt {
                        schema_version: 1,
                        identity: identity.clone(),
                        session_id: id.clone(),
                        selection,
                        recording_sha256: sha,
                        recording_bytes: length,
                        transcript_revision: io::revision(raw_transcript.as_deref()),
                        transcript_present: raw_transcript.is_some(),
                        audio: metadata_audio,
                    };
                    io::persist(&root.join("source.json"), &receipt)?;
                    load(worker, root, &identity, Some(&id))
                })
                .await
                .map_err(io::error)?
        })
        .await
        .map_err(io::error)?
    }
}

pub(super) async fn recheck(source: AppendSource) -> Result<bool, StoreError> {
    let store = source.0.store.clone();
    let id = source.0.receipt.session_id.clone();
    tokio::spawn(async move {
        if store.ensure_append_ready(&id).is_err() {
            return Ok(false);
        }
        let _audio = store.lock_session_audio(&id).await?;
        let _transcript = store.transcript_operations.lock(&id).await;
        let snapshot = store.lock_snapshot().await;
        store.ensure_ready()?;
        let Some(meta) = store
            .metadata_snapshot_locked(&snapshot._guard, &id)
            .await?
        else {
            return Ok(false);
        };
        if store.is_recording(&id)
            || store.live.lock().await.contains_key(&id)
            || meta
                .meta
                .extra
                .get("audio_import_pending")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        {
            return Ok(false);
        }
        let audio = meta
            .meta
            .extra
            .get("audio")
            .map(|v| serde_json::from_value(v.clone()).map_err(io::error))
            .transpose()?
            .unwrap_or_default();
        if source.0.receipt.audio != audio {
            return Ok(false);
        }
        let directory = store
            .vault_base
            .join(store.session_dir_locked(&snapshot._guard, &id).await?);
        store
            .spawn_blocking(move || {
                drop(snapshot);
                let _audio = _audio;
                let _transcript = _transcript;
                matches_current(&directory, &source.0.receipt)
            })
            .await
            .map_err(io::error)?
    })
    .await
    .map_err(io::error)?
}

pub(super) fn matches_current(
    directory: &Path,
    receipt: &SourceReceipt,
) -> Result<bool, StoreError> {
    let Some((selection, recording)) = io::select(directory)? else {
        return Ok(false);
    };
    if selection != receipt.selection
        || !recording.lock_for_recheck()?
        || io::hash(recording.file(), io::AUDIO_LIMIT)?
            != (receipt.recording_sha256.clone(), receipt.recording_bytes)
    {
        return Ok(false);
    }
    let current = io::select(directory)?.ok_or_else(|| io::invalid("Append source disappeared"))?;
    if current.0 != selection || !recording.unchanged(&current.1)? {
        return Ok(false);
    }
    let transcript = io::read(&directory.join("transcript.json"), io::TRANSCRIPT_LIMIT)?;
    Ok(transcript.is_some() == receipt.transcript_present
        && io::revision(transcript.as_deref()) == receipt.transcript_revision)
}

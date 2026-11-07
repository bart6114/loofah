use std::{io::Read, path::Path};

use hypr_fs_format::{TranscriptJson, TranscriptWithData};

use crate::{PersonItem, SessionMeta, SessionStore, StoreError, TagContext};

#[derive(Clone, Copy)]
pub struct SourceReadLimits {
    pub note_bytes: usize,
    pub metadata_bytes: usize,
    pub transcript_bytes: usize,
    pub registry_bytes: usize,
}

impl Default for SourceReadLimits {
    fn default() -> Self {
        Self {
            note_bytes: 16 * 1024 * 1024,
            metadata_bytes: 8 * 1024 * 1024,
            transcript_bytes: 32 * 1024 * 1024,
            registry_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub struct ProcessingSource {
    pub meta: SessionMeta,
    pub note_markdown: Option<String>,
    pub transcripts: Vec<TranscriptWithData>,
    pub people: Vec<PersonItem>,
    pub tag_context: TagContext,
}

impl SessionStore {
    pub async fn read_processing_source(
        &self,
        operation_root: Option<&Path>,
        session_id: &str,
        limits: SourceReadLimits,
    ) -> Result<Option<ProcessingSource>, StoreError> {
        self.read_processing_source_with(operation_root, session_id, limits, || async {})
            .await
    }

    async fn read_processing_source_with<C, F>(
        &self,
        operation_root: Option<&Path>,
        session_id: &str,
        limits: SourceReadLimits,
        checkpoint: C,
    ) -> Result<Option<ProcessingSource>, StoreError>
    where
        C: FnOnce() -> F + Send + 'static,
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        // Model input budgets do not change the trusted local editor's content limits.
        let mut bounded = self.clone();
        bounded.content_limits.note_read_bytes = bounded
            .content_limits
            .note_read_bytes
            .min(limits.note_bytes);
        bounded.content_limits.metadata_bytes = bounded
            .content_limits
            .metadata_bytes
            .min(limits.metadata_bytes);
        bounded
            .recovered_read(
                operation_root,
                session_id,
                move |store, snapshot, id| async move {
                    store.ensure_ready()?;
                    let Some(metadata) = store
                        .metadata_snapshot_locked(&snapshot._guard, &id)
                        .await?
                    else {
                        return Ok(None);
                    };
                    checkpoint().await;
                    let note = store
                        .read_note_snapshot_locked(&snapshot._guard, &id)
                        .await?
                        .ok_or_else(|| {
                            StoreError::Io("Processing source disappeared during snapshot".into())
                        })?;
                    let vault = store.vault_base.clone();
                    let directory = store.session_dir_locked(&snapshot._guard, &id).await?;
                    let meta = metadata.meta;
                    tokio::task::spawn_blocking(move || {
                        let raw = bounded_file(
                            &vault.join(crate::paths::transcript_path_in(&directory)),
                            limits.transcript_bytes,
                        )?;
                        let people = bounded_file(
                            &vault.join(crate::paths::people_path()),
                            limits.registry_bytes,
                        )?;
                        let tags = bounded_file(
                            &vault.join(crate::paths::tags_path()),
                            limits.registry_bytes,
                        )?;
                        // Decoding a large transcript must not delay interactive writes or recording flushes.
                        drop(snapshot);
                        let mut transcripts = match raw {
                            Some(raw) => {
                                serde_json::from_slice::<TranscriptJson>(&raw)
                                    .map_err(|_| {
                                        StoreError::Serialize(
                                            "Cannot parse processing transcript".into(),
                                        )
                                    })?
                                    .transcripts
                            }
                            None => Vec::new(),
                        };
                        crate::index::sort_transcripts(&mut transcripts);
                        let mut people = people
                            .and_then(|raw| {
                                serde_json::from_slice::<crate::people::PeopleFile>(&raw).ok()
                            })
                            .map(|file| file.people)
                            .unwrap_or_default();
                        people.sort_by(|a, b| {
                            a.name
                                .to_lowercase()
                                .cmp(&b.name.to_lowercase())
                                .then_with(|| a.id.cmp(&b.id))
                        });
                        let tags = tags
                            .and_then(|raw| {
                                serde_json::from_slice::<crate::tags::TagsFile>(&raw).ok()
                            })
                            .map(|file| file.tags)
                            .unwrap_or_default();
                        let tag_context = store.tag_context_from_meta(&meta, tags);
                        Ok(Some(ProcessingSource {
                            meta,
                            note_markdown: note.markdown,
                            transcripts,
                            people,
                            tag_context,
                        }))
                    })
                    .await
                    .map_err(|_| StoreError::Io("Processing source reader stopped".into()))?
                },
            )
            .await
    }
}

fn bounded_file(path: &Path, limit: usize) -> Result<Option<Vec<u8>>, StoreError> {
    let Some(file) = crate::media::open_regular_file(path)? else {
        return Ok(None);
    };
    if file
        .metadata()
        .map_err(|cause| StoreError::Io(cause.to_string()))?
        .len()
        > limit as u64
    {
        return Err(StoreError::ResourceLimit(
            "Processing source exceeds its size limit".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|cause| StoreError::Io(cause.to_string()))?;
    if bytes.len() > limit {
        return Err(StoreError::ResourceLimit(
            "Processing source exceeds its size limit".into(),
        ));
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests;

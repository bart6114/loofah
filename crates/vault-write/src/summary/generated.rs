use hypr_loofah_protocol::{DocumentSaveOutcome, DocumentTarget};

use crate::{
    ScoredTagSuggestion, SessionStore, StoreError,
    document_operation::{GeneratedSummaryWrite, GeneratedTagMetadata},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommitStage {
    Intent,
    Body,
    Tasks,
    Metadata,
    Registry,
}

impl SessionStore {
    pub async fn save_generated_summary_with_suggestions_if_revision(
        &self,
        operation_root: &std::path::Path,
        session_id: &str,
        markdown: &str,
        expected_revision: &str,
        suggestions: Option<Vec<ScoredTagSuggestion>>,
        auto_apply_high_confidence_tags: bool,
    ) -> Result<DocumentSaveOutcome, StoreError> {
        self.save_generated_summary_with_checkpoint(
            operation_root,
            session_id,
            (markdown, expected_revision),
            suggestions,
            auto_apply_high_confidence_tags,
            |_| async { Ok(()) },
        )
        .await
    }

    async fn save_generated_summary_with_checkpoint<C, F>(
        &self,
        operation_root: &std::path::Path,
        session_id: &str,
        replacement: (&str, &str),
        suggestions: Option<Vec<ScoredTagSuggestion>>,
        auto_apply: bool,
        mut checkpoint: C,
    ) -> Result<DocumentSaveOutcome, StoreError>
    where
        C: FnMut(CommitStage) -> F + Send + 'static,
        F: std::future::Future<Output = Result<(), StoreError>> + Send + 'static,
    {
        let (markdown, expected_revision) = replacement;
        crate::validate_session_id(session_id)?;
        if expected_revision.is_empty() || expected_revision.len() > 128 {
            return Err(StoreError::Serialize(
                "Expected summary revision is required".into(),
            ));
        }
        if markdown.len() > self.content_limits.note_write_bytes {
            return Err(StoreError::ResourceLimit(
                "Generated summary is too large".into(),
            ));
        }
        if suggestions.as_ref().is_some_and(|items| {
            items.len() > 3
                || items.iter().any(|item| {
                    item.name.trim().is_empty()
                        || item.name.encode_utf16().count() > 120
                        || !item.confidence.is_finite()
                        || !(0.0..=1.0).contains(&item.confidence)
                })
        }) {
            return Err(StoreError::Serialize(
                "Invalid generated tag suggestions".into(),
            ));
        }
        let markdown = markdown.to_owned();
        let parse = markdown.clone();
        let content = tokio::task::spawn_blocking(move || hypr_tiptap::md_to_tiptap_json(&parse))
            .await
            .map_err(|error| StoreError::Io(error.to_string()))?
            .map_err(StoreError::Serialize)?;
        let expected_revision = expected_revision.to_owned();
        let root = operation_root.to_path_buf();
        self.recovered_read(
            Some(operation_root),
            session_id,
            move |store, snapshot, id| async move {
                let mut registry_ids = Vec::new();
                let result = async {
                    let guard = &snapshot._guard;
                    store.ensure_ready()?;
                    if store.is_recording(&id) {
                        return Err(StoreError::Conflict(
                            "recording is active; generated summary cannot be applied".into(),
                        ));
                    }
                    let target = DocumentTarget::Summary;
                    let current = store
                        .document_snapshot_locked(guard, &id, &target)
                        .await?
                        .ok_or_else(|| StoreError::Conflict("session was deleted".into()))?;
                    if current.revision != expected_revision {
                        return Ok(DocumentSaveOutcome::Conflict(current));
                    }
                    let directory = store.writable_summary_dir(guard, &id).await?;
                    store
                        .try_migrate_summary_locked(guard, &id, &directory)
                        .await?;
                    let baseline = store
                        .document_snapshot_locked(guard, &id, &target)
                        .await?
                        .ok_or_else(|| StoreError::Conflict("session was deleted".into()))?;
                    let before_tasks = store.document_tasks_hash(&id).await?;
                    let mut meta = store
                        .metadata_snapshot_locked(guard, &id)
                        .await?
                        .ok_or_else(|| StoreError::Conflict("session was deleted".into()))?
                        .meta;
                    let before = GeneratedTagMetadata::read(&meta);
                    let registered = suggestions
                        .map(|items| {
                            crate::tags::apply_suggested_tags(&mut meta, items, auto_apply)
                        })
                        .unwrap_or_default();
                    registry_ids = registered.clone();
                    let after = GeneratedTagMetadata::read(&meta);
                    crate::metadata_revision::validate_metadata_size(
                        &meta,
                        store.content_limits.metadata_bytes,
                    )?;
                    store
                        .validate_generated_tag_registration(&registered)
                        .await?;
                    let tasks = store
                        .prepare_generated_tasks(&id, "session_summary", &id, &content)
                        .await?;
                    let vault = store.vault_base.clone();
                    let read_directory = directory.clone();
                    let read_id = id.clone();
                    let located = tokio::task::spawn_blocking(move || {
                        hypr_vault_read::summary::locate_in(&vault, &read_directory, &read_id)
                    })
                    .await
                    .map_err(|error| StoreError::Io(error.to_string()))??;
                    let (relative, bytes) = match located {
                        Some(located) => {
                            let bytes = if let Some(legacy) = located.legacy_id {
                                let mut document = store
                                    .read_enhanced_doc(&id, &legacy)
                                    .await?
                                    .ok_or_else(|| {
                                        StoreError::Conflict("legacy summary was deleted".into())
                                    })?;
                                document.markdown = markdown.clone();
                                hypr_vault_read::render_enhanced_file(&document)?.into_bytes()
                            } else {
                                markdown.as_bytes().to_vec()
                            };
                            (located.relative_path, bytes)
                        }
                        None => (
                            crate::paths::summary_path_in(&directory),
                            markdown.as_bytes().to_vec(),
                        ),
                    };
                    let pending = store
                        .begin_generated_summary_write_locked(
                            guard,
                            &root,
                            &id,
                            GeneratedSummaryWrite {
                                before_revision: &baseline.revision,
                                before_tasks_hash: &before_tasks,
                                after_bytes: &bytes,
                                tasks: &tasks,
                                before_metadata: before.clone(),
                                after_metadata: after.clone(),
                                register_tags: registered.clone(),
                            },
                        )
                        .await?;
                    checkpoint(CommitStage::Intent).await?;
                    store.write_file_locked(guard, relative, bytes).await?;
                    store.index_set_summary(&id, Some(markdown.clone()));
                    checkpoint(CommitStage::Body).await?;
                    store
                        .persist_generated_tasks(guard, &id, &tasks)
                        .await
                        .map_err(StoreError::after_commit)?;
                    checkpoint(CommitStage::Tasks).await?;
                    meta = store
                        .finish_generated_tag_metadata_locked(guard, &id, &before, &after)
                        .await
                        .map_err(StoreError::after_commit)?;
                    checkpoint(CommitStage::Metadata).await?;
                    store
                        .register_generated_tags_locked(guard, &registered)
                        .await
                        .map_err(StoreError::after_commit)?;
                    checkpoint(CommitStage::Registry).await?;
                    store.index_set_summary_and_meta(&meta, markdown);
                    store.sync_generated_summary_parents(&id).await?;
                    pending.finish().await?;
                    let actual = store
                        .document_snapshot_locked(guard, &id, &target)
                        .await
                        .map_err(StoreError::after_commit)?
                        .ok_or_else(|| {
                            StoreError::Io(
                                "session disappeared after generated summary save".into(),
                            )
                        })?;
                    Ok(DocumentSaveOutcome::Saved(actual))
                }
                .await;
                if result.is_err() {
                    for entity in [
                        crate::IndexEntity::Sessions,
                        crate::IndexEntity::Docs,
                        crate::IndexEntity::Tasks,
                    ] {
                        store.notify_index_changed(entity, vec![id.clone()]);
                    }
                    if !registry_ids.is_empty() {
                        store.notify_index_changed(crate::IndexEntity::Tags, registry_ids);
                    }
                }
                result
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests;

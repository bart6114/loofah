use std::{io::Read, path::Path};

use hypr_loofah_protocol::{DocumentSaveOutcome, DocumentTarget, SaveDocumentRequest};

use crate::{
    ScoredTagSuggestion, SessionStore, StoreError,
    document_operation::{GeneratedSummaryWrite, GeneratedTagMetadata},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommitStage {
    Prepared,
    Intent,
    Body,
    Tasks,
    Metadata,
    Registry,
}

impl SessionStore {
    pub async fn save_generated_template_with_suggestions_if_revision(
        &self,
        operation_root: &Path,
        session_id: &str,
        document_id: &str,
        input: SaveDocumentRequest,
        suggestions: Option<Vec<ScoredTagSuggestion>>,
        auto_apply_high_confidence_tags: bool,
    ) -> Result<DocumentSaveOutcome, StoreError> {
        self.save_generated_template_with_checkpoint(
            operation_root,
            (session_id, document_id),
            input,
            suggestions,
            auto_apply_high_confidence_tags,
            |_| async { Ok(()) },
        )
        .await
    }

    async fn save_generated_template_with_checkpoint<C, F>(
        &self,
        operation_root: &Path,
        coordinates: (&str, &str),
        input: SaveDocumentRequest,
        suggestions: Option<Vec<ScoredTagSuggestion>>,
        auto_apply: bool,
        mut checkpoint: C,
    ) -> Result<DocumentSaveOutcome, StoreError>
    where
        C: FnMut(CommitStage) -> F + Send + 'static,
        F: std::future::Future<Output = Result<(), StoreError>> + Send + 'static,
    {
        let (session_id, document_id) = coordinates;
        let SaveDocumentRequest {
            markdown,
            expected_revision,
        } = input;
        crate::validate_session_id(session_id)?;
        crate::validate_doc_id(document_id)?;
        if expected_revision.is_empty() || expected_revision.len() > 128 {
            return Err(StoreError::Serialize(
                "Expected template revision is required".into(),
            ));
        }
        if markdown.len() > self.content_limits.note_write_bytes {
            return Err(StoreError::ResourceLimit(
                "Generated template output is too large".into(),
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
        let (prepared_title, markdown, content) = self
            .prepare_generated_markdown(operation_root, session_id, &markdown)
            .await?;
        checkpoint(CommitStage::Prepared).await?;
        let document_id = document_id.to_owned();
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
                            "recording is active; generated template cannot be applied".into(),
                        ));
                    }
                    let target = DocumentTarget::TemplateOutput {
                        document_id: document_id.clone(),
                    };
                    let current = store
                        .document_snapshot_locked(guard, &id, &target)
                        .await?
                        .ok_or_else(|| StoreError::Conflict("session was deleted".into()))?;
                    if current.revision != expected_revision {
                        return Ok(DocumentSaveOutcome::Conflict(current));
                    }
                    let mut meta = store
                        .metadata_snapshot_locked(guard, &id)
                        .await?
                        .ok_or_else(|| StoreError::Conflict("session was deleted".into()))?
                        .meta;
                    if meta.title != prepared_title {
                        return Ok(DocumentSaveOutcome::Conflict(current));
                    }
                    let mut document = current.document.ok_or_else(|| {
                        StoreError::Conflict("template output was deleted".into())
                    })?;
                    let directory = store.session_dir_locked(guard, &id).await?;
                    let relative = crate::paths::enhanced_doc_path_in(&directory, &document_id);
                    let raw_path = store.vault_base.join(&relative);
                    let limit = store.content_limits.note_read_bytes;
                    let expected = current.revision.clone();
                    let preparation_target = target.clone();
                    let preparation_id = id.clone();
                    let original = document.clone();
                    let replacement = markdown.clone();
                    let bytes = store
                        .spawn_blocking(move || {
                            let raw = read_template(&raw_path, limit)?;
                            if crate::document_revision::revision(
                                &preparation_id,
                                &preparation_target,
                                Some(raw.as_bytes()),
                            )? != expected
                            {
                                return Err(StoreError::Conflict(
                                    "template output changed during preparation".into(),
                                ));
                            }
                            Ok(replace_body(&raw, &original, &replacement)?.into_bytes())
                        })
                        .await
                        .map_err(|cause| StoreError::Io(cause.to_string()))??;
                    let before_tasks = store.document_tasks_hash(&id).await?;
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
                        .prepare_generated_tasks(&id, "enhanced_note", &document_id, &content)
                        .await?;
                    let pending = store
                        .begin_generated_document_write_locked(
                            guard,
                            &root,
                            &id,
                            &target,
                            GeneratedSummaryWrite {
                                before_revision: &current.revision,
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
                    document.markdown = markdown;
                    store.index_upsert_doc(&document);
                    store.notify_index_changed(crate::IndexEntity::Docs, vec![id.clone()]);
                    checkpoint(CommitStage::Body).await?;
                    store
                        .persist_generated_tasks(guard, &id, &tasks)
                        .await
                        .map_err(StoreError::after_commit)?;
                    checkpoint(CommitStage::Tasks).await?;
                    store
                        .finish_generated_tag_metadata_locked(guard, &id, &before, &after)
                        .await
                        .map_err(StoreError::after_commit)?;
                    checkpoint(CommitStage::Metadata).await?;
                    store
                        .register_generated_tags_locked(guard, &registered)
                        .await
                        .map_err(StoreError::after_commit)?;
                    checkpoint(CommitStage::Registry).await?;
                    store.sync_generated_document_parents(&id, &target).await?;
                    pending.finish().await?;
                    let actual = store
                        .document_snapshot_locked(guard, &id, &target)
                        .await
                        .map_err(StoreError::after_commit)?
                        .ok_or_else(|| {
                            StoreError::Io(
                                "session disappeared after generated template save".into(),
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

fn read_template(path: &Path, limit: usize) -> Result<String, StoreError> {
    let file = crate::media::open_regular_file(path)?
        .ok_or_else(|| StoreError::Conflict("template output was deleted".into()))?;
    if file
        .metadata()
        .map_err(|cause| StoreError::Io(cause.to_string()))?
        .len()
        > limit as u64
    {
        return Err(StoreError::ResourceLimit(
            "Template output exceeds inspection limit".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|cause| StoreError::Io(cause.to_string()))?;
    if bytes.len() > limit {
        return Err(StoreError::ResourceLimit(
            "Template output exceeds inspection limit".into(),
        ));
    }
    String::from_utf8(bytes).map_err(|cause| StoreError::Serialize(cause.to_string()))
}

pub(super) fn replace_body(
    raw: &str,
    document: &crate::EnhancedDoc,
    markdown: &str,
) -> Result<String, StoreError> {
    // The shared parser returns an unchanged suffix; keep all preceding YAML bytes/comments.
    let prefix = raw.strip_suffix(&document.markdown).ok_or_else(|| {
        StoreError::Conflict("Template body no longer matches its snapshot".into())
    })?;
    for separator in ["", "\n", "\n\n"] {
        let candidate = format!("{prefix}{separator}{markdown}");
        let parsed =
            hypr_vault_read::parse_enhanced_file(&document.id, &document.session_id, &candidate)?;
        let mut expected = document.clone();
        expected.markdown = markdown.into();
        if parsed == expected {
            return Ok(candidate);
        }
    }
    Err(StoreError::Serialize(
        "Generated body cannot preserve template frontmatter".into(),
    ))
}

#[cfg(test)]
mod tests;

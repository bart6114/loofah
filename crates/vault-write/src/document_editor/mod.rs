//! Coherent manual document/title/task saves using the existing document recovery journal.
//! Schema 5 retains raw metadata/task fields; older binaries must recover with a compatible
//! reader before downgrade. Legacy summaries are edited in place without read-time migration.

mod plan;
pub(super) mod recovery;
#[cfg(test)]
mod tests;

use crate::{SessionStore, StoreError, VaultSnapshotGuard, WriteGuard};
use hypr_loofah_protocol::{
    DocumentEditorSnapshot, DocumentSnapshot, DocumentTarget, MetadataSnapshot,
    SaveDocumentEditorRequest, TaskInput, TasksSnapshot,
};
use plan::State;
use std::path::{Path, PathBuf};

pub enum DocumentEditorPreparation {
    Prepared(Box<DocumentEditorPlan>),
    Conflict(Box<DocumentEditorSnapshot>),
}

#[must_use]
pub struct DocumentEditorPlan {
    store: SessionStore,
    guard: VaultSnapshotGuard,
    root: PathBuf,
    record: recovery::Intent,
    body: String,
    changed: bool,
}

impl SessionStore {
    pub async fn read_document_editor(
        &self,
        operation_root: Option<&Path>,
        id: &str,
        target: &DocumentTarget,
    ) -> Result<Option<DocumentEditorSnapshot>, StoreError> {
        let target = target.clone();
        self.recovered_read(operation_root, id, move |store, snapshot, id| async move {
            store
                .editor_state_locked(&snapshot._guard, &id, &target)
                .await
                .map(|state| state.map(|state| state.snapshot))
        })
        .await
    }

    pub async fn prepare_document_editor(
        &self,
        operation_root: &Path,
        id: &str,
        target: &DocumentTarget,
        input: SaveDocumentEditorRequest,
    ) -> Result<Option<DocumentEditorPreparation>, StoreError> {
        crate::validate_session_id(id)?;
        if let DocumentTarget::TemplateOutput { document_id } = target {
            crate::validate_doc_id(document_id)?;
        }
        let store = self.clone();
        let root = operation_root.to_owned();
        let id = id.to_owned();
        let target = target.clone();
        tokio::spawn(async move {
            validate_input(&input, store.content_limits)?;
            let guard = store.lock_recovered_snapshot(Some(&root), &id).await?;
            store.ensure_ready()?;
            let Some(current) = store
                .editor_state_locked(&guard._guard, &id, &target)
                .await?
            else {
                return Ok(None);
            };
            if current.snapshot.document.revision != input.expected_revision
                || current.snapshot.metadata.revision != input.expected_metadata_revision
                || current.snapshot.tasks.revision != input.expected_tasks_revision
            {
                return Ok(Some(DocumentEditorPreparation::Conflict(Box::new(
                    current.snapshot,
                ))));
            }
            let body = match (
                &current.content.location,
                &current.content.raw,
                &current.content.document,
            ) {
                (plan::Location::Summary {}, _, _) => input.markdown,
                (_, Some(raw), Some(document)) => {
                    crate::enhanced::replace_body_preserving_frontmatter(
                        raw,
                        document,
                        &input.markdown,
                    )?
                }
                _ => return Err(StoreError::Conflict("Document disappeared".into())),
            };
            if body.len() > store.content_limits.note_write_bytes {
                return Err(plan::limit("Prospective document exceeds its size limit"));
            }
            let metadata = if input
                .title
                .as_ref()
                .is_some_and(|title| *title != current.snapshot.metadata.meta.title)
            {
                let bytes = crate::metadata_revision::serialize_title_patch(
                    current.metadata_raw.clone(),
                    input.title.as_deref().unwrap(),
                    store.content_limits.metadata_bytes,
                )?;
                Some(plan::unique(&bytes)?)
            } else {
                None
            };
            let tasks = plan::plan_tasks(&current, &id, input.tasks)?;
            let metadata_after = metadata
                .as_ref()
                .map(|value| plan::encode(value, store.content_limits.metadata_bytes))
                .transpose()?;
            let tasks_after = tasks
                .as_ref()
                .map(|value| plan::encode(value, store.content_limits.tasks_file_bytes))
                .transpose()?;
            let after_document =
                crate::document_revision::revision(&id, &target, Some(body.as_bytes()))?;
            let after_metadata = metadata_after.as_ref().map_or_else(
                || current.snapshot.metadata.revision.clone(),
                |raw| crate::metadata_revision::revision(&id, raw),
            );
            let after_tasks = tasks_after.as_ref().map_or_else(
                || current.snapshot.tasks.revision.clone(),
                |raw| crate::tasks_revision::revision(&id, Some(raw)),
            );
            let changed = after_document != current.snapshot.document.revision
                || after_metadata != current.snapshot.metadata.revision
                || after_tasks != current.snapshot.tasks.revision;
            let record = recovery::Intent {
                schema_version: 5,
                session_id: id,
                target,
                location: current.content.location,
                before_document: current.snapshot.document.revision,
                after_document,
                before_metadata: current.snapshot.metadata.revision,
                after_metadata,
                metadata,
                before_tasks: current.snapshot.tasks.revision,
                after_tasks,
                tasks,
            };
            recovery::validate(&record, store.content_limits)?;
            Ok(Some(DocumentEditorPreparation::Prepared(Box::new(
                DocumentEditorPlan {
                    store,
                    guard,
                    root,
                    record,
                    body,
                    changed,
                },
            ))))
        })
        .await
        .map_err(|error| {
            StoreError::Io(format!(
                "Document editor preparation worker failed: {error}"
            ))
        })?
    }

    async fn editor_state_locked(
        &self,
        guard: &WriteGuard<'_>,
        id: &str,
        target: &DocumentTarget,
    ) -> Result<Option<State>, StoreError> {
        let Some((metadata, raw)) = self.metadata_with_raw_locked(guard, id).await? else {
            return Ok(None);
        };
        let vault = self.vault_base.clone();
        let session = id.to_owned();
        let selected = target.clone();
        let limits = self.content_limits;
        let content = tokio::task::spawn_blocking(move || {
            plan::read_content(&vault, &session, &selected, limits)
        })
        .await
        .map_err(|error| StoreError::Io(format!("Editor snapshot worker failed: {error}")))??;
        content
            .map(|content| State::new(id, target, metadata, raw, content))
            .transpose()
    }

    fn index_editor(&self, id: &str, state: &State) {
        self.index_upsert_meta(&state.snapshot.metadata.meta);
        match &state.content.location {
            plan::Location::Summary {} | plan::Location::LegacySummary { .. } => self
                .index_set_summary(
                    id,
                    state
                        .snapshot
                        .document
                        .document
                        .as_ref()
                        .map(|doc| doc.markdown.clone()),
                ),
            plan::Location::TemplateOutput { .. } => {
                if let Some(document) = &state.content.document {
                    self.index_upsert_doc(document);
                }
            }
        }
        let mut tasks = state.content.tasks.clone();
        if let plan::Location::LegacySummary { document_id } = &state.content.location {
            for task in &mut tasks {
                if task.source_type == "enhanced_note" && task.source_id == *document_id {
                    task.source_type = "session_summary".into();
                    task.source_id = id.into();
                }
            }
        }
        self.index_set_tasks(id, tasks);
        for entity in [
            crate::IndexEntity::Sessions,
            crate::IndexEntity::Docs,
            crate::IndexEntity::Tasks,
        ] {
            self.notify_index_changed(entity, vec![id.into()]);
        }
    }
}

impl DocumentEditorPlan {
    pub async fn commit(self) -> Result<DocumentEditorSnapshot, StoreError> {
        tokio::spawn(async move {
            let Self {
                store,
                guard,
                root,
                record,
                body,
                changed,
            } = self;
            let current = store
                .editor_state_locked(&guard._guard, &record.session_id, &record.target)
                .await?
                .ok_or_else(|| {
                    StoreError::Conflict("Editor target disappeared before commit".into())
                })?;
            if !recovery::before(&record, &current) || current.content.location != record.location {
                return Err(StoreError::Conflict(
                    "Editor inputs changed before commit".into(),
                ));
            }
            if !changed {
                return Ok(current.snapshot);
            }
            let pending = store
                .begin_editor_write_locked(&guard._guard, &root, record.clone())
                .await
                .map_err(StoreError::after_commit)?;
            #[cfg(test)]
            tests::checkpoint("intent");
            let result = async {
                store
                    .write_file_locked(
                        &guard._guard,
                        record.location.path(&record.session_id)?,
                        body.into_bytes(),
                    )
                    .await?;
                #[cfg(test)]
                tests::checkpoint("document");
                recovery::finish(&store, &guard._guard, pending.path(), &record).await
            }
            .await;
            if result.is_err() {
                for entity in [
                    crate::IndexEntity::Sessions,
                    crate::IndexEntity::Docs,
                    crate::IndexEntity::Tasks,
                ] {
                    store.notify_index_changed(entity, vec![record.session_id.clone()]);
                }
            }
            result.map_err(StoreError::after_commit)
        })
        .await
        .map_err(|error| StoreError::Io(format!("Document editor commit worker failed: {error}")))?
    }
}

fn validate_input(
    input: &SaveDocumentEditorRequest,
    limits: crate::ContentLimits,
) -> Result<(), StoreError> {
    if [
        &input.expected_revision,
        &input.expected_metadata_revision,
        &input.expected_tasks_revision,
    ]
    .iter()
    .any(|revision| revision.is_empty() || revision.len() > 128)
    {
        return Err(plan::invalid("Expected editor revisions are required"));
    }
    if input.markdown.len() > limits.note_write_bytes
        || input
            .title
            .as_ref()
            .is_some_and(|title| title.len() > limits.title_bytes)
    {
        return Err(plan::limit("Editor document/title exceeds its limit"));
    }
    if input.tasks.len() > limits.task_count {
        return Err(plan::limit("Editor has too many tasks"));
    }
    let mut ids = std::collections::HashSet::new();
    for task in &input.tasks {
        if task.id.is_empty() || task.status.trim().is_empty() || !ids.insert(&task.id) {
            return Err(plan::invalid(
                "Task identifiers must be unique and nonempty",
            ));
        }
        if task.id.len() > limits.task_id_bytes
            || task.status.len() > limits.task_status_bytes
            || task.text.len() > limits.task_text_bytes
            || task.due_at.len() > limits.task_due_bytes
        {
            return Err(plan::limit("Editor task fields exceed their limits"));
        }
    }
    bounded_serialization(&input.tasks, limits.task_request_bytes)?;
    bounded_serialization(input, limits.editor_request_bytes)
}

pub(super) fn bounded_serialization(
    value: &impl serde::Serialize,
    maximum: usize,
) -> Result<(), StoreError> {
    if maximum == usize::MAX {
        return Ok(());
    }
    struct Budget(usize);
    impl std::io::Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(std::io::Error::other("editor budget exceeded"));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Budget(maximum), value)
        .map_err(|_| plan::limit("Editor serialized data exceeds its limit"))
}

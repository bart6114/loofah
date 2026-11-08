use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    pub schema_version: u32,
    pub session_id: String,
    #[serde(deserialize_with = "strict_target")]
    pub target: DocumentTarget,
    pub location: plan::Location,
    pub before_document: String,
    pub after_document: String,
    pub before_metadata: String,
    pub after_metadata: String,
    pub metadata: Option<Value>,
    pub before_tasks: String,
    pub after_tasks: String,
    pub tasks: Option<Value>,
}

fn strict_target<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<DocumentTarget, D::Error> {
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum Target {
        Summary {},
        TemplateOutput { document_id: String },
    }
    Ok(match Target::deserialize(deserializer)? {
        Target::Summary {} => DocumentTarget::Summary,
        Target::TemplateOutput { document_id } => DocumentTarget::TemplateOutput { document_id },
    })
}

pub(crate) fn validate(record: &Intent, limits: crate::ContentLimits) -> Result<(), StoreError> {
    crate::validate_session_id(&record.session_id)?;
    if record.schema_version != 5 || !record.location.agrees(&record.target) {
        return Err(plan::invalid(
            "Invalid document editor recovery identity/version",
        ));
    }
    record.location.path(&record.session_id)?;
    if [
        &record.before_document,
        &record.after_document,
        &record.before_metadata,
        &record.after_metadata,
        &record.before_tasks,
        &record.after_tasks,
    ]
    .iter()
    .any(|hash| {
        hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }) {
        return Err(plan::invalid("Invalid editor recovery revisions"));
    }
    if let Some(metadata) = &record.metadata {
        let typed: crate::SessionMeta =
            serde_json::from_value(metadata.clone()).map_err(plan::parse)?;
        let bytes = plan::encode(metadata, limits.metadata_bytes)?;
        if typed.id != record.session_id
            || crate::metadata_revision::revision(&record.session_id, &bytes)
                != record.after_metadata
        {
            return Err(plan::invalid("Editor metadata recovery binding changed"));
        }
    } else if record.before_metadata != record.after_metadata {
        return Err(plan::invalid("Missing editor metadata recovery data"));
    }
    if let Some(tasks) = &record.tasks {
        let typed: crate::tasks::TasksFile =
            serde_json::from_value(tasks.clone()).map_err(plan::parse)?;
        let mut ids = std::collections::HashSet::new();
        if typed.tasks.iter().any(|task| !ids.insert(&task.id)) {
            return Err(plan::invalid("Recovery task identifiers are not unique"));
        }
        let bytes = plan::encode(tasks, limits.tasks_file_bytes)?;
        if crate::tasks_revision::revision(&record.session_id, Some(&bytes)) != record.after_tasks {
            return Err(plan::invalid("Editor tasks recovery binding changed"));
        }
    } else if record.before_tasks != record.after_tasks {
        return Err(plan::invalid("Missing editor tasks recovery data"));
    }
    bounded_serialization(record, limits.recovery_bytes)
}

pub(super) fn before(record: &Intent, state: &State) -> bool {
    state.snapshot.document.revision == record.before_document
        && state.snapshot.metadata.revision == record.before_metadata
        && state.snapshot.tasks.revision == record.before_tasks
}

pub(crate) async fn recover(
    store: &SessionStore,
    guard: &WriteGuard<'_>,
    path: &Path,
    record: &Intent,
) -> Result<(), StoreError> {
    validate(record, store.content_limits)?;
    let current = store
        .editor_state_locked(guard, &record.session_id, &record.target)
        .await?
        .ok_or_else(|| {
            StoreError::Conflict("Editor target disappeared; recovery requires review".into())
        })?;
    if current.content.location != record.location {
        return Err(StoreError::Conflict(
            "Editor location changed; recovery requires review".into(),
        ));
    }
    if current.snapshot.document.revision == record.after_document {
        finish(store, guard, path, record).await.map(|_| ())
    } else if before(record, &current) {
        crate::document_operation::finish_title_intent(path).await
    } else {
        Err(StoreError::Conflict(
            "Editor files diverged; recovery requires review".into(),
        ))
    }
}

pub(super) async fn finish(
    store: &SessionStore,
    guard: &WriteGuard<'_>,
    path: &Path,
    record: &Intent,
) -> Result<DocumentEditorSnapshot, StoreError> {
    let current = store
        .editor_state_locked(guard, &record.session_id, &record.target)
        .await?
        .ok_or_else(|| StoreError::Conflict("Editor target disappeared during recovery".into()))?;
    if current.content.location != record.location
        || current.snapshot.document.revision != record.after_document
        || ![
            record.before_metadata.as_str(),
            record.after_metadata.as_str(),
        ]
        .contains(&current.snapshot.metadata.revision.as_str())
        || ![record.before_tasks.as_str(), record.after_tasks.as_str()]
            .contains(&current.snapshot.tasks.revision.as_str())
    {
        return Err(StoreError::Conflict(
            "Editor files changed during recovery; requires review".into(),
        ));
    }
    let directory = crate::paths::validated_session_dir(&record.session_id)?;
    if current.snapshot.metadata.revision != record.after_metadata {
        let bytes = plan::encode(
            record
                .metadata
                .as_ref()
                .ok_or_else(|| plan::invalid("Missing metadata plan"))?,
            store.content_limits.metadata_bytes,
        )?;
        store
            .write_file_locked(guard, crate::paths::meta_path_in(&directory), bytes)
            .await?;
    }
    #[cfg(test)]
    super::tests::checkpoint("metadata");
    let current = store
        .editor_state_locked(guard, &record.session_id, &record.target)
        .await?
        .ok_or_else(|| {
            StoreError::Conflict("Editor target disappeared before task write".into())
        })?;
    if current.content.location != record.location
        || current.snapshot.document.revision != record.after_document
        || current.snapshot.metadata.revision != record.after_metadata
        || ![record.before_tasks.as_str(), record.after_tasks.as_str()]
            .contains(&current.snapshot.tasks.revision.as_str())
    {
        return Err(StoreError::Conflict(
            "Editor inputs changed before task write; recovery requires review".into(),
        ));
    }
    if current.snapshot.tasks.revision != record.after_tasks {
        let bytes = plan::encode(
            record
                .tasks
                .as_ref()
                .ok_or_else(|| plan::invalid("Missing tasks plan"))?,
            store.content_limits.tasks_file_bytes,
        )?;
        store
            .write_file_locked(
                guard,
                crate::paths::session_tasks_path_in(&directory),
                bytes,
            )
            .await?;
    }
    #[cfg(test)]
    super::tests::checkpoint("tasks");
    let after = store
        .editor_state_locked(guard, &record.session_id, &record.target)
        .await?
        .ok_or_else(|| StoreError::Conflict("Editor target disappeared after recovery".into()))?;
    if after.snapshot.document.revision != record.after_document
        || after.snapshot.metadata.revision != record.after_metadata
        || after.snapshot.tasks.revision != record.after_tasks
    {
        return Err(StoreError::Conflict(
            "Editor readback diverged; recovery requires review".into(),
        ));
    }
    let paths = [
        store
            .vault_base
            .join(record.location.path(&record.session_id)?),
        store
            .vault_base
            .join(crate::paths::meta_path_in(&directory)),
        store
            .vault_base
            .join(crate::paths::session_tasks_path_in(&directory)),
    ];
    tokio::task::spawn_blocking(move || {
        for path in paths {
            crate::operation_io::sync_parent(&path)?;
        }
        Ok::<_, StoreError>(())
    })
    .await
    .map_err(|error| StoreError::Io(error.to_string()))??;
    store.index_editor(&record.session_id, &after);
    #[cfg(test)]
    super::tests::checkpoint("readback");
    crate::document_operation::finish_title_intent(path).await?;
    #[cfg(test)]
    super::tests::checkpoint("removed");
    Ok(after.snapshot)
}

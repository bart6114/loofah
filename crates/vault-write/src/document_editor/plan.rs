use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Location {
    Summary {},
    LegacySummary { document_id: String },
    TemplateOutput { document_id: String },
}

impl Location {
    pub fn path(&self, id: &str) -> Result<PathBuf, StoreError> {
        let directory = crate::paths::validated_session_dir(id)?;
        Ok(match self {
            Self::Summary {} => crate::paths::summary_path_in(&directory),
            Self::LegacySummary { document_id } | Self::TemplateOutput { document_id } => {
                crate::validate_doc_id(document_id)?;
                crate::paths::enhanced_doc_path_in(&directory, document_id)
            }
        })
    }
    pub fn agrees(&self, target: &DocumentTarget) -> bool {
        match (self, target) {
            (Self::Summary {} | Self::LegacySummary { .. }, DocumentTarget::Summary) => true,
            (
                Self::TemplateOutput { document_id: a },
                DocumentTarget::TemplateOutput { document_id: b },
            ) => a == b,
            _ => false,
        }
    }
    pub fn source<'a>(&'a self, id: &'a str) -> (&'static str, &'a str) {
        match self {
            Self::Summary {} => ("session_summary", id),
            Self::LegacySummary { document_id } | Self::TemplateOutput { document_id } => {
                ("enhanced_note", document_id)
            }
        }
    }
    pub fn owns(&self, id: &str, task: &crate::TaskItem) -> bool {
        let (kind, source) = self.source(id);
        (task.source_type == kind && task.source_id == source)
            || (matches!(self, Self::LegacySummary { .. })
                && task.source_type == "session_summary"
                && task.source_id == id)
    }
}

pub(super) struct Content {
    pub location: Location,
    pub raw: Option<String>,
    pub document: Option<crate::EnhancedDoc>,
    pub tasks_raw: Option<Value>,
    pub tasks: Vec<crate::TaskItem>,
    pub tasks_revision: String,
}

pub(super) struct State {
    pub snapshot: DocumentEditorSnapshot,
    pub metadata_raw: Value,
    pub content: Content,
}

impl State {
    pub fn new(
        id: &str,
        target: &DocumentTarget,
        metadata: MetadataSnapshot,
        metadata_raw: Value,
        content: Content,
    ) -> Result<Self, StoreError> {
        let mut tasks: Vec<_> = content
            .tasks
            .iter()
            .filter(|task| content.location.owns(id, task))
            .cloned()
            .collect();
        if matches!(target, DocumentTarget::Summary) {
            for task in &mut tasks {
                task.source_type = "session_summary".into();
                task.source_id = id.into();
            }
        }
        crate::tasks::sort_tasks(&mut tasks);
        Ok(Self {
            snapshot: DocumentEditorSnapshot {
                document: DocumentSnapshot {
                    document: content.document.as_ref().map(|document| {
                        if matches!(target, DocumentTarget::Summary) {
                            hypr_vault_read::summary::as_document(id, document.markdown.clone())
                        } else {
                            document.clone()
                        }
                    }),
                    revision: crate::document_revision::revision(
                        id,
                        target,
                        content.raw.as_deref().map(str::as_bytes),
                    )?,
                },
                metadata,
                tasks: TasksSnapshot {
                    tasks,
                    revision: content.tasks_revision.clone(),
                },
            },
            metadata_raw,
            content,
        })
    }
}

pub(super) fn read_content(
    vault: &Path,
    id: &str,
    target: &DocumentTarget,
    limits: crate::ContentLimits,
) -> Result<Option<Content>, StoreError> {
    let directory = crate::paths::validated_session_dir(id)?;
    let enhanced = vault.join(crate::paths::enhanced_dir_in(&directory));
    regular_directory(&enhanced)?;
    let (location, raw, document) = match target {
        DocumentTarget::TemplateOutput { document_id } => {
            crate::validate_doc_id(document_id)?;
            let location = Location::TemplateOutput {
                document_id: document_id.clone(),
            };
            let Some(raw) = read_text(&vault.join(location.path(id)?), limits.note_read_bytes)?
            else {
                return Ok(None);
            };
            let document = hypr_vault_read::parse_enhanced_file(document_id, id, &raw)?;
            if document.kind != "template_output" {
                return Err(StoreError::Conflict(
                    "Target is not a template output".into(),
                ));
            }
            (location, Some(raw), Some(document))
        }
        DocumentTarget::Summary => {
            // Same legacy priority/order as summary::locate_in, with bounded reads.
            let canonical = read_text(
                &vault.join(crate::paths::summary_path_in(&directory)),
                limits.note_read_bytes,
            );
            let mut selected: Option<(crate::EnhancedDoc, String)> = None;
            let mut first_error = None;
            let mut inspected = 0usize;
            let mut remaining = limits.note_read_bytes;
            if enhanced.exists() {
                for entry in std::fs::read_dir(&enhanced).map_err(io)? {
                    let entry = entry.map_err(io)?;
                    inspected += 1;
                    if limits.note_read_bytes != usize::MAX && inspected > 4096 {
                        return Err(limit("Too many legacy directory entries to inspect"));
                    }
                    let path = entry.path();
                    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    if stem.starts_with('.')
                        || stem.contains(".conflict-")
                        || path.extension().and_then(|s| s.to_str()) != Some("md")
                    {
                        continue;
                    }
                    let parsed = (|| {
                        crate::validate_doc_id(stem)?;
                        let raw = read_text(&path, remaining)?.ok_or_else(|| {
                            StoreError::Conflict("Document disappeared during inspection".into())
                        })?;
                        if remaining != usize::MAX {
                            remaining = remaining.checked_sub(raw.len()).ok_or_else(|| {
                                limit("Legacy documents exceed inspection budget")
                            })?;
                        }
                        let doc = hypr_vault_read::parse_enhanced_file(stem, id, &raw)?;
                        Ok::<_, StoreError>((doc, raw))
                    })();
                    match parsed {
                        Ok((doc, raw)) if doc.kind == "summary" => {
                            if selected.as_ref().is_none_or(|(prior, _)| {
                                (doc.sort_order, &doc.id) < (prior.sort_order, &prior.id)
                            }) {
                                selected = Some((doc, raw));
                            }
                        }
                        Ok(_) => {}
                        Err(StoreError::ResourceLimit(message)) => {
                            return Err(StoreError::ResourceLimit(message));
                        }
                        Err(error) => {
                            first_error.get_or_insert(error);
                        }
                    }
                }
            }
            if let Some((doc, raw)) = selected {
                (
                    Location::LegacySummary {
                        document_id: doc.id.clone(),
                    },
                    Some(raw),
                    Some(doc),
                )
            } else {
                let raw = canonical?;
                if raw.is_none()
                    && let Some(error) = first_error
                {
                    return Err(error);
                }
                let doc = raw
                    .as_ref()
                    .map(|raw| hypr_vault_read::summary::as_document(id, raw.clone()));
                (Location::Summary {}, raw, doc)
            }
        }
    };
    let tasks_bytes = read_bytes(
        &vault.join(crate::paths::session_tasks_path_in(&directory)),
        limits.tasks_file_bytes,
    )?;
    let tasks_raw = tasks_bytes.as_deref().map(unique).transpose()?;
    if tasks_raw.as_ref().is_some_and(|value| !value.is_object()) {
        return Err(invalid("Tasks must be an object"));
    }
    let tasks: Vec<crate::TaskItem> = tasks_raw
        .as_ref()
        .map(|value| {
            serde_json::from_value::<crate::tasks::TasksFile>(value.clone())
                .map(|file| file.tasks)
                .map_err(parse)
        })
        .transpose()?
        .unwrap_or_default();
    let mut ids = HashSet::new();
    if tasks.iter().any(|task| !ids.insert(&task.id)) {
        return Err(invalid("Stored task IDs are not unique"));
    }
    Ok(Some(Content {
        location,
        raw,
        document,
        tasks_raw,
        tasks,
        tasks_revision: crate::tasks_revision::revision(id, tasks_bytes.as_deref()),
    }))
}

pub(super) fn plan_tasks(
    state: &State,
    id: &str,
    inputs: Vec<TaskInput>,
) -> Result<Option<Value>, StoreError> {
    let logical_source = match &state.content.location {
        Location::TemplateOutput { document_id } => ("enhanced_note", document_id.as_str()),
        _ => ("session_summary", id),
    };
    let mut logical = state.content.tasks.clone();
    for task in &mut logical {
        if state.content.location.owns(id, task) {
            task.source_type = logical_source.0.into();
            task.source_id = logical_source.1.into();
        }
    }
    crate::tasks::validate_source_task_ids(&logical, logical_source.0, logical_source.1, &inputs)?;
    let next =
        crate::tasks::replaced_source_tasks(&logical, logical_source.0, logical_source.1, inputs);
    let mut root = state
        .content
        .tasks_raw
        .clone()
        .unwrap_or_else(|| serde_json::json!({"tasks":[]}));
    let prior = root
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let owned_ids: HashSet<_> = state
        .content
        .tasks
        .iter()
        .filter(|task| state.content.location.owns(id, task))
        .map(|task| task.id.as_str())
        .collect();
    let prior_by_id: HashMap<_, _> = prior
        .iter()
        .map(|row| (row["id"].as_str().unwrap(), row))
        .collect();
    let mut rows: Vec<_> = prior
        .iter()
        .filter(|row| !owned_ids.contains(row["id"].as_str().unwrap()))
        .cloned()
        .collect();
    for task in next
        .iter()
        .filter(|task| task.source_type == logical_source.0 && task.source_id == logical_source.1)
    {
        let old = prior_by_id.get(task.id.as_str()).copied();
        let owned = serde_json::to_value(task).map_err(parse)?;
        let mut row = old.cloned().unwrap_or_else(|| owned.clone());
        let object = row
            .as_object_mut()
            .ok_or_else(|| invalid("Stored task must be an object"))?;
        for key in [
            "id",
            "source_order",
            "status",
            "text",
            "body",
            "due_at",
            "assignee",
            "created_at",
            "updated_at",
        ] {
            object.insert(key.into(), owned[key].clone());
        }
        if old.is_none() {
            let (kind, source) = state.content.location.source(id);
            object.insert("source_type".into(), kind.into());
            object.insert("source_id".into(), source.into());
        }
        rows.push(row);
    }
    root.as_object_mut()
        .unwrap()
        .insert("tasks".into(), Value::Array(rows));
    Ok(
        if state.content.tasks_raw.as_ref() == Some(&root)
            || (state.content.tasks_raw.is_none() && root["tasks"].as_array().unwrap().is_empty())
        {
            None
        } else {
            Some(root)
        },
    )
}

pub(super) fn read_bytes(path: &Path, maximum: usize) -> Result<Option<Vec<u8>>, StoreError> {
    let Some(file) = crate::media::open_regular_file(path)? else {
        return Ok(None);
    };
    if file.metadata().map_err(io)?.len() > maximum as u64 {
        return Err(limit("File exceeds editor inspection limit"));
    }
    let mut bytes = Vec::new();
    file.take(maximum.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > maximum {
        return Err(limit("File exceeds editor inspection limit"));
    }
    Ok(Some(bytes))
}
fn read_text(path: &Path, maximum: usize) -> Result<Option<String>, StoreError> {
    read_bytes(path, maximum)?
        .map(|bytes| String::from_utf8(bytes).map_err(parse))
        .transpose()
}
fn regular_directory(path: &Path) -> Result<(), StoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
            Err(invalid("Document directory is not regular"))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io(error)),
    }
}
pub(super) fn unique(bytes: &[u8]) -> Result<Value, StoreError> {
    serde_json::from_slice::<crate::metadata_revision::UniqueValue>(bytes)
        .map(|value| value.0)
        .map_err(parse)
}
pub(super) fn encode(value: &impl Serialize, maximum: usize) -> Result<Vec<u8>, StoreError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(parse)?;
    if bytes.len() > maximum {
        return Err(limit("Prospective editor file exceeds its limit"));
    }
    Ok(bytes)
}
pub(super) fn invalid(message: &str) -> StoreError {
    StoreError::Serialize(message.into())
}
pub(super) fn limit(message: &str) -> StoreError {
    StoreError::ResourceLimit(message.into())
}
pub(super) fn io(error: std::io::Error) -> StoreError {
    StoreError::Io(error.to_string())
}
pub(super) fn parse(error: impl std::fmt::Display) -> StoreError {
    StoreError::Serialize(error.to_string())
}

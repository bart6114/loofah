use serde::{Deserialize, Serialize};

use crate::{DocumentSnapshot, MetadataSnapshot, TaskInput, TasksSnapshot};

pub const DOCUMENT_EDITOR_RESPONSE_LIMIT: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct DocumentEditorSnapshot {
    pub document: DocumentSnapshot,
    pub metadata: MetadataSnapshot,
    /// Logical target coordinates only; the revision guards the complete stored tasks file.
    pub tasks: TasksSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct SaveDocumentEditorRequest {
    pub markdown: String,
    pub title: Option<String>,
    pub tasks: Vec<TaskInput>,
    pub expected_revision: String,
    pub expected_metadata_revision: String,
    pub expected_tasks_revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "snapshot", rename_all = "snake_case")]
pub enum DocumentEditorSaveOutcome {
    Saved(DocumentEditorSnapshot),
    Conflict(DocumentEditorSnapshot),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compound_editor_request_requires_all_revisions_and_rejects_transport_source_fields() {
        let request = serde_json::json!({
            "markdown":"Summary", "title":null, "tasks":[],
            "expected_revision":"document", "expected_metadata_revision":"metadata",
            "expected_tasks_revision":"all-session-tasks"
        });
        let parsed: SaveDocumentEditorRequest = serde_json::from_value(request.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), request);
        for field in [
            "expected_revision",
            "expected_metadata_revision",
            "expected_tasks_revision",
        ] {
            let mut invalid = request.clone();
            invalid.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<SaveDocumentEditorRequest>(invalid).is_err());
        }
        let mut invalid = request;
        invalid["source_id"] = "foreign".into();
        assert!(serde_json::from_value::<SaveDocumentEditorRequest>(invalid).is_err());
    }
}

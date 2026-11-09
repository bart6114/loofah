use serde::{Deserialize, Serialize};

use crate::DocumentEditorSnapshot;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct DeleteDocumentRequest {
    pub expected_revision: String,
    pub expected_session_created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeleteDocumentOutcome {
    Deleted {
        revision: String,
        trash_token: String,
    },
    Conflict {
        snapshot: Box<DocumentEditorSnapshot>,
    },
    NotFound,
}

pub fn validate_delete_document_request(input: &DeleteDocumentRequest) -> Result<(), &'static str> {
    if !crate::is_session_lifecycle_revision(&input.expected_revision) {
        return Err("A checked document revision is required");
    }
    if input.expected_session_created_at.is_empty()
        || input.expected_session_created_at.len() > 128
        || input
            .expected_session_created_at
            .chars()
            .any(char::is_control)
    {
        return Err("The original session generation is required");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_deletion_requires_original_revision_and_generation_without_transport_authority() {
        let input = serde_json::json!({"expected_revision":"a".repeat(64),"expected_session_created_at":"2026-11-08T10:00:00Z"});
        let parsed: DeleteDocumentRequest = serde_json::from_value(input.clone()).unwrap();
        validate_delete_document_request(&parsed).unwrap();
        for field in ["expected_revision", "expected_session_created_at"] {
            let mut missing = input.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<DeleteDocumentRequest>(missing).is_err());
        }
        for field in ["path", "member_id", "trash_path", "force"] {
            let mut extra = input.clone();
            extra[field] = true.into();
            assert!(serde_json::from_value::<DeleteDocumentRequest>(extra).is_err());
        }
        let mut invalid = parsed;
        invalid.expected_session_created_at.clear();
        assert!(validate_delete_document_request(&invalid).is_err());
    }
}

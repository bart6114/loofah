use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, MetadataSaveOutcome, MetadataSnapshot, SaveMetadataRequest,
    SessionMetaPatch,
};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn read_metadata(
        &self,
        session_id: &str,
    ) -> Result<MetadataSnapshot, ApplicationError> {
        self.validate(session_id)?;
        self.store
            .read_metadata_snapshot(session_id)
            .await
            .map_err(|cause| storage_error(cause, false))?
            .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))
    }

    pub async fn save_metadata(
        &self,
        session_id: &str,
        input: SaveMetadataRequest,
    ) -> Result<MetadataSaveOutcome, ApplicationError> {
        self.validate(session_id)?;
        validate_patch(&input)?;
        self.store
            .update_metadata_if_revision(session_id, input.patch, &input.expected_revision)
            .await
            .map_err(|cause| storage_error(cause, true))?
            .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))
    }
}

fn validate_patch(input: &SaveMetadataRequest) -> Result<(), ApplicationError> {
    if input.expected_revision.is_empty() || input.expected_revision.len() > 128 {
        return Err(error(
            ErrorCode::Validation,
            "Expected metadata revision is required",
        ));
    }
    if input.patch == SessionMetaPatch::default() {
        return Err(error(
            ErrorCode::Validation,
            "A metadata change is required",
        ));
    }
    let patch = &input.patch;
    if [
        &patch.title,
        &patch.author,
        &patch.tracking_id,
        &patch.folder,
    ]
    .iter()
    .any(|field| field.as_ref().is_some_and(|value| value.len() > 4096))
        || patch
            .tags
            .as_ref()
            .is_some_and(|tags| tags.len() > 64 || tags.iter().any(|tag| tag.len() > 4096))
    {
        return Err(error(
            ErrorCode::ResourceLimit,
            "Metadata patch exceeds the request size limit",
        ));
    }
    if patch.tags.as_ref().is_some_and(|tags| {
        tags.iter()
            .any(|tag| hypr_vault_read::normalize_tag_name(tag).is_none())
    }) {
        return Err(error(ErrorCode::Validation, "Tag names cannot be empty"));
    }
    for (name, value, empty) in [
        ("created_at", patch.created_at.as_deref(), false),
        ("started_at", patch.started_at.as_deref(), true),
        ("ended_at", patch.ended_at.as_deref(), true),
    ] {
        if let Some(value) = value
            && !(empty && value.is_empty())
            && chrono::DateTime::parse_from_rfc3339(value).is_err()
        {
            return Err(error(
                ErrorCode::Validation,
                &format!("{name} must be an RFC 3339 timestamp"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
    use hypr_vault_write::SessionStore;
    use std::sync::Arc;

    #[tokio::test]
    async fn metadata_operations_share_checked_writes_and_validate_readiness_and_inputs() {
        let root = tempfile::tempdir().unwrap();
        let context = VaultContext::new(
            "vault".into(),
            Arc::new(SessionStore::new(root.path().into())),
            StartupState::new(Some(root.path())),
            Arc::new(ConfigState::load_or_default(root.path())),
            Arc::new(SearchService::default()),
        );
        context.store.set_startup_pending(true);
        assert_eq!(
            context.read_metadata("note").await.unwrap_err().code,
            ErrorCode::Unavailable
        );
        context.store.set_startup_pending(false);
        assert_eq!(
            context.read_metadata("../other").await.unwrap_err().code,
            ErrorCode::Validation
        );
        assert_eq!(
            context.read_metadata("missing").await.unwrap_err().code,
            ErrorCode::NotFound
        );
        let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Original","created_at":"2026-11-05T10:00:00Z","tags":[]})).unwrap();
        context
            .store
            .create_note_session(&meta, "Note")
            .await
            .unwrap();
        let before = context.read_metadata("note").await.unwrap();
        let input = SaveMetadataRequest {
            patch: SessionMetaPatch {
                title: Some("Renamed".into()),
                ..Default::default()
            },
            expected_revision: before.revision,
        };
        assert!(matches!(
            context.save_metadata("note", input.clone()).await.unwrap(),
            MetadataSaveOutcome::Saved(_)
        ));
        assert!(matches!(
            context.save_metadata("note", input.clone()).await.unwrap(),
            MetadataSaveOutcome::Conflict(_)
        ));
        let cases = [
            (SessionMetaPatch::default(), ErrorCode::Validation),
            (
                SessionMetaPatch {
                    title: Some("x".repeat(4097)),
                    ..Default::default()
                },
                ErrorCode::ResourceLimit,
            ),
            (
                SessionMetaPatch {
                    created_at: Some("not a date".into()),
                    ..Default::default()
                },
                ErrorCode::Validation,
            ),
            (
                SessionMetaPatch {
                    tags: Some(vec!["".into()]),
                    ..Default::default()
                },
                ErrorCode::Validation,
            ),
            (
                SessionMetaPatch {
                    tags: Some(vec!["tag".into(); 65]),
                    ..Default::default()
                },
                ErrorCode::ResourceLimit,
            ),
        ];
        for (patch, code) in cases {
            assert_eq!(
                context
                    .save_metadata(
                        "note",
                        SaveMetadataRequest {
                            patch,
                            ..input.clone()
                        }
                    )
                    .await
                    .unwrap_err()
                    .code,
                code
            );
        }
        assert_eq!(
            context.read_metadata("note").await.unwrap().meta.title,
            "Renamed"
        );
        assert_eq!(
            context.store.read_note("note").await.unwrap().as_deref(),
            Some("Note")
        );
    }
}

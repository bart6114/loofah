use crate::{
    VaultContext,
    vault::{error, storage_error},
};
use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, LocalCliReply, MetadataSaveOutcome, NoteSaveOutcome,
    SaveMetadataRequest, SaveNoteRequest, SessionMetaPatch,
};
use std::sync::Arc;

pub(super) async fn append(
    context: &Arc<VaultContext>,
    id: &str,
    body: String,
    expected_revision: String,
) -> Result<NoteSaveOutcome, ApplicationError> {
    validate_revision(&expected_revision)?;
    let _admission = context.admit_session_work(id).await?;
    let current = context.read_note(id).await?;
    if current.revision != expected_revision {
        return Ok(NoteSaveOutcome::Conflict(current));
    }
    let markdown = match current.markdown {
        Some(existing) if !existing.is_empty() => {
            if existing.ends_with('\n') {
                format!("{existing}{body}")
            } else {
                format!("{existing}\n{body}")
            }
        }
        _ => body,
    };
    context
        .save_note(
            id,
            SaveNoteRequest {
                markdown,
                title: None,
                expected_revision,
            },
        )
        .await
}

pub(super) async fn tags(
    context: &Arc<VaultContext>,
    id: &str,
    requested: Vec<String>,
    remove: bool,
    expected_revision: String,
) -> Result<LocalCliReply, ApplicationError> {
    validate_revision(&expected_revision)?;
    let _admission = context.admit_session_work(id).await?;
    let current = context.read_metadata(id).await?;
    let normalize = |tags: &[String]| {
        let mut tags = tags
            .iter()
            .filter_map(|tag| hypr_vault_read::normalize_tag_name(tag))
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();
        tags
    };
    let existing = normalize(&current.meta.tags);
    if current.revision != expected_revision {
        return Ok(LocalCliReply::TagDelta {
            outcome: MetadataSaveOutcome::Conflict(current),
            changed: Vec::new(),
            tags: existing,
        });
    }
    let requested = requested
        .into_iter()
        .map(|tag| {
            hypr_vault_read::normalize_tag_name(&tag)
                .ok_or_else(|| error(ErrorCode::Validation, "Tag name cannot be empty"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut changed = requested
        .iter()
        .filter(|tag| existing.contains(tag) == remove)
        .cloned()
        .collect::<Vec<_>>();
    changed.sort();
    changed.dedup();
    let finalized = if remove {
        existing
            .into_iter()
            .filter(|tag| !requested.contains(tag))
            .collect()
    } else {
        let mut tags = existing;
        tags.extend(changed.iter().cloned());
        tags.sort();
        tags
    };
    let outcome = if changed.is_empty() {
        MetadataSaveOutcome::Saved(current)
    } else {
        context
            .save_metadata_local(
                id,
                SaveMetadataRequest {
                    patch: SessionMetaPatch {
                        tags: Some(finalized.clone()),
                        ..Default::default()
                    },
                    expected_revision,
                },
            )
            .await?
    };
    if let MetadataSaveOutcome::Saved(snapshot) = &outcome {
        if !remove {
            for tag in &changed {
                context
                    .store
                    .ensure_tag_checked(tag)
                    .await
                    .map_err(|cause| {
                        let mut failure = storage_error(cause, false);
                        failure.message = format!("Tags for session {id} were saved, but updating the tag registry failed; do not repeat the edit");
                        failure.mutation_outcome = Some(hypr_loofah_protocol::MutationOutcome::Persisted);
                        failure.current_revision = Some(snapshot.revision.clone());
                        failure.retryable = false;
                        failure
                    })?;
            }
        }
    } else {
        return Ok(LocalCliReply::TagDelta {
            outcome,
            changed: Vec::new(),
            tags: finalized,
        });
    }
    Ok(LocalCliReply::TagDelta {
        outcome,
        changed,
        tags: finalized,
    })
}

fn validate_revision(revision: &str) -> Result<(), ApplicationError> {
    if revision.is_empty() || revision.len() > 128 {
        return Err(error(
            ErrorCode::Validation,
            "Expected document revision is required",
        ));
    }
    Ok(())
}

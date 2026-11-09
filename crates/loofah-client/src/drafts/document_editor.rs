use super::*;

fn coordinate(session_id: &str, target: &DocumentTarget) -> String {
    match target {
        DocumentTarget::Summary => {
            serde_json::to_string(&["document_editor", session_id, "summary"])
        }
        DocumentTarget::TemplateOutput { document_id } => serde_json::to_string(&[
            "document_editor",
            session_id,
            "template_output",
            document_id,
        ]),
    }
    .expect("string coordinates serialize")
}

pub(super) fn validate(recovery: &NoteDraftRecovery) -> Result<()> {
    let Some(document) = &recovery.document_editor else {
        if serde_json::from_str::<serde_json::Value>(&recovery.session_id)
            .ok()
            .and_then(|value| value.as_array().and_then(|values| values.first()).cloned())
            .is_some_and(|kind| kind == "document_editor")
        {
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "This recovery copy requires the document editor. Its file was kept.",
            ));
        }
        return Ok(());
    };
    let invalid = || {
        failure(
            ErrorCode::Validation,
            "Document recovery does not match its account, session, target or baseline",
        )
    };
    hypr_loofah_protocol::validate_session_id(&document.session_id).map_err(|_| invalid())?;
    let (source_type, source_id) = match &document.target {
        DocumentTarget::Summary => ("session_summary", document.session_id.as_str()),
        DocumentTarget::TemplateOutput { document_id } => {
            hypr_loofah_protocol::validate_session_id(document_id).map_err(|_| invalid())?;
            ("enhanced_note", document_id.as_str())
        }
    };
    let tasks = recovery.editor_tasks.as_ref().ok_or_else(invalid)?;
    if recovery.session_id != coordinate(&document.session_id, &document.target)
        || document.metadata.meta.id != document.session_id
        || recovery.baseline.title != document.metadata.meta.title
        || [
            &recovery.baseline.revision,
            &document.metadata.revision,
            &tasks.baseline.revision,
        ]
        .into_iter()
        .any(|revision| {
            revision.is_empty() || revision.len() > 128 || revision.chars().any(char::is_control)
        })
        || tasks
            .baseline
            .tasks
            .iter()
            .any(|task| task.source_type != source_type || task.source_id != source_id)
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests;

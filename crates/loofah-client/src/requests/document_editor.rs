use super::*;

pub(super) fn is_save(operation: &RemoteOperation) -> bool {
    matches!(operation, RemoteOperation::SaveDocumentEditor { .. })
}

pub(super) fn validate_operation(operation: &RemoteOperation) -> Result<()> {
    let (session_id, target) = match operation {
        RemoteOperation::DocumentEditor {
            session_id, target, ..
        }
        | RemoteOperation::SaveDocumentEditor {
            session_id, target, ..
        } => (session_id, target),
        _ => return Ok(()),
    };
    validate_session_id(session_id).map_err(|_| invalid("Invalid document owner"))?;
    if let DocumentTarget::TemplateOutput { document_id } = target {
        validate_session_id(document_id).map_err(|_| invalid("Invalid document target"))?;
    }
    Ok(())
}

fn revision(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

pub(super) fn validate_snapshot(
    snapshot: &DocumentEditorSnapshot,
    session_id: &str,
    target: &DocumentTarget,
) -> Result<()> {
    let malformed = || {
        failure(
            ErrorCode::IncompatibleVersion,
            "Invalid compound document response",
        )
    };
    if snapshot.metadata.meta.id != session_id
        || !revision(&snapshot.document.revision)
        || !revision(&snapshot.metadata.revision)
        || !revision(&snapshot.tasks.revision)
    {
        return Err(malformed());
    }
    match (&snapshot.document.document, target) {
        (None, DocumentTarget::Summary) => {}
        (Some(document), target) => {
            if document.session_id != session_id || validate_session_id(&document.id).is_err() {
                return Err(malformed());
            }
            let matches = match target {
                DocumentTarget::Summary => document.kind == "summary" && document.id == session_id,
                DocumentTarget::TemplateOutput { document_id } => {
                    document.kind == "template_output" && document.id == *document_id
                }
            };
            if !matches {
                return Err(malformed());
            }
        }
        (None, DocumentTarget::TemplateOutput { .. }) => return Err(malformed()),
    }
    let (source_type, source_id) = match target {
        DocumentTarget::Summary => ("session_summary", session_id),
        DocumentTarget::TemplateOutput { document_id } => ("enhanced_note", document_id.as_str()),
    };
    let mut ids = std::collections::HashSet::new();
    for task in &snapshot.tasks.tasks {
        if task.id.is_empty()
            || task.id.len() > 512
            || task.id.chars().any(char::is_control)
            || !ids.insert(task.id.as_str())
            || task.source_type != source_type
            || task.source_id != source_id
        {
            return Err(malformed());
        }
    }
    Ok(())
}

pub(super) fn validate_outcome(
    outcome: &DocumentEditorSaveOutcome,
    session_id: &str,
    target: &DocumentTarget,
    conflict: bool,
) -> Result<()> {
    let snapshot = match (outcome, conflict) {
        (DocumentEditorSaveOutcome::Saved(snapshot), false)
        | (DocumentEditorSaveOutcome::Conflict(snapshot), true) => snapshot,
        _ => {
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "Invalid compound document save outcome",
            ));
        }
    };
    if matches!(outcome, DocumentEditorSaveOutcome::Saved(_))
        && snapshot.document.document.is_none()
    {
        return Err(failure(
            ErrorCode::IncompatibleVersion,
            "Saved document is missing from the response",
        ));
    }
    validate_snapshot(snapshot, session_id, target)
}

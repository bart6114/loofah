use super::*;
pub(super) async fn load(
    context: &Arc<VaultContext>,
    request: &str,
) -> Result<Option<Record>, ApplicationError> {
    load_scoped(context, request, None).await
}
pub(super) async fn load_scoped(
    context: &Arc<VaultContext>,
    request: &str,
    principal: Option<&str>,
) -> Result<Option<Record>, ApplicationError> {
    if let Some(principal) = principal {
        crate::jobs::validate_namespace(principal)?;
    }
    uuid(request)?;
    let root = root(context, principal)?;
    let principal = principal.map(str::to_owned);
    let request = request.to_owned();
    let owner = context.clone();
    tokio::task::spawn_blocking(move || {
        let _owner = &owner;
        if !ledger::existing_directory(&root)? {
            return Ok(None);
        }
        let path = root.join(format!("{request}.json"));
        if !ledger::check_file(&path)? {
            return Ok(None);
        }
        let record: Record =
            serde_json::from_slice(&ledger::read(&path, 128 * 1024)?).map_err(review)?;
        validate(&record)?;
        if record.principal != principal
            || record.declaration.request_id != request
            || record.vault_id != owner.events.vault_id()
        {
            return Err(review("Manual summary original identity changed"));
        }
        Ok(Some(record))
    })
    .await
    .map_err(review)?
}
pub(super) async fn persist(
    context: &Arc<VaultContext>,
    record: &Record,
    create: bool,
) -> Result<(), ApplicationError> {
    validate(record)?;
    let root = root(context, record.principal.as_deref())?;
    let record = record.clone();
    let owner = context.clone();
    tokio::task::spawn_blocking(move || {
        let _owner = owner;
        ledger::ensure_directory(&root)?;
        let path = root.join(format!("{}.json", record.declaration.request_id));
        let bytes = ledger::encode_bounded_result(&record, 128 * 1024)?;
        if create {
            return ledger::write_new(&path, &bytes);
        }
        if !ledger::check_file(&path)? {
            return Err(review("Original manual summary intent disappeared"));
        }
        hypr_storage::fs::atomic_write(&path, std::str::from_utf8(&bytes).map_err(review)?)
            .map_err(review)?;
        ledger::sync_parent(&path)
    })
    .await
    .map_err(review)?
}
fn validate(record: &Record) -> Result<(), ApplicationError> {
    validate_declaration(&record.declaration)?;
    crate::jobs::validate_namespace(record.principal.as_deref().unwrap_or(OWNER))?;
    uuid(&record.status.child_request_id)?;
    if record.version != 1
        || record.status.request_id != record.declaration.request_id
        || record.status.session_id != record.declaration.session_id
        || record
            .session_created_at
            .as_ref()
            .is_some_and(|value| chrono::DateTime::parse_from_rfc3339(value).is_err())
        || (!record.declaration.fresh_session && record.session_created_at.is_none())
    {
        return Err(review("Manual summary parent requires review"));
    }
    match (&record.request, &record.declaration.summary) {
        (None, None) => {}
        (Some(request), Some(options))
            if request.session_id == record.declaration.session_id
                && request.target == options.target
                && request.title_policy == options.title_policy
                && request.only_if_empty == record.declaration.only_if_empty
                && is_transcription_sha256(&request.expected_revision) => {}
        _ => return Err(review("Original manual summary options changed")),
    }
    if matches!(
        record.status.state,
        ManualMediaSummaryState::SourceBound | ManualMediaSummaryState::Child
    ) {
        let origin = ManualMediaChildOrigin {
            principal: record.principal.clone(),
            parent_request_id: record.declaration.request_id.clone(),
            child_request_id: record.status.child_request_id.clone(),
            session_id: record.declaration.session_id.clone(),
            source: record
                .source
                .clone()
                .ok_or_else(|| review("Manual summary source missing"))?,
        };
        origin.validate_request(
            record
                .request
                .as_ref()
                .ok_or_else(|| review("Manual summary request missing"))?,
        )?;
    }
    if let Some(job) = &record.status.job {
        let request = record
            .request
            .as_ref()
            .ok_or_else(|| review("Manual summary child has no request"))?;
        if job.session_id != record.declaration.session_id || job.target != request.target {
            return Err(review("Original manual summary child target changed"));
        }
        uuid(&job.id)?;
    }
    if record.status.state == ManualMediaSummaryState::Child && record.status.job.is_none() {
        return Err(review("Manual summary child missing"));
    }
    Ok(())
}

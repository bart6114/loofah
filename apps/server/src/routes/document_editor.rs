use super::*;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route(
            "/vaults/{vault}/sessions/{session}/summary/editor",
            get(summary).put(save_summary),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/outputs/{document}/editor",
            get(output).put(save_output),
        )
}

async fn summary(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    read(service, identity, vault, session, DocumentTarget::Summary).await
}

async fn output(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session, document_id)): ApiPath<(String, String, String)>,
) -> Result<Response> {
    read(
        service,
        identity,
        vault,
        session,
        DocumentTarget::TemplateOutput { document_id },
    )
    .await
}

async fn read(
    service: Arc<Service>,
    identity: Identity,
    vault: String,
    session: String,
    target: DocumentTarget,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    content_read(
        &service,
        access,
        DOCUMENT_EDITOR_RESPONSE_LIMIT,
        async move { context.read_document_editor(&session, &target).await },
    )
    .await
}

async fn save_summary(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session)): ApiPath<(String, String)>,
    ApiJson(input): ApiJson<SaveDocumentEditorRequest>,
) -> Result<Response> {
    save(
        service,
        identity,
        vault,
        session,
        DocumentTarget::Summary,
        input,
    )
    .await
}

async fn save_output(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session, document_id)): ApiPath<(String, String, String)>,
    ApiJson(input): ApiJson<SaveDocumentEditorRequest>,
) -> Result<Response> {
    save(
        service,
        identity,
        vault,
        session,
        DocumentTarget::TemplateOutput { document_id },
        input,
    )
    .await
}

async fn save(
    service: Arc<Service>,
    identity: Identity,
    vault: String,
    session: String,
    target: DocumentTarget,
    input: SaveDocumentEditorRequest,
) -> Result<Response> {
    // Keep the catalog grant ahead of all Store locks, as for existing note writers.
    let access = service.catalog.access(&identity, &vault, true).await?;
    let context = service.vault(&vault).await?;
    write(&service, access, async move {
        let result = context
            .save_document_editor(&session, &target, input)
            .await?;
        let (status, outcome) = match result {
            DocumentEditorSaveOutcome::Saved(_) => (StatusCode::OK, MutationOutcome::Persisted),
            DocumentEditorSaveOutcome::Conflict(_) => {
                (StatusCode::CONFLICT, MutationOutcome::NotCommitted)
            }
        };
        let bytes = tokio::task::spawn_blocking(move || {
            encode_content(&result, DOCUMENT_EDITOR_RESPONSE_LIMIT)
        })
        .await
        .map_err(ApiError::internal)
        .and_then(|result| result)
        .map_err(|mut cause| {
            cause.0.mutation_outcome = Some(outcome);
            cause
        })?;
        Ok((status, [("content-type", "application/json")], bytes).into_response())
    })
    .await
}

use super::*;
pub(super) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route(
            "/vaults/{vault}/sessions/{session}/transcript/editor",
            get(read),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/transcript/speakers",
            axum::routing::post(assign),
        )
        .route(
            "/vaults/{vault}/people/ensure",
            axum::routing::post(ensure_person),
        )
}
async fn read(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    content_read(
        &service,
        access,
        TRANSCRIPT_EDITOR_RESPONSE_LIMIT,
        async move { context.read_transcript_editor(&session).await },
    )
    .await
}
async fn assign(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session)): ApiPath<(String, String)>,
    ApiJson(input): ApiJson<AssignTranscriptSpeakersRequest>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, true).await?;
    let context = service.vault(&vault).await?;
    write(&service, access, async move {
        let result = context.assign_transcript_speakers(&session, input).await?;
        let (status, outcome) = if matches!(result, TranscriptEditorSaveOutcome::Conflict(_)) {
            (StatusCode::CONFLICT, MutationOutcome::NotCommitted)
        } else {
            (StatusCode::OK, MutationOutcome::Persisted)
        };
        let bytes = tokio::task::spawn_blocking(move || {
            encode_content(&result, TRANSCRIPT_EDITOR_RESPONSE_LIMIT)
        })
        .await
        .map_err(ApiError::internal)
        .and_then(|value| value)
        .map_err(|mut cause| {
            cause.0.mutation_outcome = Some(outcome);
            cause
        })?;
        Ok((status, [("content-type", "application/json")], bytes).into_response())
    })
    .await
}
async fn ensure_person(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    ApiJson(input): ApiJson<EnsurePersonRequest>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, true).await?;
    let context = service.vault(&vault).await?;
    write(&service, access, async move {
        let result = context.ensure_person_checked(input).await?;
        Ok(Json(result).into_response())
    })
    .await
}

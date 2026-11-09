use super::*;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route(
            "/vaults/{vault}/tags/suggestions/query",
            axum::routing::post(suggestions),
        )
        .route("/vaults/{vault}/tags/ensure", axum::routing::post(ensure))
        .route(
            "/vaults/{vault}/sessions/{session}/tag-suggestions/accept",
            axum::routing::post(accept),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/tag-suggestions/dismiss",
            axum::routing::post(dismiss),
        )
}

async fn suggestions(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    ApiJson(input): ApiJson<TagSuggestionsRequest>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    content_read(
        &service,
        access,
        TAG_SUGGESTIONS_RESPONSE_LIMIT,
        async move { context.tag_suggestions(input).await },
    )
    .await
}

async fn ensure(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    ApiJson(input): ApiJson<EnsureTagRequest>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, true).await?;
    let context = service.vault(&vault).await?;
    write(&service, access, async move {
        let tag = context.ensure_tag(input).await?;
        let bytes = tokio::task::spawn_blocking(move || encode_content(&tag, 16 * 1024 * 1024))
            .await
            .map_err(ApiError::internal)??;
        Ok(([("content-type", "application/json")], bytes).into_response())
    })
    .await
}

async fn accept(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session)): ApiPath<(String, String)>,
    ApiJson(input): ApiJson<TagSuggestionActionRequest>,
) -> Result<Response> {
    edit(service, identity, vault, session, input, true).await
}

async fn dismiss(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session)): ApiPath<(String, String)>,
    ApiJson(input): ApiJson<TagSuggestionActionRequest>,
) -> Result<Response> {
    edit(service, identity, vault, session, input, false).await
}

async fn edit(
    service: Arc<Service>,
    identity: Identity,
    vault: String,
    session: String,
    input: TagSuggestionActionRequest,
    accept: bool,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, true).await?;
    let context = service.vault(&vault).await?;
    write(&service, access, async move {
        let result = if accept {
            context.accept_tag_suggestion(&session, input).await?
        } else {
            context.dismiss_tag_suggestion(&session, input).await?
        };
        let status = if matches!(result, MetadataSaveOutcome::Conflict(_)) {
            StatusCode::CONFLICT
        } else {
            StatusCode::OK
        };
        let bytes = tokio::task::spawn_blocking(move || encode_content(&result, 16 * 1024 * 1024))
            .await
            .map_err(ApiError::internal)??;
        Ok((status, [("content-type", "application/json")], bytes).into_response())
    })
    .await
}

use super::*;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route(
            "/vaults/{vault}/timeline/headers",
            axum::routing::post(headers),
        )
        .route(
            "/vaults/{vault}/timeline/facets",
            axum::routing::post(facets),
        )
}

async fn headers(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    ApiJson(input): ApiJson<TimelineQuery>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    content_read(&service, access, TIMELINE_RESPONSE_LIMIT, async move {
        context.timeline_headers(input).await
    })
    .await
}

async fn facets(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    ApiJson(input): ApiJson<TimelineFacetsQuery>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    content_read(&service, access, TIMELINE_RESPONSE_LIMIT, async move {
        context.timeline_facets(input).await
    })
    .await
}

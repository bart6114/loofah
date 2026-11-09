use super::*;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new().route(
        "/vaults/{vault}/session-summaries/query",
        axum::routing::post(query),
    )
}

async fn query(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    ApiJson(input): ApiJson<SessionSummaryQuery>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    content_read(
        &service,
        access,
        SESSION_SUMMARY_RESPONSE_LIMIT,
        async move { context.session_summaries(input).await },
    )
    .await
}

use super::*;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new().route(
        "/vaults/{vault}/sessions/{session}/transcript-overview",
        get(overview),
    )
}

async fn overview(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    content_read(
        &service,
        access,
        TRANSCRIPT_OVERVIEW_RESPONSE_LIMIT,
        async move {
            context
                .transcript_overview(
                    &session,
                    Some((
                        TRANSCRIPT_OVERVIEW_LABEL_LIMIT,
                        TRANSCRIPT_OVERVIEW_RESPONSE_LIMIT,
                    )),
                )
                .await
        },
    )
    .await
}

use super::*;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new().route(
        "/vaults/{vault}/session-cleanups",
        axum::routing::post(delete_empty),
    )
}

async fn delete_empty(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<DeleteEmptySessionRequest>,
) -> Result<Json<DeleteEmptySessionOutcome>> {
    let request_id = request_id(&headers)?.to_owned();
    let access = service.catalog.access(&identity, &vault, true).await?;
    let namespace = access.member_id.clone();
    let context = service.vault(&vault).await?;
    let authorization = service.clone();
    drop(access);
    admitted_write(&service, async move {
        let outcome = context
            .delete_empty_session_authorized(namespace, request_id, input, move || async move {
                if authorization.stop.is_cancelled() {
                    return Err(ApiError::new(ErrorCode::Unavailable, "Server is shutting down").0);
                }
                if identity.expires_at <= now() {
                    return Err(ApiError::new(ErrorCode::Unauthenticated, "Sign in again").0);
                }
                authorization
                    .catalog
                    .access(&identity, &vault, true)
                    .await
                    .map_err(|cause| cause.0)
            })
            .await?;
        Ok(Json(outcome))
    })
    .await
}

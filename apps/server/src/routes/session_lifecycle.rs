use super::*;

const RESPONSE_LIMIT: usize = 16 * 1024;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route(
            "/vaults/{vault}/sessions/{session}/lifecycle",
            get(snapshot),
        )
        .route(
            "/vaults/{vault}/session-deletions",
            axum::routing::post(delete),
        )
        .route(
            "/vaults/{vault}/session-restorations",
            axum::routing::post(restore),
        )
        .layer(DefaultBodyLimit::max(4096))
}
async fn snapshot(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    let mut response = content_read(&service, access, RESPONSE_LIMIT, async move {
        context.read_session_lifecycle(&session).await
    })
    .await?;
    response.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}
async fn authorize(
    service: Arc<Service>,
    identity: Identity,
    vault: String,
) -> std::result::Result<VaultAccess, ApplicationError> {
    if service.stop.is_cancelled() {
        return Err(ApiError::new(ErrorCode::Unavailable, "Server is shutting down").0);
    }
    if identity.expires_at <= now() {
        return Err(ApiError::new(ErrorCode::Unauthenticated, "Sign in again").0);
    }
    service
        .catalog
        .access(&identity, &vault, true)
        .await
        .map_err(|cause| cause.0)
}
async fn delete(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<DeleteSessionRequest>,
) -> Result<Response> {
    let request = request_id(&headers)?.to_owned();
    let access = service.catalog.access(&identity, &vault, true).await?;
    let namespace = access.member_id.clone();
    let context = service.vault(&vault).await?;
    let authorization = service.clone();
    drop(access);
    admitted_write(&service, async move {
        let outcome = context
            .delete_session_authorized(namespace, request, input, move || {
                authorize(authorization, identity, vault)
            })
            .await?;
        let status = if matches!(outcome, DeleteSessionOutcome::Conflict { .. }) {
            StatusCode::CONFLICT
        } else {
            StatusCode::OK
        };
        Ok((status, [("cache-control", "no-store")], Json(outcome)).into_response())
    })
    .await
}
async fn restore(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<RestoreSessionRequest>,
) -> Result<Response> {
    let request = request_id(&headers)?.to_owned();
    let access = service.catalog.access(&identity, &vault, true).await?;
    let namespace = access.member_id.clone();
    let context = service.vault(&vault).await?;
    let authorization = service.clone();
    drop(access);
    admitted_write(&service, async move {
        let outcome = context
            .restore_session_authorized(namespace, request, input, move || {
                authorize(authorization, identity, vault)
            })
            .await?;
        let status = if matches!(outcome, RestoreSessionOutcome::Conflict { .. }) {
            StatusCode::CONFLICT
        } else {
            StatusCode::OK
        };
        Ok((status, [("cache-control", "no-store")], Json(outcome)).into_response())
    })
    .await
}

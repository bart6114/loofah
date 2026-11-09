use super::*;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route(
            "/vaults/{vault}/captures/{capture}/processing",
            get(status).put(submit),
        )
        .route(
            "/vaults/{vault}/captures/{capture}/processing/cancel",
            axum::routing::post(cancel),
        )
}
async fn submit(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture)): ApiPath<(String, String)>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<CaptureProcessingRequest>,
) -> Result<Json<CaptureProcessingSnapshot>> {
    validate_capture_processing_request(&input)
        .map_err(|message| ApiError::new(ErrorCode::Validation, message))?;
    let request = request_id(&headers)?.to_owned();
    let (context, member) = capture::context(&service, &identity, &vault, true).await?;
    let authorize = capture::authorization(service.clone(), identity, vault, true);
    admitted_write(&service, async move {
        Ok(Json(
            context
                .submit_capture_processing_authorized(member, capture, request, input, authorize)
                .await?,
        ))
    })
    .await
}
async fn status(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture)): ApiPath<(String, String)>,
) -> Result<Response> {
    let (context, member) = capture::context(&service, &identity, &vault, false).await?;
    let authorize = capture::authorization(service.clone(), identity, vault, false);
    capture_live::read_response(&service, CAPTURE_PROCESSING_RESPONSE_LIMIT, async move {
        context
            .capture_processing_status_authorized(member, capture, authorize)
            .await
    })
    .await
}
async fn cancel(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture)): ApiPath<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<CaptureProcessingSnapshot>> {
    request_id(&headers)?;
    let (context, member) = capture::context(&service, &identity, &vault, true).await?;
    let authorize = capture::authorization(service.clone(), identity, vault, true);
    admitted_write(&service, async move {
        Ok(Json(
            context
                .cancel_capture_processing_authorized(member, capture, authorize)
                .await?,
        ))
    })
    .await
}

use super::*;
use futures_util::{StreamExt, TryStreamExt};

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route("/vaults/{vault}/media/source/{session}", get(source))
        .route("/vaults/{vault}/media/requests", get(list).post(declare))
        .route("/vaults/{vault}/media/requests/{request}", get(status))
        .route(
            "/vaults/{vault}/media/requests/{request}/result",
            get(result),
        )
        .route(
            "/vaults/{vault}/media/requests/{request}/summary",
            get(summary_status),
        )
        .route(
            "/vaults/{vault}/media/requests/{request}/cancel",
            axum::routing::post(cancel),
        )
        .route(
            "/vaults/{vault}/media/requests/{request}/recover",
            axum::routing::post(recover),
        )
}
pub(super) fn upload_router(service: Arc<Service>) -> Router<Arc<Service>> {
    Router::new()
        .route(
            "/vaults/{vault}/media/requests/{request}/source",
            axum::routing::put(upload),
        )
        .route_layer(middleware::from_fn_with_state(service, authorize_upload))
}
async fn declare(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<MediaDeclaration>,
) -> Result<Json<MediaAdmission>> {
    validate_media_declaration(&input)
        .map_err(|message| ApiError::new(ErrorCode::Validation, message))?;
    if request_id(&headers)? != input.request_id {
        return Err(ApiError::new(
            ErrorCode::Validation,
            "Media request identity mismatch",
        ));
    }
    let access = service.catalog.access(&identity, &vault, true).await?;
    if let Some(options) = &input.transcription {
        let policy = service.transcription_policy();
        if !policy.batch_enabled || policy.model.as_deref() != Some(options.model.as_str()) {
            return Err(ApiError::new(
                ErrorCode::Validation,
                "Select the configured server transcription model",
            ));
        }
    }
    let dispatcher = service.media_dispatcher(&vault, &access.member_id).await?;
    drop(access);
    admitted_write(&service, async move {
        dispatcher
            .declare(input)
            .await
            .map(Json)
            .map_err(Into::into)
    })
    .await
}
async fn source(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, true).await?;
    let dispatcher = service.media_dispatcher(&vault, &access.member_id).await?;
    content_read(&service, access, 1024 * 1024, async move {
        dispatcher.source(&session).await
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListInput {
    session_id: Option<String>,
}
async fn list(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    ApiQuery(input): ApiQuery<ListInput>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, true).await?;
    let dispatcher = service.media_dispatcher(&vault, &access.member_id).await?;
    content_read(&service, access, 8 * 1024 * 1024, async move {
        dispatcher.list(input.session_id.as_deref()).await
    })
    .await
}
async fn status(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, request)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, true).await?;
    let dispatcher = service.media_dispatcher(&vault, &access.member_id).await?;
    content_read(&service, access, 1024 * 1024, async move {
        dispatcher.status(&request).await
    })
    .await
}
async fn result(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, request)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, true).await?;
    let dispatcher = service.media_dispatcher(&vault, &access.member_id).await?;
    content_read(
        &service,
        access,
        TRANSCRIPTION_RESULT_RESPONSE_LIMIT,
        async move { dispatcher.result(&request).await },
    )
    .await
}
async fn cancel(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, request)): ApiPath<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<MediaRequestStatus>> {
    request_id(&headers)?;
    let access = service.catalog.access(&identity, &vault, true).await?;
    let dispatcher = service.media_dispatcher(&vault, &access.member_id).await?;
    drop(access);
    admitted_write(&service, async move {
        dispatcher
            .cancel(&request)
            .await
            .map(Json)
            .map_err(Into::into)
    })
    .await
}
async fn recover(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, request)): ApiPath<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<MediaRequestStatus>> {
    request_id(&headers)?;
    let access = service.catalog.access(&identity, &vault, true).await?;
    let dispatcher = service.media_dispatcher(&vault, &access.member_id).await?;
    drop(access);
    admitted_write(&service, async move {
        dispatcher
            .recover(&request)
            .await
            .map(Json)
            .map_err(Into::into)
    })
    .await
}
async fn upload(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, request)): ApiPath<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<MediaRequestStatus>> {
    request_id(&headers)?;
    if headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/octet-stream")
        || headers
            .get(axum::http::header::CONTENT_ENCODING)
            .is_some_and(|value| value != "identity")
    {
        return Err(ApiError::new(
            ErrorCode::Validation,
            "Use an unencoded octet-stream media source",
        ));
    }
    let revision = headers
        .get("X-Loofah-Media-Revision")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128)
        .ok_or_else(|| ApiError::new(ErrorCode::Validation, "Media admission revision required"))?
        .to_owned();
    let access = service.catalog.access(&identity, &vault, true).await?;
    let dispatcher = service.media_dispatcher(&vault, &access.member_id).await?;
    drop(access);
    let original = dispatcher.status(&request).await?;
    let expected = original
        .declaration
        .operation
        .upload()
        .map(|(_, source)| source.size_bytes)
        .ok_or_else(|| {
            ApiError::new(ErrorCode::Validation, "Media request has no source upload")
        })?;
    if headers
        .get(axum::http::header::CONTENT_LENGTH)
        .is_some_and(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                != Some(expected)
        })
    {
        return Err(ApiError::new(
            ErrorCode::Validation,
            "Media upload length differs from its declaration",
        ));
    }
    let permit = service
        .attachment_uploads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("Media transfers are busy"))?;
    let mut stream = body.into_data_stream();
    let stream = async_stream::try_stream! {
        let mut size = 0_u64;
        while let Some(chunk) = tokio::time::timeout(Duration::from_secs(35), stream.next()).await
            .map_err(|_| std::io::Error::other("Media source idle timeout"))? {
            let chunk = chunk.map_err(|_| std::io::Error::other("Media source incomplete"))?;
            size = size.checked_add(chunk.len() as u64).filter(|size| *size <= expected)
                .ok_or_else(|| std::io::Error::other("Media source too large"))?;
            for start in (0..chunk.len()).step_by(64 * 1024) { yield chunk.slice(start..(start + 64 * 1024).min(chunk.len())); }
        }
    };
    let stream = Box::pin(stream.map_err(|cause: std::io::Error| cause));
    let reader = tokio_util::io::StreamReader::new(stream);
    admitted_write(&service, async move {
        let _permit = permit;
        dispatcher
            .receive(request, revision, reader)
            .await
            .map(Json)
            .map_err(Into::into)
    })
    .await
}

async fn summary_status(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, request)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, true).await?;
    let dispatcher = service.media_dispatcher(&vault, &access.member_id).await?;
    content_read(&service, access, 1024 * 1024, async move {
        dispatcher.summary_status(&request).await
    })
    .await
}

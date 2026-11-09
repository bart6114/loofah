use super::*;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route("/vaults/{vault}/jobs", get(list).post(submit))
        .route("/vaults/{vault}/jobs/{job}", get(status))
        .route("/vaults/{vault}/jobs/{job}/result", get(result))
        .route("/vaults/{vault}/jobs/{job}/preview", get(preview))
        .route(
            "/vaults/{vault}/jobs/{job}/cancel",
            axum::routing::post(cancel),
        )
}

async fn submit(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<SummaryJobRequest>,
) -> Result<Json<ProcessingJobSnapshot>> {
    let access = service.catalog.access(&identity, &vault, true).await?;
    let context = service.vault(&vault).await?;
    let namespace = access.member_id.clone();
    let request_id = request_id(&headers)?.to_owned();
    write(&service, access, async move {
        Ok(Json(
            context
                .submit_summary_job(namespace, request_id, input)
                .await?,
        ))
    })
    .await
}

async fn list(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    ApiQuery(input): ApiQuery<ListProcessingJobsInput>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    let namespace = access.member_id.clone();
    content_read(&service, access, 1024 * 1024, async move {
        context.list_processing_jobs(namespace, input).await
    })
    .await
}

async fn status(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, job)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    let namespace = access.member_id.clone();
    content_read(&service, access, 1024 * 1024, async move {
        context.processing_job(namespace, job).await
    })
    .await
}

async fn result(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, job)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    let namespace = access.member_id.clone();
    content_read(&service, access, 32 * 1024 * 1024, async move {
        context.processing_job_result(namespace, job).await
    })
    .await
}

async fn preview(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, job)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    let namespace = access.member_id.clone();
    let mut response = content_read(&service, access, 128 * 1024, async move {
        context.processing_job_preview(namespace, job).await
    })
    .await?;
    response.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

async fn cancel(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, job)): ApiPath<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<ProcessingJobSnapshot>> {
    request_id(&headers)?;
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    let namespace = access.member_id.clone();
    write(&service, access, async move {
        Ok(Json(context.cancel_processing_job(namespace, job).await?))
    })
    .await
}

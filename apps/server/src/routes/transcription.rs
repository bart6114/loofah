use super::*;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route("/vaults/{vault}/transcription/jobs", get(list).post(submit))
        .route("/vaults/{vault}/transcription/jobs/{job}", get(status))
        .route(
            "/vaults/{vault}/transcription/jobs/{job}/result",
            get(result),
        )
        .route(
            "/vaults/{vault}/transcription/jobs/{job}/cancel",
            axum::routing::post(cancel),
        )
        .route(
            "/vaults/{vault}/transcription/jobs/{job}/resume",
            axum::routing::post(resume),
        )
}

async fn submit(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<TranscriptionJobRequest>,
) -> Result<Json<TranscriptionJobSnapshot>> {
    validate_transcription_job_request(&input)
        .map_err(|message| ApiError::new(ErrorCode::Validation, message))?;
    let request_id = request_id(&headers)?.to_owned();
    let access = service.catalog.access(&identity, &vault, true).await?;
    let context = service.vault(&vault).await?;
    let member = access.member_id.clone();
    write(&service, access, async move {
        Ok(Json(
            context
                .submit_transcription_job(member, request_id, input)
                .await?,
        ))
    })
    .await
}

async fn list(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    ApiQuery(input): ApiQuery<ListTranscriptionJobsInput>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    let member = access.member_id.clone();
    content_read(&service, access, 1024 * 1024, async move {
        context.list_transcription_jobs(member, input).await
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
    let member = access.member_id.clone();
    content_read(&service, access, TRANSCRIPTION_SNAPSHOT_LIMIT, async move {
        context.transcription_job(member, job).await
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
    let member = access.member_id.clone();
    content_read(
        &service,
        access,
        TRANSCRIPTION_RESULT_RESPONSE_LIMIT,
        async move { context.transcription_job_result(member, job).await },
    )
    .await
}

async fn cancel(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, job)): ApiPath<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<TranscriptionJobSnapshot>> {
    request_id(&headers)?;
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    let member = access.member_id.clone();
    write(&service, access, async move {
        Ok(Json(context.cancel_transcription_job(member, job).await?))
    })
    .await
}

async fn resume(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, job)): ApiPath<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<TranscriptionJobSnapshot>> {
    request_id(&headers)?;
    let access = service.catalog.access(&identity, &vault, true).await?;
    let context = service.vault(&vault).await?;
    let member = access.member_id.clone();
    write(&service, access, async move {
        Ok(Json(
            context.resume_transcription_result(member, job).await?,
        ))
    })
    .await
}

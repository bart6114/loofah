use super::*;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route("/vaults/{vault}/processing/policy", get(policy))
        .route("/vaults/{vault}/processing/settings", get(read).put(save))
}

async fn policy(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    content_read(&service, access, 4 * 1024 * 1024, async move {
        context.processing_policy()
    })
    .await
}

async fn read(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    content_read(&service, access, 1024 * 1024, async move {
        context.read_processing_settings().await
    })
    .await
}

async fn save(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<SaveProcessingSettingsRequest>,
) -> Result<Json<ProcessingSettingsSaveOutcome>> {
    request_id(&headers)?;
    let access = service.catalog.access(&identity, &vault, true).await?;
    let context = service.vault(&vault).await?;
    write(&service, access, async move {
        Ok(Json(context.save_processing_settings(input).await?))
    })
    .await
}

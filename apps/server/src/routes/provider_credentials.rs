use super::*;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new().route(
        "/vaults/{vault}/processing/providers/{provider}/credential",
        get(read).put(save),
    )
}

async fn read(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, provider)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, true).await?;
    require_owner(&access)?;
    let context = service.vault(&vault).await?;
    content_read(&service, access, 16 * 1024, async move {
        context.provider_credentials()?.status(provider).await
    })
    .await
}

async fn save(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, provider)): ApiPath<(String, String)>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<SaveProviderCredentialRequest>,
) -> Result<Json<ProviderCredentialSaveOutcome>> {
    request_id(&headers)?;
    let access = service.catalog.access(&identity, &vault, true).await?;
    require_owner(&access)?;
    let context = service.vault(&vault).await?;
    write(&service, access, async move {
        Ok(Json(
            context
                .provider_credentials()?
                .save(provider, input)
                .await?,
        ))
    })
    .await
}

fn require_owner(access: &VaultAccess) -> Result<()> {
    if access.info.role != VaultRole::Owner {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "Only vault owners can manage provider credentials",
        ));
    }
    Ok(())
}

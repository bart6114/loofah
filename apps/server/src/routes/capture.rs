use super::*;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route(
            "/vaults/{vault}/capture-append-admissions",
            axum::routing::post(prepare_append),
        )
        .route("/vaults/{vault}/captures", axum::routing::post(begin))
        .route("/vaults/{vault}/captures/{capture}", get(status))
        .route(
            "/vaults/{vault}/captures/{capture}/retirement",
            axum::routing::post(retire),
        )
        .route(
            "/vaults/{vault}/captures/{capture}/chunks/{sequence}",
            axum::routing::put(append),
        )
        .route(
            "/vaults/{vault}/captures/{capture}/seal",
            axum::routing::post(seal),
        )
        .route(
            "/vaults/{vault}/captures/{capture}/publish",
            axum::routing::post(publish),
        )
}

async fn begin(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<CaptureIngestRequest>,
) -> Result<Json<CaptureIngestStatus>> {
    if request_id(&headers)? != input.identity.request_id {
        return Err(rejected(
            ErrorCode::Validation,
            "Request ID must match the capture declaration",
        ));
    }
    let (context, member) = context(&service, &identity, &vault, true).await?;
    let authorize = authorization(service.clone(), identity, vault, true);
    admitted_write(&service, async move {
        Ok(Json(
            context
                .begin_capture_authorized(member, input, authorize)
                .await?,
        ))
    })
    .await
}

async fn status(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture)): ApiPath<(String, String)>,
) -> Result<Json<CaptureIngestStatus>> {
    let (context, member) = context(&service, &identity, &vault, false).await?;
    let authorize = authorization(service.clone(), identity, vault, false);
    admitted_write(&service, async move {
        Ok(Json(
            context
                .capture_status_authorized(member, capture, authorize)
                .await?,
        ))
    })
    .await
}

async fn append(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture, sequence)): ApiPath<(String, String, u64)>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<CapturePrefixReceipt>> {
    request_id(&headers)?;
    if headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        != Some("application/octet-stream")
        || headers
            .get("content-encoding")
            .is_some_and(|value| value != "identity")
    {
        return Err(rejected(
            ErrorCode::Validation,
            "Use an unencoded octet-stream capture chunk",
        ));
    }
    let declarations: Vec<_> = headers.get_all("x-capture-chunk").iter().collect();
    let [raw] = declarations.as_slice() else {
        return Err(rejected(
            ErrorCode::Validation,
            "One capture chunk declaration is required",
        ));
    };
    if raw.as_bytes().len() > 2048 {
        return Err(rejected(
            ErrorCode::Validation,
            "Capture chunk declaration exceeds its size limit",
        ));
    }
    let descriptor: CaptureChunkDescriptor = serde_json::from_slice(raw.as_bytes())
        .map_err(|_| rejected(ErrorCode::Validation, "Invalid capture chunk declaration"))?;
    if descriptor.sequence != sequence || descriptor.bytes == 0 || descriptor.bytes > 1024 * 1024 {
        return Err(rejected(
            ErrorCode::Validation,
            "Capture chunk declaration does not match this request",
        ));
    }
    if let Some(length) = headers.get("content-length")
        && length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            != Some(descriptor.bytes)
    {
        return Err(rejected(
            ErrorCode::Validation,
            "Capture length does not match its declaration",
        ));
    }
    let (context, member) = context(&service, &identity, &vault, true).await?;
    let authorize = authorization(service.clone(), identity, vault, true);
    admitted_write(&service, async move {
        let bytes = tokio::time::timeout(
            Duration::from_secs(35),
            axum::body::to_bytes(body, 1024 * 1024),
        )
        .await
        .map_err(|_| rejected(ErrorCode::Validation, "Capture chunk transfer timed out"))?
        .map_err(|_| {
            rejected(
                ErrorCode::Validation,
                "Capture chunk exceeds its size limit or is incomplete",
            )
        })?;
        Ok(Json(
            context
                .append_capture_authorized(member, capture, descriptor, bytes.to_vec(), authorize)
                .await?,
        ))
    })
    .await
}

async fn seal(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture)): ApiPath<(String, String)>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<CaptureSeal>,
) -> Result<Json<CaptureIngestStatus>> {
    request_id(&headers)?;
    let (context, member) = context(&service, &identity, &vault, true).await?;
    let authorize = authorization(service.clone(), identity, vault, true);
    admitted_write(&service, async move {
        Ok(Json(
            context
                .seal_capture_authorized(member, capture, input, authorize)
                .await?,
        ))
    })
    .await
}

async fn publish(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture)): ApiPath<(String, String)>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<CaptureSeal>,
) -> Result<Json<CaptureIngestStatus>> {
    request_id(&headers)?;
    let (context, member) = context(&service, &identity, &vault, true).await?;
    let authorize = authorization(service.clone(), identity, vault, true);
    admitted_write(&service, async move {
        Ok(Json(
            context
                .publish_capture_authorized(member, capture, input, authorize)
                .await?,
        ))
    })
    .await
}

pub(super) async fn context(
    service: &Service,
    identity: &Identity,
    vault: &str,
    write: bool,
) -> Result<(Arc<hypr_loofah_core::VaultContext>, String)> {
    let access = service.catalog.access(identity, vault, write).await?;
    let member = access.member_id.clone();
    drop(access);
    Ok((service.vault(vault).await?, member))
}

pub(super) fn authorization(
    service: Arc<Service>,
    identity: Identity,
    vault: String,
    write: bool,
) -> impl Fn() -> std::pin::Pin<
    Box<dyn Future<Output = std::result::Result<VaultAccess, ApplicationError>> + Send>,
> + Send {
    move || {
        let service = service.clone();
        let identity = identity.clone();
        let vault = vault.clone();
        Box::pin(async move {
            if service.stop.is_cancelled() {
                return Err(rejected(ErrorCode::Unavailable, "Server is shutting down").0);
            }
            if identity.expires_at <= now() {
                return Err(rejected(ErrorCode::Unauthenticated, "Sign in again").0);
            }
            service
                .catalog
                .access(&identity, &vault, write)
                .await
                .map_err(|cause| cause.0)
        })
    }
}

fn rejected(code: ErrorCode, message: &str) -> ApiError {
    let mut cause = ApiError::new(code, message);
    cause.0.mutation_outcome = Some(MutationOutcome::NotCommitted);
    cause
}

async fn prepare_append(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<CaptureAppendRequest>,
) -> Result<Json<CaptureAppendAdmission>> {
    if request_id(&headers)? != input.request_id || input.scope.vault_id != vault {
        return Err(rejected(
            ErrorCode::Validation,
            "Append request does not match this request scope",
        ));
    }
    let (context, member) = context(&service, &identity, &vault, true).await?;
    let authorize = authorization(service.clone(), identity, vault, true);
    admitted_write(&service, async move {
        Ok(Json(
            context
                .prepare_capture_append_authorized(member, input, authorize)
                .await?,
        ))
    })
    .await
}

async fn retire(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture)): ApiPath<(String, String)>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<CaptureRetirementRequest>,
) -> Result<Json<CaptureIngestStatus>> {
    if request_id(&headers)? != input.cleanup_request_id {
        return Err(rejected(
            ErrorCode::Validation,
            "Request ID must match the cleanup declaration",
        ));
    }
    let (context, member) = context(&service, &identity, &vault, true).await?;
    let authorize = authorization(service.clone(), identity, vault, true);
    admitted_write(&service, async move {
        Ok(Json(
            context
                .retire_capture_authorized(member, capture, input, authorize)
                .await?,
        ))
    })
    .await
}

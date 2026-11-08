use super::*;
use hypr_loofah_client::{CaptureAppendAuthority, CaptureAppendPreparation};
use hypr_loofah_protocol::CaptureAppendRequest;

pub(super) struct AppendIntent {
    request: CaptureAppendRequest,
    client: Weak<RemoteClient>,
    window: String,
    lifetime: CancellationToken,
    authority: OnceCell<Arc<CaptureAppendAuthority>>,
    prepared: OnceCell<CaptureAppendPreparation>,
}

impl RemoteCaptureCoordinator {
    pub async fn pending_append_requests(
        &self,
        profile: Arc<ProfileLease>,
        scope: CaptureScope,
    ) -> Result<Vec<CaptureAppendRequest>, ApplicationError> {
        let generation = profile.generation().await?;
        let (id, current) = profile_scope(&profile).await?;
        if current != scope {
            return Err(error(
                ErrorCode::Conflict,
                "The original append destination changed",
            ));
        }
        let host = self.inner.host(&id).await?;
        let captures = host.spool.inventory(scope.clone()).await?;
        let requests = CaptureAppendPreparation::inventory(self.append_root(&id), scope).await?;
        drop(profile.guard_generation(&generation).await?);
        Ok(requests
            .into_iter()
            .filter(|request| {
                !captures.iter().any(|capture| {
                    capture.identity.capture_id == request.capture_id
                        && matches!(&capture.identity.target, CaptureTarget::AppendSession { admission, .. } if admission.request == *request)
                        && capture.publication.is_some()
                })
            })
            .collect())
    }

    fn append_root(&self, profile_id: &str) -> PathBuf {
        self.inner.root.join("append-preparations").join(profile_id)
    }

    pub async fn start_append(
        self: &Arc<Self>,
        profile: Arc<ProfileLease>,
        client: Arc<RemoteClient>,
        request: CaptureAppendRequest,
        window: String,
        lifetime: CancellationToken,
    ) -> Result<RemoteDeviceCapture, ApplicationError> {
        hypr_loofah_protocol::validate_capture_append_request(&request)
            .map_err(|message| error(ErrorCode::Validation, message))?;
        let (id, scope) = profile_scope(&profile).await?;
        if scope != request.scope || lifetime.is_cancelled() {
            return Err(error(
                ErrorCode::Conflict,
                "The original append owner changed",
            ));
        }
        validate_client(&scope, &client)?;
        let permit = self.inner.starts.clone().try_acquire_owned().map_err(|_| {
            error(
                ErrorCode::ResourceLimit,
                "Too many device recordings are being prepared",
            )
        })?;
        let intent = {
            let mut state = self.inner.state.lock().unwrap();
            if state.closing {
                return Err(error(ErrorCode::Unavailable, "Device capture is stopping"));
            }
            let key = (id.clone(), request.request_id.clone());
            if let Some(original) = state.append.get(&key) {
                if original.request != request
                    || original.window != window
                    || !Weak::ptr_eq(&original.client, &Arc::downgrade(&client))
                    || original.lifetime.is_cancelled()
                {
                    return Err(error(
                        ErrorCode::Conflict,
                        "Append retries require the original window, profile, client and declaration",
                    ));
                }
                original.clone()
            } else {
                if state.append.len() >= 64 {
                    return Err(error(
                        ErrorCode::ResourceLimit,
                        "Too many retained append preparations",
                    ));
                }
                let original = Arc::new(AppendIntent {
                    request: request.clone(),
                    client: Arc::downgrade(&client),
                    window,
                    lifetime,
                    authority: OnceCell::new(),
                    prepared: OnceCell::new(),
                });
                state.append.insert(key, original.clone());
                original
            }
        };
        let this = self.clone();
        tokio::spawn(async move {
            let mut permit = Some(permit);
            let work = async {
                let authority = intent.authority.get_or_try_init(|| async {
                    client.bind_capture_append(profile.clone(), request.clone(), intent.lifetime.clone()).await
                }).await?.clone();
                drop(authority.guard_dispatch().await?);
                let prepared = intent.prepared.get_or_try_init(|| async {
                    let _serial = this.inner.start_serial.lock().await;
                    let pending = this.pending_append_requests(profile.clone(), scope.clone()).await?;
                    if pending.iter().any(|original| original.session_id == request.session_id && original != &request) {
                        return Err(error(ErrorCode::Conflict, "Recover the original saved append request before preparing another recording"));
                    }
                    CaptureAppendPreparation::prepare(this.append_root(&id), request.clone()).await
                }).await?;
                authority.check_current().await?;
                let admission = client.prepare_capture_append(prepared, intent.lifetime.clone()).await?;
                authority.check_current().await?;
                let format = admission.baseline.format.clone();
                this.start_authorized(profile, Some(client), DeviceCaptureStartRequest {
                    request_id: request.request_id,
                    scope,
                    target: CaptureTarget::AppendSession { session_id: request.session_id, admission },
                    format,
                    processing: request.processing,
                }, Some(authority), permit.take()).await
            };
            tokio::pin!(work);
            tokio::select! {
                result = &mut work => result,
                _ = this.inner.retirement_stop.cancelled() => {
                    intent.lifetime.cancel();
                    Err(error(ErrorCode::Unavailable, "Device capture is stopping; the original append request was retained"))
                }
            }
        }).await.map_err(|_| error(ErrorCode::Unavailable, "Append preparation stopped; retain its original request"))?
    }
}

use super::*;

const PAGE_RESPONSE_LIMIT: usize = 2 * CAPTURE_LIVE_PAGE_LIMIT;
const STREAM_PAGE_SIZE: u32 = 8;

pub(super) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route(
            "/vaults/{vault}/captures/{capture}/live",
            axum::routing::post(start),
        )
        .route(
            "/vaults/{vault}/captures/{capture}/live/{generation}",
            get(status),
        )
        .route(
            "/vaults/{vault}/captures/{capture}/live/{generation}/captions",
            get(captions),
        )
        .route(
            "/vaults/{vault}/captures/{capture}/live/{generation}/result",
            get(result),
        )
        .route(
            "/vaults/{vault}/captures/{capture}/live/{generation}/cancel",
            axum::routing::post(cancel),
        )
        .route(
            "/vaults/{vault}/captures/{capture}/live/{generation}/events",
            get(events),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptionQuery {
    after: Option<u64>,
    #[serde(default = "page_size")]
    limit: u32,
}
fn page_size() -> u32 {
    128
}

async fn start(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture)): ApiPath<(String, String)>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<CaptureLiveRequest>,
) -> Result<Json<CaptureLiveSnapshot>> {
    validate_capture_live_request(&input)
        .map_err(|message| ApiError::new(ErrorCode::Validation, message))?;
    let request = request_id(&headers)?.to_owned();
    let (context, member) = capture::context(&service, &identity, &vault, true).await?;
    let authorize = capture::authorization(service.clone(), identity, vault, true);
    admitted_write(&service, async move {
        Ok(Json(
            context
                .start_capture_live_authorized(member, capture, request, input, authorize)
                .await?,
        ))
    })
    .await
}

async fn status(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture, generation)): ApiPath<(String, String, String)>,
) -> Result<Response> {
    let (context, member) = capture::context(&service, &identity, &vault, false).await?;
    let authorize = capture::authorization(service.clone(), identity, vault, false);
    read_response(&service, 64 * 1024, async move {
        context
            .capture_live_status_authorized(member, capture, generation, authorize)
            .await
    })
    .await
}

async fn captions(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture, generation)): ApiPath<(String, String, String)>,
    ApiQuery(input): ApiQuery<CaptionQuery>,
) -> Result<Response> {
    validate_page_query(input.after, input.limit)?;
    let (context, member) = capture::context(&service, &identity, &vault, false).await?;
    let authorize = capture::authorization(service.clone(), identity, vault, false);
    read_response(&service, PAGE_RESPONSE_LIMIT, async move {
        context
            .capture_live_captions_authorized(
                member,
                capture,
                generation,
                input.after,
                input.limit,
                authorize,
            )
            .await
    })
    .await
}

async fn result(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture, generation)): ApiPath<(String, String, String)>,
) -> Result<Response> {
    let (context, member) = capture::context(&service, &identity, &vault, false).await?;
    let authorize = capture::authorization(service.clone(), identity, vault, false);
    read_response(&service, TRANSCRIPTION_RESULT_RESPONSE_LIMIT, async move {
        context
            .capture_live_result_authorized(member, capture, generation, authorize)
            .await
    })
    .await
}

async fn cancel(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture, generation)): ApiPath<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<CaptureLiveSnapshot>> {
    request_id(&headers)?;
    let (context, member) = capture::context(&service, &identity, &vault, true).await?;
    let authorize = capture::authorization(service.clone(), identity, vault, true);
    admitted_write(&service, async move {
        Ok(Json(
            context
                .cancel_capture_live_authorized(member, capture, generation, authorize)
                .await?,
        ))
    })
    .await
}

fn validate_page_query(after: Option<u64>, limit: u32) -> Result<()> {
    if !(1..=128).contains(&limit) || after.is_some_and(|sequence| sequence > 9_007_199_254_740_991)
    {
        return Err(ApiError::new(
            ErrorCode::Validation,
            "Invalid live caption page",
        ));
    }
    Ok(())
}

// A fresh Catalog grant is acquired inside Core before its journal lock. Keeping
// an older grant here while reacquiring could deadlock behind a queued writer.
async fn owned_read<T: Send + 'static>(
    service: &Service,
    operation: impl Future<Output = std::result::Result<T, ApplicationError>> + Send + 'static,
) -> Result<T> {
    if service.stop.is_cancelled() {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "Server is shutting down",
        ));
    }
    let permit = service
        .content_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("Content read capacity reached; retry later"))?;
    if service.stop.is_cancelled() {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "Server is shutting down",
        ));
    }
    tokio::spawn(async move {
        let _permit = permit;
        operation.await.map_err(ApiError::from)
    })
    .await
    .map_err(ApiError::internal)?
}

pub(super) async fn read_response<T: serde::Serialize + Send + 'static>(
    service: &Service,
    limit: usize,
    operation: impl Future<Output = std::result::Result<T, ApplicationError>> + Send + 'static,
) -> Result<Response> {
    owned_read(service, async move {
        let value = operation.await?;
        tokio::task::spawn_blocking(move || {
            encode_content(&value, limit)
                .map(|bytes| ([("content-type", "application/json")], bytes).into_response())
                .map_err(|cause| cause.0)
        })
        .await
        .map_err(|cause| ApiError::internal(cause).0)?
    })
    .await
}

fn event_cursor(generation: &str, after: Option<u64>) -> String {
    format!(
        "{generation}:{}",
        after.map_or_else(|| "initial".into(), |sequence| sequence.to_string())
    )
}
fn parse_cursor(value: Option<&str>, generation: &str) -> Result<Option<u64>> {
    let Some(value) = value else { return Ok(None) };
    if value.len() > 96 {
        return Err(ApiError::new(
            ErrorCode::Validation,
            "Invalid live caption cursor",
        ));
    }
    let Some((scope, sequence)) = value.rsplit_once(':') else {
        return Err(ApiError::new(
            ErrorCode::Validation,
            "Invalid live caption cursor",
        ));
    };
    if scope != generation {
        return Err(ApiError::new(
            ErrorCode::Validation,
            "Live cursor belongs to another generation",
        ));
    }
    if sequence == "initial" {
        return Ok(None);
    };
    let sequence = sequence
        .parse::<u64>()
        .map_err(|_| ApiError::new(ErrorCode::Validation, "Invalid live caption cursor"))?;
    validate_page_query(Some(sequence), STREAM_PAGE_SIZE)?;
    Ok(Some(sequence))
}

async fn events(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, capture, generation)): ApiPath<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_util::Stream<Item = std::result::Result<Event, Infallible>>>> {
    let (context, member) = capture::context(&service, &identity, &vault, false).await?;
    let raw_cursor = headers
        .get("last-event-id")
        .map(|value| {
            value
                .to_str()
                .map_err(|_| ApiError::new(ErrorCode::Validation, "Invalid live caption cursor"))
        })
        .transpose()?;
    let mut after = parse_cursor(raw_cursor, &generation)?;
    let mut changes = context.events.subscribe_for(&member, None);
    let mut access_changes = service.catalog.subscribe();
    let initial_context = context.clone();
    let initial_member = member.clone();
    let initial_capture = capture.clone();
    let initial_generation = generation.clone();
    let authorize = capture::authorization(service.clone(), identity.clone(), vault.clone(), false);
    let first = owned_read(&service, async move {
        initial_context
            .capture_live_captions_authorized(
                initial_member,
                initial_capture,
                initial_generation,
                after,
                STREAM_PAGE_SIZE,
                authorize,
            )
            .await
    })
    .await?;
    let permit = service
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("Subscription capacity reached"))?;
    if service.stop.is_cancelled() {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "Server is shutting down",
        ));
    }
    let stream = async_stream::stream! {
        let _permit=permit;
        let expiry=tokio::time::sleep(Duration::from_secs(identity.expires_at.saturating_sub(now())));tokio::pin!(expiry);
        let mut reconcile=tokio::time::interval(Duration::from_secs(5));reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);reconcile.tick().await;
        let mut page=Some(first);let mut previous_snapshot=None;
        'delivery: loop {
            if service.stop.is_cancelled()||identity.expires_at<=now(){break;}
            if let Some(current)=page.take() {
                let progress=current.captions.last().map(|caption|caption.sequence);
                if let Some(sequence)=progress {after=Some(sequence);}
                let deliver=progress.is_some()||previous_snapshot.as_ref()!=Some(&current.snapshot);
                previous_snapshot=Some(current.snapshot.clone());
                let more=current.next_cursor.is_some();let terminal=current.snapshot.state.is_terminal();
                if deliver {
                    let authorize=capture::authorization(service.clone(),identity.clone(),vault.clone(),false);
                    let access=match authorize().await {Ok(access)=>access,Err(cause)=>{if let Ok(data)=serde_json::to_string(&cause){yield Ok(Event::default().event("error").data(data));}break;}};
                    let Ok(bytes)=encode_content(&current,CAPTURE_LIVE_PAGE_LIMIT) else{break};
                    let Ok(data)=String::from_utf8(bytes) else{break};
                    drop(access);
                    yield Ok(Event::default().id(event_cursor(&generation,after)).event("page").data(data));
                }
                if terminal&&!more {break;}
                if !more {
                    loop {
                        tokio::select! {
                            _=service.stop.cancelled()=>break 'delivery,
                            _=&mut expiry=>break 'delivery,
                            _=reconcile.tick()=>break,
                            change=access_changes.changed()=>{if change.is_err(){break 'delivery;}break;},
                            event=changes.recv()=>{let Some(event)=event else{break 'delivery};if matches!(event.change,Change::JobChanged{ref job_id} if job_id==&generation)||matches!(event.change,Change::ResyncRequired){break;}},
                        }
                    }
                }
            }
            let caller=context.clone();let member=member.clone();let capture=capture.clone();let generation=generation.clone();let authorize=capture::authorization(service.clone(),identity.clone(),vault.clone(),false);
            let fetched=tokio::select! {
                _=service.stop.cancelled()=>break,
                _=&mut expiry=>break,
                value=owned_read(&service,async move{caller.capture_live_captions_authorized(member,capture,generation,after,STREAM_PAGE_SIZE,authorize).await})=>value,
            };
            match fetched {
                Ok(value)=>page=Some(value),
                Err(cause)=>{if let Ok(data)=serde_json::to_string(&cause.0){yield Ok(Event::default().event("error").data(data));}break;},
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

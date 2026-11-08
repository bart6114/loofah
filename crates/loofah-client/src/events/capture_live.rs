use super::*;
use hypr_loofah_protocol::{
    CaptureFormat, CaptureLiveCaptionPage, CaptureLiveSnapshot, CaptureLiveState, CaptureTarget,
    validate_capture_format, validate_capture_live_caption,
};

const MAX_SEQUENCE: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureLiveBinding {
    pub capture_id: String,
    pub generation_id: String,
    pub capture_fingerprint: String,
    pub session_id: String,
    pub format: CaptureFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureLiveEvent {
    Connection {
        state: ConnectionState,
        error: Option<ApplicationError>,
    },
    Page {
        page: CaptureLiveCaptionPage,
    },
}

pub(crate) fn valid_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
fn malformed() -> ApplicationError {
    failure(
        ErrorCode::IncompatibleVersion,
        "Invalid live capture projection",
    )
}
pub(crate) fn validate_binding(value: &CaptureLiveBinding) -> Result<()> {
    if !valid_uuid(&value.capture_id)
        || !valid_uuid(&value.generation_id)
        || !hypr_loofah_protocol::is_transcription_sha256(&value.capture_fingerprint)
        || hypr_loofah_protocol::validate_session_id(&value.session_id).is_err()
    {
        return Err(crate::invalid("Invalid live capture binding"));
    }
    validate_capture_format(&value.format).map_err(crate::invalid)
}
pub(crate) fn validate_snapshot(
    value: &CaptureLiveSnapshot,
    binding: &CaptureLiveBinding,
) -> Result<()> {
    if value.capture_id != binding.capture_id
        || value.generation_id != binding.generation_id
        || value.capture_fingerprint != binding.capture_fingerprint
        || value.session_id != binding.session_id
        || value.admitted_end_frame > MAX_SEQUENCE
        || value.input_consumed_end_frame > value.admitted_end_frame
        || value.next_caption_sequence > MAX_SEQUENCE
        || value
            .completed_through_frame
            .is_some_and(|frame| frame > value.input_consumed_end_frame)
        || value.publication.as_ref().is_some_and(|publication| {
            value.state != CaptureLiveState::Completed
                || !hypr_loofah_protocol::is_transcription_sha256(&publication.revision)
                || !valid_uuid(&publication.transcript_id)
        })
        || (value.state == CaptureLiveState::Completed && value.publication.is_none())
        || (matches!(
            value.state,
            CaptureLiveState::Completed | CaptureLiveState::Empty
        ) && (value.completed_through_frame != Some(value.admitted_end_frame)
            || value.input_consumed_end_frame != value.admitted_end_frame
            || !value.result_available))
        || (value.state == CaptureLiveState::Empty && value.publication.is_some())
    {
        return Err(malformed());
    }
    Ok(())
}
pub(crate) fn validate_page(
    value: &CaptureLiveCaptionPage,
    binding: &CaptureLiveBinding,
    after: Option<u64>,
    limit: u32,
) -> Result<()> {
    validate_snapshot(&value.snapshot, binding)?;
    let start = after
        .map(|value| value.checked_add(1).ok_or_else(malformed))
        .transpose()?
        .unwrap_or(0);
    let end = start
        .checked_add(value.captions.len() as u64)
        .ok_or_else(malformed)?;
    if value.captions.len() > limit as usize
        || end > value.snapshot.next_caption_sequence
        || value.next_cursor
            != (end < value.snapshot.next_caption_sequence)
                .then(|| end.checked_sub(1))
                .flatten()
        || (end < value.snapshot.next_caption_sequence && value.captions.is_empty())
    {
        return Err(malformed());
    }
    for (offset, caption) in value.captions.iter().enumerate() {
        if caption.generation_id != binding.generation_id
            || caption.sequence != start + offset as u64
        {
            return Err(malformed());
        }
        validate_capture_live_caption(caption, &binding.format.channels)
            .map_err(|_| malformed())?;
    }
    Ok(())
}
fn cursor(binding: &CaptureLiveBinding, after: Option<u64>) -> String {
    format!(
        "{}:{}",
        binding.generation_id,
        after.map_or_else(|| "initial".into(), |value| value.to_string())
    )
}
fn emit(
    listener: &(impl Fn(CaptureLiveEvent) -> bool + Send + Sync),
    state: ConnectionState,
    error: Option<ApplicationError>,
) -> bool {
    listener(CaptureLiveEvent::Connection { state, error })
}

impl RemoteClient {
    pub fn subscribe_capture_live(
        self: &Arc<Self>,
        vault_id: String,
        binding: CaptureLiveBinding,
        after: Option<u64>,
        listener: impl Fn(CaptureLiveEvent) -> bool + Send + Sync + 'static,
    ) -> Result<Subscription> {
        live_capability(self)?;
        if !valid_uuid(&vault_id) {
            return Err(crate::invalid("A canonical vault UUID is required"));
        }
        validate_binding(&binding)?;
        if after.is_some_and(|value| value > MAX_SEQUENCE) {
            return Err(crate::invalid("Invalid live caption cursor"));
        }
        let url = self.endpoint(&[
            "vaults",
            &vault_id,
            "captures",
            &binding.capture_id,
            "live",
            &binding.generation_id,
            "events",
        ])?;
        let cancel = self.stop.child_token();
        let task_cancel = cancel.clone();
        let client = self.clone();
        let task = tokio::spawn(async move {
            let mut after = after;
            let mut verified = false;
            let mut failures = 0u32;
            let mut unauthorized_retry = false;
            loop {
                if task_cancel.is_cancelled() {
                    break;
                }
                if !emit(
                    &listener,
                    if failures == 0 {
                        ConnectionState::Connecting
                    } else {
                        ConnectionState::Reconnecting
                    },
                    None,
                ) {
                    break;
                }
                let result = tokio::select! {
                    _=task_cancel.cancelled()=>break,
                    value=async {
                        if !verified {
                            let source=client.capture_status(&vault_id,&binding.capture_id,task_cancel.clone()).await?;
                            if source.prefix.capture_fingerprint!=binding.capture_fingerprint||source.format!=binding.format||!matches!(source.identity.target,CaptureTarget::ExistingSession{ref session_id,..} if session_id==&binding.session_id){return Err(malformed());}
                            verified=true;
                        }
                        client.capture_live_connection(url.clone(),&binding,&mut after,&listener,&mut failures,&task_cancel).await
                    }=>value,
                };
                let error = match result {
                    Ok(true) => break,
                    Ok(false) => unavailable(),
                    Err(cause) => cause,
                };
                if task_cancel.is_cancelled() {
                    break;
                }
                let terminal = match error.code {
                    ErrorCode::Unauthenticated if unauthorized_retry => {
                        Some(ConnectionState::LoginRequired)
                    }
                    ErrorCode::Forbidden | ErrorCode::NotFound => {
                        Some(ConnectionState::AccessDenied)
                    }
                    ErrorCode::IncompatibleVersion
                    | ErrorCode::Validation
                    | ErrorCode::Conflict => Some(ConnectionState::Incompatible),
                    ErrorCode::Cancelled => break,
                    _ => None,
                };
                if let Some(state) = terminal {
                    emit(&listener, state, Some(error));
                    return;
                }
                unauthorized_retry = error.code == ErrorCode::Unauthenticated;
                if !emit(&listener, ConnectionState::Reconnecting, Some(error)) {
                    break;
                }
                failures = failures.saturating_add(1);
                let delay = (250u64.saturating_mul(1u64 << failures.min(5))).min(10_000)
                    + u64::from(uuid::Uuid::new_v4().as_bytes()[0]);
                tokio::select! {_=task_cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(delay))=>{}}
            }
            emit(&listener, ConnectionState::Closed, None);
        });
        Ok(Subscription { cancel, task })
    }

    async fn capture_live_connection(
        self: &Arc<Self>,
        url: url::Url,
        binding: &CaptureLiveBinding,
        after: &mut Option<u64>,
        listener: &(impl Fn(CaptureLiveEvent) -> bool + Send + Sync),
        failures: &mut u32,
        cancel: &CancellationToken,
    ) -> Result<bool> {
        let access = self.access_token().await?;
        let response = tokio::time::timeout(
            Duration::from_secs(30),
            self.http
                .get(url)
                .bearer_auth(&access)
                .header("x-loofah-api-major", API_MAJOR.to_string())
                .header("accept", "text/event-stream")
                .header("last-event-id", cursor(binding, *after))
                .send(),
        )
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.expire_access(&access).await;
            return Err(login_required());
        }
        if !response.status().is_success() {
            return Err(json_body(response, 131_072)
                .await
                .unwrap_or_else(|_| unavailable()));
        }
        if response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_none_or(|value| value.split(';').next() != Some("text/event-stream"))
        {
            return Err(malformed());
        }
        if !emit(listener, ConnectionState::Connected, None) {
            return Err(cancelled());
        }
        *failures = 0;
        let mut decoder = Decoder::new();
        let mut stream = response.bytes_stream();
        loop {
            let chunk = tokio::select! {_=cancel.cancelled()=>return Err(cancelled()),value=tokio::time::timeout(Duration::from_secs(45),stream.next())=>value.map_err(|_|unavailable())?};
            let Some(chunk) = chunk else { return Ok(false) };
            for message in decoder.push(&chunk.map_err(|_| unavailable())?)? {
                if message.event == "error" {
                    return Err(serde_json::from_str::<ApplicationError>(&message.data)
                        .map_err(|_| malformed())?);
                }
                if message.event != "page" {
                    return Err(malformed());
                }
                let page: CaptureLiveCaptionPage =
                    serde_json::from_str(&message.data).map_err(|_| malformed())?;
                validate_page(&page, binding, *after, 8)?;
                let next = page
                    .captions
                    .last()
                    .map(|caption| caption.sequence)
                    .or(*after);
                if message.id.as_deref() != Some(cursor(binding, next).as_str()) {
                    return Err(malformed());
                }
                let terminal = page.snapshot.state.is_terminal() && page.next_cursor.is_none();
                if !listener(CaptureLiveEvent::Page { page }) {
                    return Err(cancelled());
                }
                *after = next;
                if terminal {
                    return Ok(true);
                }
            }
        }
    }
}

pub(crate) fn live_capability(client: &RemoteClient) -> Result<()> {
    if !client
        .capabilities
        .features
        .iter()
        .any(|feature| feature == "capture_live_transcription")
    {
        return Err(failure(
            ErrorCode::IncompatibleVersion,
            "This server does not support live capture transcription. Upgrade the server, then reconnect.",
        ));
    }
    Ok(())
}

use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use hypr_loofah_protocol::{API_MAJOR, ApplicationError, Change, ChangeEvent, ErrorCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{
    RemoteClient, Result, auth::login_required, cancelled, failure, json_body, sse::Decoder,
    unavailable,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting,
    LoginRequired,
    AccessDenied,
    Incompatible,
    Closed,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteEvent {
    Connection {
        state: ConnectionState,
        error: Option<ApplicationError>,
    },
    Change {
        event: ChangeEvent,
    },
}

pub struct Subscription {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Subscription {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
    pub async fn close(mut self) {
        self.cancel.cancel();
        if tokio::time::timeout(Duration::from_secs(1), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
        }
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl RemoteClient {
    pub fn subscribe(
        self: &Arc<Self>,
        vault_id: String,
        listener: impl Fn(RemoteEvent) -> bool + Send + Sync + 'static,
    ) -> Result<Subscription> {
        let url = self.endpoint(&["vaults", &vault_id, "events"])?;
        let cancel = self.stop.child_token();
        let task_cancel = cancel.clone();
        let client = self.clone();
        let task = tokio::spawn(async move {
            let mut cursor = None;
            let mut failures = 0_u32;
            let mut unauthorized_retry = false;
            loop {
                if task_cancel.is_cancelled() {
                    break;
                }
                if !emit_state(
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
                    _ = task_cancel.cancelled() => break,
                    result = client.event_connection(url.clone(), &vault_id, &mut cursor, &listener, &mut failures, &task_cancel) => result,
                };
                let error = match result {
                    Ok(()) => unavailable(),
                    Err(error) => error,
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
                    ErrorCode::IncompatibleVersion | ErrorCode::Validation => {
                        Some(ConnectionState::Incompatible)
                    }
                    ErrorCode::Cancelled => break,
                    _ => None,
                };
                if let Some(state) = terminal {
                    emit_state(&listener, state, Some(error));
                    return;
                }
                unauthorized_retry = error.code == ErrorCode::Unauthenticated;
                if !emit_state(&listener, ConnectionState::Reconnecting, Some(error)) {
                    break;
                }
                failures = failures.saturating_add(1);
                let delay = (250_u64.saturating_mul(1_u64 << failures.min(5))).min(10_000);
                let jitter = u64::from(uuid::Uuid::new_v4().as_bytes()[0]);
                tokio::select! {
                    _ = task_cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(delay + jitter)) => {},
                }
            }
            emit_state(&listener, ConnectionState::Closed, None);
        });
        Ok(Subscription { cancel, task })
    }

    async fn event_connection(
        self: &Arc<Self>,
        url: url::Url,
        vault_id: &str,
        cursor: &mut Option<String>,
        listener: &(impl Fn(RemoteEvent) -> bool + Send + Sync),
        failures: &mut u32,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let access = self.access_token().await?;
        let mut request = self
            .http
            .get(url)
            .bearer_auth(&access)
            .header("x-loofah-api-major", API_MAJOR.to_string())
            .header("accept", "text/event-stream");
        if let Some(cursor) = cursor {
            request = request.header("last-event-id", cursor.as_str());
        }
        let response = tokio::time::timeout(Duration::from_secs(30), request.send())
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
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "The server did not return a change stream",
            ));
        }
        if !emit_state(listener, ConnectionState::Connected, None) {
            return Err(cancelled());
        }
        *failures = 0;
        let mut decoder = Decoder::new();
        let mut stream = response.bytes_stream();
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => return Err(cancelled()),
                result = tokio::time::timeout(Duration::from_secs(45), stream.next()) => result.map_err(|_| unavailable())?,
            };
            let Some(chunk) = chunk else {
                return Ok(());
            };
            for message in decoder.push(&chunk.map_err(|_| unavailable())?)? {
                let id = message.id.filter(|id| !id.is_empty()).ok_or_else(|| {
                    failure(
                        ErrorCode::IncompatibleVersion,
                        "A change event has no cursor",
                    )
                })?;
                let mut event: ChangeEvent = serde_json::from_str(&message.data).map_err(|_| {
                    failure(
                        ErrorCode::IncompatibleVersion,
                        "Invalid server change event",
                    )
                })?;
                if event.vault_id != vault_id || event.cursor != id {
                    return Err(failure(
                        ErrorCode::IncompatibleVersion,
                        "Change event scope or cursor does not match the subscription",
                    ));
                }
                if !matches!(message.event.as_str(), "" | "message" | "change")
                    || event.change == Change::Unknown
                {
                    event.change = Change::ResyncRequired;
                }
                if !listener(RemoteEvent::Change { event }) {
                    return Err(cancelled());
                }
                *cursor = Some(id);
            }
        }
    }
}

fn emit_state(
    listener: &impl Fn(RemoteEvent) -> bool,
    state: ConnectionState,
    error: Option<ApplicationError>,
) -> bool {
    listener(RemoteEvent::Connection { state, error })
}

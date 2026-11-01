use std::convert::Infallible;

use axum::{
    Json,
    extract::ws::{Message, WebSocket},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures_util::{SinkExt, stream::SplitSink};
use owhisper_interface::batch_sse::{BatchSseMessage, EVENT_NAME, KEEP_ALIVE_INTERVAL};
use owhisper_interface::stream::StreamResponse;
use tokio::sync::mpsc;

pub type WsSender = SplitSink<WebSocket, Message>;

pub async fn send_ws(sender: &mut WsSender, value: &StreamResponse) -> bool {
    let payload = match serde_json::to_string(value) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::warn!("failed to serialize ws response: {error}");
            return false;
        }
    };

    sender.send(Message::Text(payload.into())).await.is_ok()
}

pub async fn send_ws_best_effort(sender: &mut WsSender, value: &StreamResponse) {
    let payload = match serde_json::to_string(value) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::warn!("failed to serialize ws response: {error}");
            return;
        }
    };

    let _ = sender.send(Message::Text(payload.into())).await;
}

pub fn json_error_response(status: StatusCode, error: &str, detail: impl Into<String>) -> Response {
    (
        status,
        Json(serde_json::json!({
            "error": error,
            "detail": detail.into(),
        })),
    )
        .into_response()
}

pub fn batch_sse_response(event_rx: mpsc::UnboundedReceiver<BatchSseMessage>) -> Response {
    let events_stream = futures_util::stream::unfold(event_rx, |mut rx| async move {
        rx.recv().await.map(|message| {
            let event = match Event::default().event(EVENT_NAME).json_data(&message) {
                Ok(event) => event,
                Err(error) => {
                    tracing::warn!("failed to serialize batch SSE event: {error}");
                    Event::default()
                        .event(EVENT_NAME)
                        .data(r#"{"error":"transcription_failed","detail":"failed to serialize SSE event"}"#)
                }
            };
            (Ok::<_, Infallible>(event), rx)
        })
    });

    Sse::new(events_stream)
        .keep_alive(KeepAlive::new().interval(KEEP_ALIVE_INTERVAL))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    #[tokio::test(start_paused = true)]
    async fn batch_sse_sends_keepalives_while_worker_is_busy() {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut body = batch_sse_response(rx).into_body().into_data_stream();
        for _ in 0..4 {
            let start = tokio::time::Instant::now();
            let bytes = body.next().await.unwrap().unwrap();
            assert!(bytes.starts_with(b":"));
            assert!(start.elapsed() >= KEEP_ALIVE_INTERVAL);
        }
        tx.send(BatchSseMessage::Error {
            error: "transcription_failed".into(),
            detail: "worker failed".into(),
        })
        .unwrap();
        let bytes = body.next().await.unwrap().unwrap();
        assert!(
            std::str::from_utf8(&bytes)
                .unwrap()
                .contains("worker failed")
        );
        drop(tx);
        assert!(body.next().await.is_none());
    }
}

pub fn format_timestamp_now() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();

    let mut days = total_secs / 86_400;
    let day_secs = (total_secs % 86_400) as u32;
    let hours = day_secs / 3_600;
    let minutes = (day_secs % 3_600) / 60;
    let seconds = day_secs % 60;

    let mut year = 1970i32;
    loop {
        let year_days = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
            366
        } else {
            365
        };
        if days < year_days {
            break;
        }
        days -= year_days;
        year += 1;
    }

    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u32;
    for month_len in month_days {
        if days < month_len {
            break;
        }
        days -= month_len;
        month += 1;
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year,
        month,
        days + 1,
        hours,
        minutes,
        seconds,
        millis
    )
}

use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    body::Body,
    extract::{FromRequestParts, ws::WebSocketUpgrade},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use hypr_model_manager::{ModelManager, ModelManagerBuilder};
use hypr_ws_utils::ConnectionManager;
use owhisper_interface::ListenParams;
use owhisper_interface::stream::StreamResponse;
use tower::Service;

use super::batch;
use super::response::{
    TranscriptKind, build_transcript_response, format_timestamp_now, send_ws, send_ws_best_effort,
};
use super::{build_metadata, parse_listen_params, redemption_time};

pub const LISTEN_PATH: &str = "/v1/listen";
pub const HEALTH_PATH: &str = "/health";

#[derive(Clone)]
pub struct TranscribeService {
    model_path: PathBuf,
    manager: ModelManager<hypr_whisper_local::LoadedWhisper>,
    connection_manager: ConnectionManager,
}

impl TranscribeService {
    pub fn builder() -> TranscribeServiceBuilder {
        TranscribeServiceBuilder::default()
    }

    pub fn into_router<F, Fut>(self, on_error: F) -> axum::Router
    where
        F: FnOnce(String) -> Fut + Clone + Send + Sync + 'static,
        Fut: std::future::Future<Output = (StatusCode, String)> + Send,
    {
        let service = axum::error_handling::HandleError::new(self, on_error);
        axum::Router::new()
            .route(HEALTH_PATH, axum::routing::get(|| async { "ok" }))
            .route_service(LISTEN_PATH, service)
    }
}

#[derive(Default)]
pub struct TranscribeServiceBuilder {
    model_path: Option<PathBuf>,
    connection_manager: Option<ConnectionManager>,
}

impl TranscribeServiceBuilder {
    pub fn model_path(mut self, model_path: PathBuf) -> Self {
        self.model_path = Some(model_path);
        self
    }

    pub fn build(self) -> TranscribeService {
        let model_path = self
            .model_path
            .expect("TranscribeServiceBuilder requires model_path");
        let manager = ModelManagerBuilder::default()
            .register("default", &model_path)
            .default_model("default")
            .build();

        let warmup_manager = manager.clone();
        tokio::spawn(async move {
            match warmup_manager.get(None).await {
                Ok(_) => tracing::info!("whisper_local_model_warmup_completed"),
                Err(error) => tracing::warn!(error = %error, "whisper_local_model_warmup_failed"),
            }
        });

        TranscribeService {
            model_path,
            manager,
            connection_manager: self.connection_manager.unwrap_or_default(),
        }
    }
}

impl Service<Request<Body>> for TranscribeService {
    type Response = Response;
    type Error = String;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let model_path = self.model_path.clone();
        let manager = self.manager.clone();
        let connection_manager = self.connection_manager.clone();

        Box::pin(async move {
            let is_ws = req
                .headers()
                .get("upgrade")
                .and_then(|value| value.to_str().ok())
                .map(|value| value.eq_ignore_ascii_case("websocket"))
                .unwrap_or(false);

            let params = match parse_listen_params(req.uri().query().unwrap_or("")) {
                Ok(params) => params,
                Err(error) => {
                    return Ok((StatusCode::BAD_REQUEST, error.to_string()).into_response());
                }
            };

            if is_ws {
                let model = match manager.get(None).await {
                    Ok(model) => model,
                    Err(error) => {
                        tracing::error!(error = %error, "failed_to_load_model");
                        return Ok((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("failed to load model: {error}"),
                        )
                            .into_response());
                    }
                };

                let metadata = build_metadata(&model_path);
                let (mut parts, _body) = req.into_parts();
                let ws_upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
                    Ok(ws) => ws,
                    Err(error) => {
                        return Ok((StatusCode::BAD_REQUEST, error.to_string()).into_response());
                    }
                };

                let guard = connection_manager.acquire_connection();
                Ok(ws_upgrade
                    .max_message_size(4 * 1024 * 1024)
                    .on_upgrade(move |socket| async move {
                        handle_websocket(socket, params, metadata, guard, model, manager).await;
                    })
                    .into_response())
            } else {
                let content_type = req
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let accept = req
                    .headers()
                    .get("accept")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let body = match axum::body::to_bytes(req.into_body(), 100 * 1024 * 1024).await {
                    Ok(body) => body,
                    Err(error) => {
                        return Ok((StatusCode::BAD_REQUEST, error.to_string()).into_response());
                    }
                };

                if body.is_empty() {
                    return Ok((StatusCode::BAD_REQUEST, "request body is empty").into_response());
                }

                if accept.contains("text/event-stream") {
                    Ok(
                        batch::handle_batch_sse(
                            body,
                            &content_type,
                            &params,
                            &manager,
                            &model_path,
                        )
                        .await,
                    )
                } else {
                    Ok(
                        batch::handle_batch(body, &content_type, &params, &manager, &model_path)
                            .await,
                    )
                }
            }
        })
    }
}

struct ConnectionTasks {
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    producer: tokio::task::AbortHandle,
}

impl Drop for ConnectionTasks {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        self.producer.abort();
    }
}

async fn handle_websocket(
    socket: axum::extract::ws::WebSocket,
    params: ListenParams,
    metadata: owhisper_interface::stream::Metadata,
    guard: hypr_ws_utils::ConnectionGuard,
    model: Arc<hypr_whisper_local::LoadedWhisper>,
    _manager: ModelManager<hypr_whisper_local::LoadedWhisper>,
) {
    use super::live_worker::Output;
    use std::sync::atomic::{AtomicBool, Ordering};
    let started = std::time::Instant::now();
    let count = usize::from(params.channels).clamp(1, 2);
    let redemption = redemption_time(&params);
    let cancelled = Arc::new(AtomicBool::new(false));
    let (jobs, mut output, completion) =
        super::live_worker::start(model, params, cancelled.clone());
    let (mut sender, receiver) = socket.split();
    let lifecycle = tokio_util::sync::CancellationToken::new();
    let input_lifecycle = lifecycle.clone();
    let input_error = Arc::new(std::sync::Mutex::new(None));
    let producer_error = input_error.clone();
    let mut producer = tokio::spawn(async move {
        let result = super::live_input::run(receiver, jobs, count, redemption).await;
        if let Err(error) = &result {
            *producer_error.lock().unwrap() = Some(error.clone());
            input_lifecycle.cancel();
        }
        result
    });
    let _tasks = ConnectionTasks {
        cancelled: cancelled.clone(),
        producer: producer.abort_handle(),
    };
    let conversation = async {
        let mut producer_done = false;
        let mut first_text = true;
        loop {
            tokio::select! {
                result = &mut producer, if !producer_done => {
                    producer_done = true;
                    match result {
                        Ok(Ok(())) => {},
                        error => {
                            tracing::debug!(?error, "whisper_input_closed");
                            return;
                        }
                    }
                }
                message = output.recv() => {
                    match message {
                        Some(Output::Segment(channel, segment, finalized)) => {
                            if first_text { tracing::info!(elapsed_ms = started.elapsed().as_millis(), "whisper_live_first_text"); first_text = false; }
                            if !send_ws(&mut sender, &StreamResponse::SpeechStartedResponse { channel: vec![channel as u8], timestamp: segment.start }).await { return; }
                            if !send_ws(&mut sender, &build_transcript_response(&segment, if finalized { TranscriptKind::Finalized } else { TranscriptKind::Confirmed }, &metadata, &[channel as i32, count as i32])).await { return; }
                            if !send_ws(&mut sender, &StreamResponse::UtteranceEndResponse { channel: vec![channel as u8], last_word_end: segment.start + segment.duration }).await { return; }
                        }
                        Some(Output::Finished { duration, finalized }) => {
                            tracing::info!(elapsed_ms = started.elapsed().as_millis(), duration, finalized, "whisper_live_completed");
                            send_ws_best_effort(&mut sender, &StreamResponse::TerminalResponse {
                                request_id: metadata.request_id.clone(), created: format_timestamp_now(), duration, channels: count as u32,
                            }).await;
                            return;
                        }
                        Some(Output::Error(error)) => {
                            send_ws_best_effort(&mut sender, &StreamResponse::ErrorResponse { error_code: None, error_message: error, provider: "whisper-local".into() }).await;
                            return;
                        }
                        None => return,
                    }
                }
            }
        }
    };
    tokio::select! {
        _ = guard.cancelled() => { tracing::info!("websocket_cancelled_by_new_connection"); }
        _ = lifecycle.cancelled() => {}
        _ = conversation => {}
    }
    cancelled.store(true, Ordering::Release);
    producer.abort();
    // Dropping the receiver unblocks a worker stalled on output; completion never joins on Tokio.
    drop(output);
    let _ = completion.await;
    let error = input_error.lock().unwrap().take();
    if let Some(error) = error {
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            send_ws_best_effort(
                &mut sender,
                &StreamResponse::ErrorResponse {
                    error_code: None,
                    error_message: error,
                    provider: "whisper-local".into(),
                },
            ),
        )
        .await;
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), sender.close()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::build_metadata;

    #[test]
    fn health_and_listen_paths_are_stable() {
        assert_eq!(HEALTH_PATH, "/health");
        assert_eq!(LISTEN_PATH, "/v1/listen");
    }

    #[test]
    fn metadata_uses_model_info() {
        let metadata = build_metadata(std::path::Path::new("/tmp/model.bin"));
        assert_eq!(metadata.model_info.arch, "whisper-local");
    }
}

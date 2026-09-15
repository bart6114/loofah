mod error;
mod service;

pub use error::*;
pub use service::*;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use tokio_tungstenite::{connect_async, tungstenite::Error as TungsteniteError};

    #[tokio::test]
    #[ignore = "requires LOOFAH_WHISPER_MODEL"]
    async fn whisper_batch_sse_smoke() {
        use tower::ServiceExt;
        let app = TranscribeService::builder()
            .model_path(std::env::var("LOOFAH_WHISPER_MODEL").unwrap().into())
            .build()
            .into_router(|err: String| async move { (StatusCode::INTERNAL_SERVER_ERROR, err) });
        let audio_path = std::env::var("LOOFAH_WHISPER_AUDIO")
            .unwrap_or_else(|_| hypr_data::english_1::AUDIO_PATH.into());
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/listen?language=en&language=nl&keywords=Kubernetes")
            .header("content-type", "audio/wav")
            .header("accept", "text/event-stream")
            .body(axum::body::Body::from(std::fs::read(audio_path).unwrap()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        let text = std::str::from_utf8(&body).unwrap();
        let mut progress = 0.0;
        let mut results = 0;
        let mut segments = 0;
        for line in text.lines().filter_map(|l| l.strip_prefix("data: ")) {
            let event: owhisper_interface::batch_sse::BatchSseMessage =
                serde_json::from_str(line).unwrap();
            match event {
                owhisper_interface::batch_sse::BatchSseMessage::Progress { progress: next } => {
                    assert!(next.percentage >= progress);
                    progress = next.percentage;
                }
                owhisper_interface::batch_sse::BatchSseMessage::Segment { .. } => segments += 1,
                owhisper_interface::batch_sse::BatchSseMessage::Result { response } => {
                    results += 1;
                    assert!(
                        !response.results.channels[0].alternatives[0]
                            .words
                            .is_empty()
                    );
                }
                owhisper_interface::batch_sse::BatchSseMessage::Error { detail, .. } => {
                    panic!("{detail}")
                }
            }
        }
        assert_eq!(results, 1);
        assert!(segments > 0);
    }

    #[tokio::test]
    async fn websocket_invalid_model_path_fails_before_upgrade() {
        let app = TranscribeService::builder()
            .model_path(std::env::temp_dir().join("missing-whisper-model.bin"))
            .build()
            .into_router(|err: String| async move { (StatusCode::INTERNAL_SERVER_ERROR, err) });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        let result = connect_async(format!(
            "ws://{addr}/v1/listen?channels=1&sample_rate=16000"
        ))
        .await;

        match result {
            Err(TungsteniteError::Http(response)) => {
                assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
                let body = response
                    .body()
                    .as_ref()
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .unwrap_or_default();
                assert!(
                    body.contains("failed to load model"),
                    "unexpected body: {body}"
                );
            }
            other => panic!("expected HTTP upgrade failure, got {other:?}"),
        }

        let _ = shutdown_tx.send(());
    }

    #[tokio::test]
    async fn batch_errors_use_json_for_http_and_sse() {
        use tower::ServiceExt;
        for accept in ["application/json", "text/event-stream"] {
            let app = TranscribeService::builder()
                .model_path(std::env::temp_dir().join("missing-whisper-model.bin"))
                .build()
                .into_router(|err: String| async move { (StatusCode::INTERNAL_SERVER_ERROR, err) });
            let request = axum::http::Request::builder()
                .method("POST")
                .uri("/v1/listen")
                .header("content-type", "audio/wav")
                .header("accept", accept)
                .body(axum::body::Body::from(
                    std::fs::read(hypr_data::english_1::AUDIO_PATH).unwrap(),
                ))
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["error"], "model_load_failed");
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use std::time::{Duration, Instant};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    #[tokio::test]
    #[ignore = "requires LOOFAH_WHISPER_MODEL; real WebSocket lifecycle QA"]
    async fn live_finalize_replacement_and_disconnect() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("transcribe_whisper_local=debug,whisper_local=debug")
            .with_test_writer()
            .try_init();
        let app = TranscribeService::builder()
            .model_path(std::env::var("LOOFAH_WHISPER_MODEL").unwrap().into())
            .build()
            .into_router(
                |error| async move { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, error) },
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let url = format!(
            "ws://{address}/v1/listen?channels=2&sample_rate=16000&language=en&language=nl"
        );
        let run = async {
            let (mut socket, _) = connect_async(&url).await.unwrap();
            let batch_url = format!("http://{address}/v1/listen?language=en");
            let batch = tokio::spawn(async move {
                reqwest::Client::new()
                    .post(batch_url)
                    .header("content-type", "audio/wav")
                    .body(std::fs::read(hypr_data::english_1::AUDIO_PATH).unwrap())
                    .send()
                    .await
                    .unwrap()
                    .status()
            });
            let start = Instant::now();
            // Short startup, quiet system channel, and a final partial VAD frame.
            let samples = &hypr_data::english_1::AUDIO[..16000 * 2 * 6 + 200];
            let stereo: Vec<u8> = samples
                .chunks_exact(2)
                .flat_map(|p| [p[0], p[1], 0, 0])
                .collect();
            socket.send(Message::Binary(stereo.into())).await.unwrap();
            socket
                .send(Message::Text(r#"{"type":"Finalize"}"#.into()))
                .await
                .unwrap();
            let stop = Instant::now();
            let mut terminals = 0;
            let mut transcripts = 0;
            while let Some(Ok(message)) = socket.next().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let response: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_ne!(response["type"], "Error");
                if response["type"] == "Results" {
                    transcripts += 1;
                    assert_eq!(response["channel_index"][0], 0);
                    println!("live_first_text_ms={}", start.elapsed().as_millis());
                }
                if response["type"] == "Metadata" {
                    terminals += 1;
                    assert_eq!(response["channels"], 2);
                }
            }
            assert_eq!(terminals, 1);
            assert!(transcripts > 0);
            println!("live_stop_ms={}", stop.elapsed().as_millis());
            assert_eq!(batch.await.unwrap(), axum::http::StatusCode::OK);

            // The deadline must fire even when no more socket input arrives.
            let (mut deadline_socket, _) = connect_async(&url).await.unwrap();
            let mut stereo: Vec<u8> = samples
                .chunks_exact(2)
                .flat_map(|p| [p[0], p[1], 0, 0])
                .collect();
            stereo.resize(stereo.len() + 2 * 16000 * 4, 0);
            let deadline_started = Instant::now();
            deadline_socket
                .send(Message::Binary(stereo.into()))
                .await
                .unwrap();
            let mut saw_text = false;
            while let Some(Ok(message)) = deadline_socket.next().await {
                if let Message::Text(text) = message {
                    let response: serde_json::Value = serde_json::from_str(&text).unwrap();
                    assert_ne!(response["type"], "Error");
                    if response["type"] == "Results" {
                        saw_text = true;
                        break;
                    }
                }
            }
            assert!(saw_text);
            assert!(deadline_started.elapsed() >= Duration::from_secs(14));
            assert!(deadline_started.elapsed() < Duration::from_secs(30));
            println!(
                "live_startup_deadline_text_ms={}",
                deadline_started.elapsed().as_millis()
            );
            deadline_socket
                .send(Message::Text(r#"{"type":"Finalize"}"#.into()))
                .await
                .unwrap();
            let mut terminals = 0;
            while let Some(Ok(message)) = deadline_socket.next().await {
                if let Message::Text(text) = message
                    && serde_json::from_str::<serde_json::Value>(&text).unwrap()["type"]
                        == "Metadata"
                {
                    terminals += 1;
                }
            }
            assert_eq!(terminals, 1);

            let (mut first, _) = connect_async(&url).await.unwrap();
            let stereo: Vec<u8> = hypr_data::english_1::AUDIO[..16000 * 2 * 10]
                .chunks_exact(2)
                .flat_map(|p| [p[0], p[1], 0, 0])
                .collect();
            first
                .send(Message::Binary(stereo.clone().into()))
                .await
                .unwrap();
            let (mut replacement, _) = connect_async(&url).await.unwrap();
            replacement
                .send(Message::Text(r#"{"type":"Finalize"}"#.into()))
                .await
                .unwrap();
            let mut terminals = 0;
            while let Some(Ok(message)) = replacement.next().await {
                if let Message::Text(text) = message
                    && serde_json::from_str::<serde_json::Value>(&text).unwrap()["type"]
                        == "Metadata"
                {
                    terminals += 1;
                }
            }
            assert_eq!(terminals, 1);
            while let Some(Ok(message)) = first.next().await {
                if message.is_close() {
                    break;
                }
            }

            let (mut disconnected, _) = connect_async(&url).await.unwrap();
            disconnected
                .send(Message::Binary(stereo.into()))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(disconnected);
            let (mut final_connection, _) = connect_async(&url).await.unwrap();
            final_connection
                .send(Message::Text(r#"{"type":"Finalize"}"#.into()))
                .await
                .unwrap();
            while let Some(Ok(message)) = final_connection.next().await {
                if message.is_close() {
                    break;
                }
            }

            let (mut saturated, _) = connect_async(&url).await.unwrap();
            let audio: Vec<u8> = hypr_data::english_1::AUDIO[..16000 * 2 * 10]
                .chunks_exact(2)
                .flat_map(|p| [p[0], p[1], 0, 0])
                .collect();
            let audio = bytes::Bytes::from(audio);
            let writer = tokio::spawn(async move {
                for _ in 0..1000 {
                    if saturated
                        .send(Message::Binary(audio.clone()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
            tokio::time::sleep(Duration::from_millis(300)).await;
            let replaced = Instant::now();
            let (mut replacement, _) = connect_async(&url).await.unwrap();
            replacement
                .send(Message::Text(r#"{"type":"Finalize"}"#.into()))
                .await
                .unwrap();
            let mut terminals = 0;
            while let Some(Ok(message)) = replacement.next().await {
                if let Message::Text(text) = message
                    && serde_json::from_str::<serde_json::Value>(&text).unwrap()["type"]
                        == "Metadata"
                {
                    terminals += 1;
                }
            }
            assert_eq!(terminals, 1);
            assert!(replaced.elapsed() < Duration::from_secs(10));
            writer.abort();
            let _ = writer.await;
        };
        let result = tokio::time::timeout(Duration::from_secs(90), run).await;
        server.abort();
        result.expect("live connection stalled");
    }
}

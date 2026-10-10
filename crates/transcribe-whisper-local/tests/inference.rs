use std::time::Duration;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use futures_util::{SinkExt, StreamExt};
use owhisper_interface::batch_sse::BatchSseMessage;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tower::ServiceExt;
use transcribe_whisper_local::TranscribeService;

fn router() -> axum::Router {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("transcribe_whisper_local=debug,whisper_local=debug,model_manager=debug")
        .with_test_writer()
        .try_init();
    TranscribeService::builder()
        .model_path(
            std::env::var("LOOFAH_WHISPER_MODEL")
                .expect("LOOFAH_WHISPER_MODEL")
                .into(),
        )
        .build()
        .into_router(|error: String| async move { (StatusCode::INTERNAL_SERVER_ERROR, error) })
}

#[tokio::test]
#[ignore = "requires LOOFAH_WHISPER_MODEL"]
async fn real_batch_sse_has_progress_words_and_one_result() {
    tokio::time::timeout(Duration::from_secs(240), async {
        let response = router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/listen?language=en")
                    .header("content-type", "audio/wav")
                    .header("accept", "text/event-stream")
                    .body(Body::from(
                        std::fs::read(hypr_data::english_2::AUDIO_PATH).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        let mut progress = 0.0;
        let mut progress_events = 0;
        let mut results = 0;
        for line in std::str::from_utf8(&body)
            .unwrap()
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
        {
            match serde_json::from_str::<BatchSseMessage>(line).unwrap() {
                BatchSseMessage::Progress { progress: next } => {
                    assert!(next.percentage >= progress);
                    progress = next.percentage;
                    progress_events += 1;
                }
                BatchSseMessage::Result { response } => {
                    results += 1;
                    let alternative = &response.results.channels[0].alternatives[0];
                    let text = alternative.transcript.to_lowercase();
                    assert!(text.contains("hello") && text.contains("jersey"), "{text}");
                    assert!(!alternative.words.is_empty());
                    assert!(alternative.words.iter().all(|word| word.start.is_finite()
                        && word.end.is_finite()
                        && word.start >= 0.0
                        && word.end >= word.start));
                }
                BatchSseMessage::Error { detail, .. } => panic!("{detail}"),
                BatchSseMessage::Segment { .. } => {}
            }
        }
        assert_eq!(results, 1);
        assert!(progress_events > 0);
    })
    .await
    .expect("batch transcription stalled");
}

#[tokio::test]
#[ignore = "requires LOOFAH_WHISPER_MODEL"]
async fn real_websocket_finalizes_and_recovers_from_cancellation() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router()).await.unwrap() });
    let result = tokio::time::timeout(Duration::from_secs(240), async {
        let url = format!(
            "ws://{address}/v1/listen?language=en&channels=1&sample_rate=16000&encoding=linear16"
        );
        let (mut socket, _) = connect_async(&url).await.unwrap();
        // Keep repeated CPU encoder passes bounded on portable ARM64 kernels.
        for chunk in hypr_data::english_2::AUDIO[..16000 * 2 * 10].chunks(16000 * 2) {
            socket
                .send(Message::Binary(chunk.to_vec().into()))
                .await
                .unwrap();
        }
        socket
            .send(Message::Text(r#"{"type":"Finalize"}"#.into()))
            .await
            .unwrap();
        let mut text = String::new();
        let mut terminals = 0;
        let mut closed = false;
        while let Some(message) = socket.next().await {
            let message = message.unwrap();
            closed |= matches!(message, Message::Close(_));
            let Message::Text(message) = message else {
                continue;
            };
            let response: serde_json::Value = serde_json::from_str(&message).unwrap();
            assert_ne!(response["type"], "Error", "{response}");
            if response["type"] == "Results" && response["is_final"] == true {
                text.push_str(
                    response["channel"]["alternatives"][0]["transcript"]
                        .as_str()
                        .unwrap(),
                );
                for word in response["channel"]["alternatives"][0]["words"]
                    .as_array()
                    .unwrap()
                {
                    let start = word["start"].as_f64().unwrap();
                    let end = word["end"].as_f64().unwrap();
                    assert!(start.is_finite() && end.is_finite() && start >= 0.0 && end >= start);
                }
            }
            if response["type"] == "Metadata" {
                terminals += 1;
            }
        }
        assert!(closed, "server ended without a WebSocket close frame");
        assert_eq!(terminals, 1);
        assert!(text.to_lowercase().contains("hello"), "{text}");
        let (mut cancelled, _) = connect_async(&url).await.unwrap();
        cancelled
            .send(Message::Binary(
                hypr_data::english_2::AUDIO[..16000 * 2 * 10]
                    .to_vec()
                    .into(),
            ))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(cancelled);
        tokio::time::timeout(Duration::from_secs(15), async {
            let (mut replacement, _) = connect_async(&url).await.unwrap();
            replacement
                .send(Message::Text(r#"{"type":"Finalize"}"#.into()))
                .await
                .unwrap();
            let mut terminals = 0;
            while let Some(message) = replacement.next().await {
                if let Message::Text(message) = message.unwrap() {
                    let response: serde_json::Value = serde_json::from_str(&message).unwrap();
                    assert_ne!(response["type"], "Error", "{response}");
                    if response["type"] == "Metadata" {
                        terminals += 1;
                    }
                }
            }
            assert_eq!(terminals, 1);
        })
        .await
        .expect("cancelled connection blocked its replacement");
    })
    .await;
    server.abort();
    result.expect("live transcription stalled");
}

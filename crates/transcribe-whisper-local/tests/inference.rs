use std::time::{Duration, Instant};

#[path = "support/delayed_proxy.rs"]
mod delayed_proxy;
#[path = "support/native_observation.rs"]
mod native_observation;

use delayed_proxy::{DelayedProxy, delay_budget};

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
    let _model_guard = native_observation::MODEL_TEST_LOCK.lock().await;
    let _native = native_observation::install();
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

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send_speech(socket: &mut Socket) {
    for chunk in hypr_data::english_2::AUDIO[..16000 * 2 * 10].chunks(16000 * 2) {
        socket
            .send(Message::Binary(chunk.to_vec().into()))
            .await
            .unwrap();
    }
}

async fn observe_speech(socket: &mut Socket) {
    while let Some(message) = socket.next().await {
        let message = message.unwrap();
        assert!(
            !matches!(message, Message::Close(_)),
            "closed before speech"
        );
        if let Message::Text(message) = message {
            let response: serde_json::Value = serde_json::from_str(&message).unwrap();
            assert_ne!(response["type"], "Error", "{response}");
            assert_ne!(
                response["type"], "Metadata",
                "completed before cancellation"
            );
            if response["type"] == "Results"
                && !response["channel"]["alternatives"][0]["transcript"]
                    .as_str()
                    .unwrap()
                    .is_empty()
            {
                return;
            }
        }
    }
    panic!("server ended before real speech output");
}

async fn receive_close(socket: &mut Socket, expected_terminals: usize) -> String {
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
    assert_eq!(terminals, expected_terminals);
    text
}

async fn finalize(socket: &mut Socket) -> String {
    socket
        .send(Message::Text(r#"{"type":"Finalize"}"#.into()))
        .await
        .unwrap();
    receive_close(socket, 1).await
}

async fn bounded_probe<T>(
    name: &str,
    rtt_ms: u64,
    allowance: Duration,
    connections: usize,
    work: impl std::future::Future<Output = T>,
) -> T {
    let budget = allowance + delay_budget(Duration::from_millis(rtt_ms), connections);
    let started = Instant::now();
    let result = tokio::time::timeout(budget, work)
        .await
        .unwrap_or_else(|_| panic!("WebSocket {name} stalled at {rtt_ms} ms RTT"));
    println!(
        "websocket shaped probe: rtt_ms={rtt_ms} name={name} elapsed_ms={} budget_ms={}",
        started.elapsed().as_millis(),
        budget.as_millis(),
    );
    result
}

fn connection_proof(rtt_ms: u64, proof: delayed_proxy::Connection) {
    let index = proof.index;
    assert!(proof.upstream.peer_closed && proof.downstream.peer_closed);
    assert!(proof.upstream.bytes > 0 && proof.downstream.bytes > 0);
    if !matches!(index, 2 | 4) {
        assert_eq!(proof.upstream.completed_write_bytes, proof.upstream.bytes);
    }
    if index != 4 {
        assert_eq!(
            proof.downstream.completed_write_bytes,
            proof.downstream.bytes
        );
    }
    if index != 0 {
        // The service joins its CPU worker before emitting this real Close frame.
        // Even a dropped client must leave that frame in the owned proxy capture.
        proof.require_server_close();
    }
    println!(
        "websocket shaped connection: rtt_ms={rtt_ms} index={index} upload_bytes={} upload_slabs={} completed_upload_bytes={} download_bytes={} download_slabs={} completed_download_bytes={} client_closed=true server_eof=true server_close={}",
        proof.upstream.bytes,
        proof.upstream.slabs,
        proof.upstream.completed_write_bytes,
        proof.downstream.bytes,
        proof.downstream.slabs,
        proof.downstream.completed_write_bytes,
        index != 0,
    );
}

async fn shaped_websocket_profile(
    upstream: std::net::SocketAddr,
    rtt_ms: u64,
    native: &native_observation::NativeEvents,
) {
    let rtt = Duration::from_millis(rtt_ms);
    let connections = if rtt_ms == 50 { 6 } else { 4 };
    let mut proxy = DelayedProxy::start(upstream, rtt, connections).await;
    let model_allowance = Duration::from_secs(240);
    let drain_allowance = Duration::from_secs(15);
    let started = Instant::now();
    let health_elapsed = bounded_probe("health", rtt_ms, drain_allowance, 1, async {
        let began = Instant::now();
        let health = reqwest::Client::new()
            .get(format!("http://{}/health", proxy.address))
            .header("connection", "close")
            .send()
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        assert_eq!(health.text().await.unwrap(), "ok");
        let elapsed = began.elapsed();
        assert!(elapsed >= rtt);
        connection_proof(rtt_ms, proxy.connection(0).await);
        elapsed
    })
    .await;
    let url = format!(
        "ws://{}/v1/listen?language=en&channels=1&sample_rate=16000&encoding=linear16",
        proxy.address
    );
    let (handshake_elapsed, finalize_elapsed) =
        bounded_probe("finalization", rtt_ms, model_allowance, 1, async {
            let handshake_started = Instant::now();
            let (mut socket, _) = connect_async(&url).await.unwrap();
            let handshake_elapsed = handshake_started.elapsed();
            send_speech(&mut socket).await;
            let finalize_started = Instant::now();
            let text = finalize(&mut socket).await;
            let finalize_elapsed = finalize_started.elapsed();
            drop(socket);
            assert!(text.to_lowercase().contains("hello"), "{text}");
            connection_proof(rtt_ms, proxy.connection(1).await);
            (handshake_elapsed, finalize_elapsed)
        })
        .await;

    let mut cancelled = bounded_probe("replacement-setup", rtt_ms, model_allowance, 1, async {
        let (mut socket, _) = connect_async(&url).await.unwrap();
        send_speech(&mut socket).await;
        observe_speech(&mut socket).await;
        socket
    })
    .await;
    let cancellation_started = Instant::now();
    bounded_probe("replacement-drain", rtt_ms, drain_allowance, 2, async {
        let (mut replacement, _) = connect_async(&url).await.unwrap();
        receive_close(&mut cancelled, 0).await;
        drop(cancelled);
        finalize(&mut replacement).await;
        drop(replacement);
        connection_proof(rtt_ms, proxy.connection(2).await);
        connection_proof(rtt_ms, proxy.connection(3).await);
    })
    .await;
    let cancellation_elapsed = cancellation_started.elapsed();

    let mut abrupt_elapsed = None;
    if rtt_ms == 50 {
        let disconnected = bounded_probe("abrupt-setup", rtt_ms, model_allowance, 1, async {
            let before = native.snapshot();
            let (mut socket, _) = connect_async(&url).await.unwrap();
            send_speech(&mut socket).await;
            native.started_after(before.0).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            let at_drop = native.snapshot();
            assert!(at_drop.0 > before.0);
            assert_eq!(at_drop.1, before.1, "native call completed before early drop");
            println!(
                "websocket abrupt witness: rtt_ms={rtt_ms} started_before={} completed_before={} started_at_drop={} completed_at_drop={} post_start_wait_ms=100 witness=pre_call_trace_not_inside_kernel",
                before.0, before.1, at_drop.0, at_drop.1,
            );
            socket
        })
        .await;
        let dropped = Instant::now();
        bounded_probe("abrupt-drain", rtt_ms, drain_allowance, 2, async {
            drop(disconnected);
            let (mut replacement, _) = connect_async(&url).await.unwrap();
            finalize(&mut replacement).await;
            drop(replacement);
            connection_proof(rtt_ms, proxy.connection(4).await);
            connection_proof(rtt_ms, proxy.connection(5).await);
        })
        .await;
        abrupt_elapsed = Some(dropped.elapsed().as_millis());
    }
    bounded_probe("proxy-join", rtt_ms, drain_allowance, 0, proxy.finish()).await;
    println!(
        "websocket shaped profile: rtt_ms={rtt_ms} health_ms={} handshake_ms={} finalize_ms={} replacement_cancel_drain_ms={} abrupt_reconnect_ms={abrupt_elapsed:?} total_ms={} connections={connections} slab_bytes={} model_probe_allowance_ms={} cancel_probe_allowance_ms={}",
        health_elapsed.as_millis(),
        handshake_elapsed.as_millis(),
        finalize_elapsed.as_millis(),
        cancellation_elapsed.as_millis(),
        started.elapsed().as_millis(),
        delayed_proxy::SLAB_BYTES,
        model_allowance.as_millis(),
        drain_allowance.as_millis(),
    );
}

#[tokio::test]
#[ignore = "requires LOOFAH_WHISPER_MODEL; real CPU WebSocket at 50/150/500 ms RTT"]
async fn real_websocket_finalizes_and_recovers_from_cancellation() {
    let _model_guard = native_observation::MODEL_TEST_LOCK.lock().await;
    let native = native_observation::install();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router()).await.unwrap() });
    for rtt_ms in [50, 150, 500] {
        shaped_websocket_profile(address, rtt_ms, &native).await;
    }
    server.abort();
    let _ = server.await;
}

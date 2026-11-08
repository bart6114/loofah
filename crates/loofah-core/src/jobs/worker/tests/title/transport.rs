use crate::jobs::{
    configured::{ConfiguredSummaryGenerator, ProcessingAdmission},
    providers::{HttpModel, HttpModelConfig, HttpProvider},
    source::SummaryIdentity,
};
use axum::{Router, body::Body, extract::State, response::Response};

use super::*;

async fn handler(State(calls): State<Arc<AtomicUsize>>, body: axum::body::Bytes) -> Response {
    calls.fetch_add(1, Ordering::SeqCst);
    let input: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let title = input["max_tokens"] == 128;
    if title {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    let delta = serde_json::json!({"choices":[{"index":0,"delta":{"content": if title { "Unconfirmed partial title" } else { BODY }}}]});
    let mut response = format!("data: {delta}\n\n");
    if !title {
        response.push_str("data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n");
    }
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(async_stream::stream! {
            for bytes in response.as_bytes().chunks(17) {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                yield Ok::<_, std::io::Error>(bytes.to_vec());
            }
        }))
        .unwrap()
}

#[tokio::test]
async fn delayed_http_title_disconnect_keeps_body_success_and_never_retries_paid_generation() {
    let fixture = blank_fixture().await;
    fixture
        .context
        .settings
        .set_values(std::collections::HashMap::from([
            ("current_llm_provider".into(), serde_json::json!("custom")),
            ("current_llm_model".into(), serde_json::json!("fixture")),
        ]))
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().fallback(handler).with_state(calls.clone());
    let stopping = CancellationToken::new();
    let server_stop = stopping.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(server_stop.cancelled_owned())
            .await
            .unwrap();
    });
    let model = HttpModel::new(HttpModelConfig {
        provider: HttpProvider::ChatCompletions,
        base_url: url,
        model: "fixture".into(),
        api_key: None,
        allow_insecure_http: false,
    })
    .unwrap();
    let generator = Arc::new(ConfiguredSummaryGenerator::new(
        &fixture.context,
        SummaryIdentity::LocalOwner,
        Arc::new(super::recovery::Models(Arc::new(model))),
        ProcessingAdmission::new(1, 0).unwrap(),
        true,
    ));
    let worker = JobWorker::start(
        fixture.context.clone(),
        fixture.journal.clone(),
        generator,
        fixture.authorizer.clone(),
    )
    .await
    .unwrap();
    let job = submit(&fixture).await;
    assert_eq!(
        fixture.terminal(&job.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert_eq!(
        title_result(&fixture, &job.id).await.status,
        ProcessingTitleStatus::Interrupted
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.summary().await, BODY);
    assert_eq!(
        fixture
            .context
            .store
            .read_meta("note")
            .await
            .unwrap()
            .unwrap()
            .title,
        ""
    );
    worker.shutdown().await.unwrap();
    stopping.cancel();
    server.await.unwrap();
}

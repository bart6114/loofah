use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, Request, Response, Uri},
};
use serde_json::{Value, json};

use super::*;
use crate::jobs::{
    JobAuthorizer, JobWork,
    generation::{ModelChunk, SummaryGenerator},
    source::{SummaryIdentity, VaultSummarySource},
};
use crate::jobs::{
    JobGenerator,
    generation::{SummaryPrompt, SummarySource},
};

struct Recorded {
    uri: Uri,
    headers: HeaderMap,
    body: Value,
}
struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    chunks: Vec<(Duration, Vec<u8>)>,
    header_delay: Duration,
}
impl Reply {
    fn sse(body: String) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type", "text/event-stream; charset=utf-8".into())],
            chunks: vec![(Duration::ZERO, body.into_bytes())],
            header_delay: Duration::ZERO,
        }
    }
    fn status(status: u16) -> Self {
        Self {
            status,
            ..Self::sse("secret provider failure body".into())
        }
    }
}
struct ServerState {
    requests: Mutex<Vec<Recorded>>,
    replies: Mutex<VecDeque<Reply>>,
    stop: CancellationToken,
}
struct Server {
    url: String,
    state: Arc<ServerState>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.state.stop.cancel();
    }
}
impl Server {
    async fn start(replies: Vec<Reply>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api", listener.local_addr().unwrap());
        let state = Arc::new(ServerState {
            requests: Mutex::new(Vec::new()),
            replies: Mutex::new(replies.into()),
            stop: CancellationToken::new(),
        });
        let app = Router::new().fallback(handler).with_state(state.clone());
        let stop = state.stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            url,
            state,
            task: Some(task),
        }
    }
    fn model(&self, provider: HttpProvider) -> HttpModel {
        HttpModel::new(HttpModelConfig {
            allow_insecure_http: false,
            provider,
            base_url: self.url.clone(),
            model: "configured-model".into(),
            api_key: Some("host-secret".into()),
        })
        .unwrap()
    }
    async fn close(mut self) {
        self.state.stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}
async fn handler(State(state): State<Arc<ServerState>>, request: Request<Body>) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, 24 * 1024 * 1024).await.unwrap();
    state.requests.lock().unwrap().push(Recorded {
        uri: parts.uri,
        headers: parts.headers,
        body: serde_json::from_slice(&body).unwrap(),
    });
    let reply = state
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| Reply::status(500));
    tokio::select! {_ = state.stop.cancelled() => {}, _ = tokio::time::sleep(reply.header_delay) => {}}
    let stop = state.stop.clone();
    let mut response = Response::builder().status(reply.status);
    for (key, value) in reply.headers {
        response = response.header(key, value);
    }
    response.body(Body::from_stream(async_stream::stream! {
        for (delay,bytes) in reply.chunks {
            tokio::select! {biased; _ = stop.cancelled() => break, _ = tokio::time::sleep(delay) => {}}
            yield Ok::<_,std::io::Error>(Bytes::from(bytes));
        }
    })).unwrap()
}
fn request() -> ModelRequest {
    ModelRequest {
        system: "Summarize faithfully.".into(),
        prompt: "Ship Friday.".into(),
        images: vec![super::super::generation::SummaryImage {
            bytes: vec![1, 2, 3],
            mime_type: "image/png".into(),
        }],
        max_output_tokens: 8192,
        max_retries: 4,
    }
}
fn event(value: Value) -> String {
    format!("data: {value}\n\n")
}
fn named(kind: &str, mut value: Value) -> String {
    value["type"] = kind.into();
    format!("event: {kind}\ndata: {value}\n\n")
}
fn chat_text(text: &str) -> String {
    event(json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]}))
}
fn chat_finish(reason: &str) -> String {
    event(json!({"choices":[{"index":0,"delta":{},"finish_reason":reason}]}))
}
fn success(provider: HttpProvider, text: &str) -> String {
    match provider {
        HttpProvider::OpenAi | HttpProvider::AzureOpenAi => format!(
            "{}{}",
            named("response.output_text.delta", json!({"delta":text})),
            named(
                "response.completed",
                json!({"response":{"status":"completed","output":[]}})
            )
        ),
        HttpProvider::Anthropic => format!(
            "{}{}{}{}{}{}",
            named("message_start", json!({"message":{"content":[]}})),
            named(
                "content_block_start",
                json!({"index":0,"content_block":{"type":"text","text":""}})
            ),
            named(
                "content_block_delta",
                json!({"index":0,"delta":{"type":"text_delta","text":text}})
            ),
            named("content_block_stop", json!({"index":0})),
            named("message_delta", json!({"delta":{"stop_reason":"end_turn"}})),
            named("message_stop", json!({}))
        ),
        HttpProvider::Google => format!(
            "{}{}",
            event(json!({"candidates":[{"index":0,"content":{"parts":[{"text":text}]}}]})),
            event(json!({"candidates":[{"index":0,"finishReason":"STOP"}]}))
        ),
        _ => format!("{}{}data: [DONE]\n\n", chat_text(text), chat_finish("stop")),
    }
}
async fn collect(model: &HttpModel) -> Result<Vec<ModelChunk>, ProcessingJobFailure> {
    model
        .stream(request(), CancellationToken::new())
        .await?
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
}

#[tokio::test]
async fn configured_provider_formats_stream_through_real_http_without_exposing_keys() {
    for (provider, path, header) in [
        (
            HttpProvider::OpenAi,
            "/api/responses",
            Some("authorization"),
        ),
        (
            HttpProvider::AzureOpenAi,
            "/api/v1/responses?api-version=v1",
            Some("api-key"),
        ),
        (
            HttpProvider::AzureAi,
            "/api/chat/completions",
            Some("api-key"),
        ),
        (
            HttpProvider::ChatCompletions,
            "/api/chat/completions",
            Some("authorization"),
        ),
        (HttpProvider::Ollama, "/api/chat/completions", None),
        (HttpProvider::Anthropic, "/api/messages", Some("x-api-key")),
        (
            HttpProvider::Google,
            "/api/models/configured-model:streamGenerateContent?alt=sse",
            Some("x-goog-api-key"),
        ),
    ] {
        let mut reply = Reply::sse(String::new());
        reply.chunks = success(provider, "# Café 🚀\nShip Friday.")
            .as_bytes()
            .chunks(17)
            .map(|bytes| (Duration::from_millis(1), bytes.to_vec()))
            .collect();
        let server = Server::start(vec![reply]).await;
        let output = collect(&server.model(provider)).await.unwrap();
        assert!(
            matches!(&output[..],[ModelChunk::Text(text), ModelChunk::Finish{truncated:false}] if text == "# Café 🚀\nShip Friday.")
        );
        {
            let requests = server.state.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let recorded = &requests[0];
            assert_eq!(recorded.uri.to_string(), path);
            if let Some(header) = header {
                assert!(
                    recorded.headers[header]
                        .to_str()
                        .unwrap()
                        .ends_with("host-secret")
                );
            }
            assert!(!recorded.body.to_string().contains("host-secret"));
            assert!(!recorded.uri.to_string().contains("host-secret"));
            match provider {
                HttpProvider::OpenAi | HttpProvider::AzureOpenAi => {
                    assert_eq!(recorded.body["model"], "configured-model");
                    assert_eq!(recorded.body["instructions"], "Summarize faithfully.");
                    assert_eq!(recorded.body["store"], false);
                    assert_eq!(
                        recorded.body["input"][0]["content"][1]["image_url"],
                        "data:image/png;base64,AQID"
                    );
                    assert_eq!(recorded.body["max_output_tokens"], 8192);
                }
                HttpProvider::Anthropic => {
                    assert_eq!(recorded.headers["anthropic-version"], "2023-06-01");
                    assert!(
                        !recorded
                            .headers
                            .contains_key("anthropic-dangerous-direct-browser-access")
                    );
                    assert_eq!(
                        recorded.body["messages"][0]["content"][1]["source"]["data"],
                        "AQID"
                    );
                }
                HttpProvider::Google => {
                    assert_eq!(
                        recorded.body["systemInstruction"]["parts"][0]["text"],
                        "Summarize faithfully."
                    );
                    assert_eq!(
                        recorded.body["contents"][0]["parts"][1]["inlineData"]["data"],
                        "AQID"
                    );
                }
                _ => {
                    assert_eq!(
                        recorded.body["messages"][1]["content"][1]["image_url"]["url"],
                        "data:image/png;base64,AQID"
                    );
                    if matches!(provider, HttpProvider::Ollama) {
                        assert_eq!(
                            recorded.headers["origin"],
                            url::Url::parse(&server.url)
                                .unwrap()
                                .origin()
                                .ascii_serialization()
                        );
                        assert!(!recorded.headers.contains_key("authorization"));
                    }
                }
            }
        }
        server.close().await;
    }
}

#[tokio::test]
async fn only_explicit_rate_limit_rejection_is_retried_and_limits_are_honored() {
    let mut rejected = Reply::status(429);
    rejected.headers.push(("retry-after", "0".into()));
    let server = Server::start(vec![
        rejected,
        Reply::sse(success(HttpProvider::OpenAi, "# Done")),
    ])
    .await;
    assert!(collect(&server.model(HttpProvider::OpenAi)).await.is_ok());
    assert_eq!(server.state.requests.lock().unwrap().len(), 2);
    server.close().await;
    for code in [400, 401, 403, 408, 500, 502, 503] {
        let server = Server::start(vec![
            Reply::status(code),
            Reply::sse(success(HttpProvider::OpenAi, "# Duplicate")),
        ])
        .await;
        assert!(collect(&server.model(HttpProvider::OpenAi)).await.is_err());
        assert_eq!(server.state.requests.lock().unwrap().len(), 1);
        server.close().await;
    }
    let server = Server::start(
        (0..6)
            .map(|_| {
                let mut r = Reply::status(429);
                r.headers.push(("retry-after", "0".into()));
                r
            })
            .collect(),
    )
    .await;
    assert!(matches!(
        collect(&server.model(HttpProvider::OpenAi)).await,
        Err(ProcessingJobFailure::ProviderUnavailable)
    ));
    assert_eq!(server.state.requests.lock().unwrap().len(), 5);
    server.close().await;
}

#[tokio::test]
async fn redirects_do_not_forward_credentials_or_body() {
    let destination =
        Server::start(vec![Reply::sse(success(HttpProvider::OpenAi, "# Leaked"))]).await;
    let mut redirect = Reply::status(307);
    redirect.headers.push(("location", destination.url.clone()));
    let server = Server::start(vec![redirect]).await;
    assert!(collect(&server.model(HttpProvider::OpenAi)).await.is_err());
    assert!(destination.state.requests.lock().unwrap().is_empty());
    server.close().await;
    destination.close().await;
}

#[tokio::test]
async fn ambiguous_disconnect_never_repeats_request_or_confirms_partial_output() {
    for body in [
        chat_text("# Partial"),
        format!("{}data: {{", chat_text("# Partial")),
        String::new(),
    ] {
        let server = Server::start(vec![
            Reply::sse(body),
            Reply::sse(success(HttpProvider::ChatCompletions, "# Duplicate")),
        ])
        .await;
        assert!(matches!(
            collect(&server.model(HttpProvider::ChatCompletions)).await,
            Err(ProcessingJobFailure::Interrupted)
        ));
        assert_eq!(server.state.requests.lock().unwrap().len(), 1);
        server.close().await;
    }
}

#[tokio::test]
async fn cancellation_interrupts_headers_body_and_retry_backoff() {
    for stage in ["headers", "body", "backoff"] {
        let mut reply = Reply::sse(chat_text("# Partial"));
        if stage == "headers" {
            reply.header_delay = Duration::from_secs(20);
        }
        if stage == "body" {
            reply
                .chunks
                .push((Duration::from_secs(20), chat_finish("stop").into_bytes()));
        }
        if stage == "backoff" {
            reply.status = 429;
            reply.headers.push(("retry-after", "20".into()));
        }
        let server = Server::start(vec![reply]).await;
        let model = Arc::new(server.model(HttpProvider::ChatCompletions));
        let cancel = CancellationToken::new();
        let stopping = cancel.clone();
        let task = tokio::spawn(async move {
            match model.stream(request(), stopping).await {
                Ok(stream) => stream
                    .collect::<Vec<_>>()
                    .await
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>(),
                Err(cause) => Err(cause),
            }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while server.state.requests.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if stage == "body" {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        cancel.cancel();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap(),
            Err(ProcessingJobFailure::Interrupted)
        ));
        assert_eq!(server.state.requests.lock().unwrap().len(), 1);
        server.close().await;
    }
}

#[tokio::test]
async fn idle_and_total_timeouts_are_interrupted_without_retries() {
    for idle in [true, false] {
        let mut reply = Reply::sse(chat_text("# Partial"));
        if idle {
            reply
                .chunks
                .push((Duration::from_secs(20), chat_finish("stop").into_bytes()));
        } else {
            reply
                .chunks
                .extend((0..40).map(|_| (Duration::from_millis(10), b": ping\n\n".to_vec())));
        }
        let server = Server::start(vec![reply]).await;
        let model = HttpModel::with_timeouts(
            HttpModelConfig {
                allow_insecure_http: false,
                provider: HttpProvider::ChatCompletions,
                base_url: server.url.clone(),
                model: "same-model".into(),
                api_key: None,
            },
            if idle {
                Duration::from_millis(30)
            } else {
                Duration::from_secs(2)
            },
            Duration::from_millis(150),
        )
        .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), collect(&model))
                .await
                .unwrap(),
            Err(ProcessingJobFailure::Interrupted)
        ));
        assert_eq!(server.state.requests.lock().unwrap().len(), 1);
        server.close().await;
    }
}

#[test]
fn endpoint_and_header_validation_rejects_unsafe_configuration_without_secret_diagnostics() {
    for endpoint in [
        "http://example.com/v1",
        "ftp://127.0.0.1/v1",
        "https://user:secret@example.com/v1",
        "https://example.com/v1?api_key=secret",
        "https://example.com/v1#secret",
    ] {
        assert!(matches!(
            HttpModel::new(HttpModelConfig {
                allow_insecure_http: false,
                provider: HttpProvider::OpenAi,
                base_url: endpoint.into(),
                model: "configured".into(),
                api_key: Some("secret".into())
            }),
            Err(ProcessingJobFailure::ProviderUnavailable)
        ));
    }
    for key in [None, Some(""), Some("invalid\r\nheader")] {
        assert!(
            HttpModel::new(HttpModelConfig {
                allow_insecure_http: false,
                provider: HttpProvider::OpenAi,
                base_url: "https://example.com/v1".into(),
                model: "configured".into(),
                api_key: key.map(str::to_owned)
            })
            .is_err()
        );
    }
    assert!(
        HttpModel::new(HttpModelConfig {
            allow_insecure_http: false,
            provider: HttpProvider::ChatCompletions,
            base_url: "http://[::1]:1234/v1".into(),
            model: "configured".into(),
            api_key: None
        })
        .is_ok()
    );
}

fn decode(provider: HttpProvider, input: &str) -> Result<Vec<ModelChunk>, ProcessingJobFailure> {
    let mut decoder = sse::Decoder::default();
    let mut wire = wire::Wire::new(provider);
    let mut output = Vec::new();
    for byte in input.bytes() {
        if let Some(event) = decoder.push(byte)? {
            output.extend(wire.event(event)?);
        }
    }
    decoder.end()?;
    wire.end()?;
    Ok(output)
}

#[test]
fn sse_handles_split_utf8_bom_crlf_cr_comments_and_multiline_json() {
    let input = "\u{feff}: ping\r\nid: 7\r\nevent: response.output_text.delta\r\ndata: {\r\ndata: \"type\":\"response.output_text.delta\",\"delta\":\"# Café 🚀\"}\r\n\r\n";
    let final_event = named(
        "response.completed",
        json!({"response":{"status":"completed"}}),
    );
    for ending in ["\n", "\r\n", "\r"] {
        let input = format!("{}{}", input, final_event)
            .replace("\r\n", "\n")
            .replace('\n', ending);
        let output = decode(HttpProvider::OpenAi, &input).unwrap();
        assert!(
            matches!(&output[..],[ModelChunk::Text(text),ModelChunk::Finish{truncated:false}] if text=="# Café 🚀")
        );
    }
}

#[test]
fn sse_limits_invalid_utf8_unterminated_events_and_oversized_frames() {
    let mut decoder = sse::Decoder::default();
    decoder.push(0xff).unwrap();
    assert_eq!(
        decoder.push(b'\n').err(),
        Some(ProcessingJobFailure::ProviderFailed)
    );
    for input in ["data: {}", "data: {}\n", "event: response.completed\n"] {
        let mut decoder = sse::Decoder::default();
        for byte in input.bytes() {
            decoder.push(byte).unwrap();
        }
        assert_eq!(decoder.end(), Err(ProcessingJobFailure::Interrupted));
    }
    let mut decoder = sse::Decoder::default();
    for _ in 0..1024 * 1024 {
        decoder.push(b'a').unwrap();
    }
    assert_eq!(
        decoder.push(b'a').err(),
        Some(ProcessingJobFailure::ResourceLimit)
    );
}

#[test]
fn input_budgets_and_provider_namespaces_are_checked_before_dispatch() {
    let mut text_only = request();
    text_only.images.clear();
    let body = request::body(
        HttpProvider::ChatCompletions,
        "configured".into(),
        text_only,
    )
    .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["messages"][1]["content"], "Ship Friday.");
    let mut input = request();
    input.prompt = "a".repeat(16 * 1024 * 1024 + 1);
    assert_eq!(
        request::body(HttpProvider::OpenAi, "configured".into(), input).err(),
        Some(ProcessingJobFailure::ResourceLimit)
    );
    let mut input = request();
    input.images[0].mime_type = "image/svg+xml".into();
    assert_eq!(
        request::body(HttpProvider::OpenAi, "configured".into(), input).err(),
        Some(ProcessingJobFailure::SourceUnavailable)
    );
    for model in ["models/../escape", "models/..", "other/model"] {
        assert!(
            HttpModel::new(HttpModelConfig {
                allow_insecure_http: false,
                provider: HttpProvider::Google,
                base_url: "https://example.com/v1".into(),
                model: model.into(),
                api_key: Some("secret".into())
            })
            .is_err()
        );
    }
    let body = request::body(HttpProvider::Google, "gemma-configured".into(), request()).unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert!(body.get("systemInstruction").is_none());
    assert_eq!(
        body["contents"][0]["parts"][0]["text"],
        "Summarize faithfully."
    );
    for endpoint in ["http://localhost:1234/v1", "http://[::1]:1234/v1"] {
        assert!(
            HttpModel::new(HttpModelConfig {
                allow_insecure_http: false,
                provider: HttpProvider::ChatCompletions,
                base_url: endpoint.into(),
                model: "configured".into(),
                api_key: None
            })
            .is_ok()
        );
    }
    assert!(
        HttpModel::new(HttpModelConfig {
            allow_insecure_http: true,
            provider: HttpProvider::ChatCompletions,
            base_url: "http://company-model.internal/v1".into(),
            model: "configured".into(),
            api_key: None
        })
        .is_ok()
    );
}

#[test]
fn provider_reasoning_is_excluded_without_hiding_actual_output() {
    let google = format!(
        "{}{}",
        event(
            json!({"candidates":[{"content":{"parts":[{"thought":true,"text":"private reasoning"},{"text":"# Answer"}]}}]})
        ),
        event(json!({"candidates":[{"finishReason":"STOP"}]}))
    );
    let output = decode(HttpProvider::Google, &google).unwrap();
    assert!(
        matches!(&output[..],[ModelChunk::Text(text),ModelChunk::Finish{..}] if text=="# Answer")
    );
    let anthropic = success(HttpProvider::Anthropic, "# Answer");
    let thinking = format!(
        "{}{}{}",
        named(
            "content_block_start",
            json!({"index":7,"content_block":{"type":"thinking","thinking":"private"}})
        ),
        named(
            "content_block_delta",
            json!({"index":7,"delta":{"type":"thinking_delta","thinking":"private"}})
        ),
        named("content_block_stop", json!({"index":7}))
    );
    let initial = named("message_start", json!({"message":{"content":[]}}));
    let anthropic = anthropic.replacen(&initial, &format!("{initial}{thinking}"), 1);
    let output = decode(HttpProvider::Anthropic, &anthropic).unwrap();
    assert!(
        matches!(&output[..],[ModelChunk::Text(text),ModelChunk::Finish{..}] if text=="# Answer")
    );
}

#[tokio::test]
async fn unsupported_content_types_and_retry_delays_fail_without_unbounded_reads_or_retries() {
    let mut stalled_error = Reply::status(500);
    stalled_error.chunks = vec![(Duration::from_secs(20), b"host-secret".to_vec())];
    let server = Server::start(vec![stalled_error]).await;
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(2),
            collect(&server.model(HttpProvider::OpenAi))
        )
        .await
        .unwrap(),
        Err(ProcessingJobFailure::ProviderFailed)
    ));
    server.close().await;
    for header in ["120", "Wed, 11 Nov 2026 10:00:00 GMT", "invalid"] {
        let mut reply = Reply::status(429);
        reply.headers.push(("retry-after", header.into()));
        let server = Server::start(vec![reply]).await;
        assert!(matches!(
            collect(&server.model(HttpProvider::OpenAi)).await,
            Err(ProcessingJobFailure::ProviderUnavailable)
        ));
        assert_eq!(server.state.requests.lock().unwrap().len(), 1);
        server.close().await;
    }
    let mut reply = Reply::sse(success(HttpProvider::OpenAi, "# Answer"));
    reply.headers = vec![("content-type", "application/json".into())];
    let server = Server::start(vec![reply]).await;
    assert!(matches!(
        collect(&server.model(HttpProvider::OpenAi)).await,
        Err(ProcessingJobFailure::ProviderFailed)
    ));
    server.close().await;
}

#[test]
fn explicit_length_completion_is_distinct_from_interrupted_eof() {
    for (provider, body) in [
        (
            HttpProvider::ChatCompletions,
            format!(
                "{}{}data: [DONE]\n\n",
                chat_text("# Partial"),
                chat_finish("length")
            ),
        ),
        (
            HttpProvider::OpenAi,
            format!(
                "{}{}",
                named("response.output_text.delta", json!({"delta":"# Partial"})),
                named(
                    "response.incomplete",
                    json!({"response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}})
                )
            ),
        ),
        (
            HttpProvider::Google,
            event(
                json!({"candidates":[{"content":{"parts":[{"text":"# Partial"}]},"finishReason":"MAX_TOKENS"}]}),
            ),
        ),
        (
            HttpProvider::Anthropic,
            success(HttpProvider::Anthropic, "# Partial").replace("end_turn", "max_tokens"),
        ),
    ] {
        let output = decode(provider, &body).unwrap();
        assert!(matches!(
            output.last(),
            Some(ModelChunk::Finish { truncated: true })
        ));
    }
}

#[test]
fn semantic_failures_refusals_tools_wrong_choices_and_trailing_output_never_succeed() {
    for (provider,body) in [
        (HttpProvider::ChatCompletions,"data: [DONE]\n\n".into()),
        (HttpProvider::ChatCompletions,format!("{}{}",chat_text("# Partial"),chat_finish("content_filter"))),
        (HttpProvider::ChatCompletions,event(json!({"choices":[{"index":1,"delta":{"content":"wrong"},"finish_reason":"stop"}]}))),
        (HttpProvider::ChatCompletions,event(json!({"choices":[{"index":0,"delta":{"refusal":"No"},"finish_reason":"stop"}]}))),
        (HttpProvider::ChatCompletions,event(json!({"choices":[{"index":0,"delta":{"tool_calls":[{}]},"finish_reason":"stop"}]}))),
        (HttpProvider::ChatCompletions,format!("{}{}",success(HttpProvider::ChatCompletions,"# Done"),chat_text("late"))),
        (HttpProvider::OpenAi,named("response.failed",json!({"response":{"error":{"message":"host-secret"}}}))),
        (HttpProvider::OpenAi,named("response.refusal.delta",json!({"delta":"No"}))),
        (HttpProvider::OpenAi,named("response.incomplete",json!({"response":{"status":"incomplete","incomplete_details":{"reason":"content_filter"}}}))),
        (HttpProvider::OpenAi,named("response.completed",json!({"response":{"status":"completed","output":[{"type":"message","content":[{"type":"refusal","refusal":"No"}]}]}}))),
        (HttpProvider::OpenAi,named("response.output_item.added",json!({"item":{"type":"function_call"}}))),
        (HttpProvider::OpenAi,format!("{}{}",success(HttpProvider::OpenAi,"# Done"),named("response.output_text.delta",json!({"delta":"late"})))),
        (HttpProvider::OpenAi,"event: response.completed\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"confused\"}\n\n".into()),
        (HttpProvider::Anthropic,success(HttpProvider::Anthropic,"# Partial").replace("end_turn","refusal")),
        (HttpProvider::Anthropic,named("message_stop",json!({}))),
        (HttpProvider::Anthropic,named("error",json!({"error":{"message":"host-secret"}}))),
        (HttpProvider::Google,event(json!({"candidates":[{"content":{"parts":[{"text":"# Partial"}]},"finishReason":"SAFETY"}]}))),
        (HttpProvider::Google,event(json!({"promptFeedback":{"blockReason":"SAFETY"}}))),
        (HttpProvider::Google,event(json!({"candidates":[{"content":{"parts":[{"functionCall":{}}]},"finishReason":"STOP"}]}))),
    ] {
        assert!(matches!(decode(provider,&body),Err(ProcessingJobFailure::ProviderFailed)),"accepted {body}");
    }
}

struct Allow;
impl JobAuthorizer for Allow {
    fn authorize<'a>(
        &'a self,
        _: &'a JobWork,
    ) -> JobFuture<'a, Result<Box<dyn Send>, ProcessingJobFailure>> {
        Box::pin(async { Ok(Box::new(()) as Box<dyn Send>) })
    }
}

struct Prepared;
impl SummarySource for Prepared {
    fn prepare(
        &self,
        _: JobWork,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<SummaryPrompt, ProcessingJobFailure>> {
        Box::pin(async {
            Ok(SummaryPrompt {
                system: "Summarize faithfully.".into(),
                prompt: "Ship Friday.".into(),
                images: vec![],
                custom_prompt: false,
            })
        })
    }
}

#[tokio::test]
async fn early_heading_validation_cancels_the_old_http_stream_before_explicit_feedback_retry() {
    let provider = HttpProvider::OpenAi;
    let mut initial = Reply::sse(named(
        "response.output_text.delta",
        json!({"delta":"Invalid opening without heading."}),
    ));
    initial.chunks.push((
        Duration::from_secs(20),
        named(
            "response.completed",
            json!({"response":{"status":"completed"}}),
        )
        .into_bytes(),
    ));
    let server = Server::start(vec![
        initial,
        Reply::sse(success(provider, "# Decisions\n- Ship Friday.")),
    ])
    .await;
    let generator = SummaryGenerator::new(Arc::new(Prepared), Arc::new(server.model(provider)));
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        generator.generate(
            JobWork {
                principal: "device".into(),
                request: hypr_loofah_protocol::SummaryJobRequest {
                    session_id: "note".into(),
                    target: hypr_loofah_protocol::DocumentTarget::Summary,
                    expected_revision: "revision".into(),
                },
            },
            CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.markdown, "# Decisions\n- Ship Friday.");
    {
        let requests = server.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1]
                .body
                .to_string()
                .contains("Previous attempt failed.")
        );
    }
    server.close().await;
}

#[test]
fn cumulative_text_is_bounded_across_small_valid_events() {
    let mut wire = wire::Wire::new(HttpProvider::ChatCompletions);
    let data=json!({"choices":[{"index":0,"delta":{"content":"a".repeat(64*1024)},"finish_reason":null}]}).to_string();
    for _ in 0..64 {
        wire.event(sse::Event {
            name: String::new(),
            data: data.clone(),
        })
        .unwrap();
    }
    assert_eq!(
        wire.event(sse::Event {
            name: String::new(),
            data
        })
        .err(),
        Some(ProcessingJobFailure::ResourceLimit)
    );
}

#[tokio::test]
async fn headless_worker_uses_canonical_source_and_http_model_and_preserves_files_on_disconnect() {
    use hypr_loofah_protocol::{DocumentTarget, ProcessingJobState, SummaryJobRequest};
    for interrupted in [false, true] {
        let provider = HttpProvider::OpenAi;
        let body = if interrupted {
            named("response.output_text.delta", json!({"delta":"# Partial"}))
        } else {
            success(
                provider,
                "# Release plan\n- Ship Friday.<loofah-tags>{\"tags\":[]}</loofah-tags>",
            )
        };
        let mut replies = vec![Reply::sse(body)];
        if interrupted {
            replies.push(Reply::sse(success(
                provider,
                "# Recovered\n- Retry explicitly.",
            )));
        }
        let server = Server::start(replies).await;
        let root = tempfile::tempdir().unwrap();
        let mut runtime = crate::VaultRuntime::open_with_changes(
            "vault-one".into(),
            &root.path().join("vault"),
            &root.path().join("cache"),
            &root.path().join("operations"),
            tokio::sync::mpsc::unbounded_channel().1,
        )
        .await
        .unwrap();
        let meta = serde_json::from_value(
            json!({"id":"note","title":"Release","created_at":"2026-11-07T10:00:00Z","tags":[]}),
        )
        .unwrap();
        runtime
            .context
            .store
            .create_note_session(&meta, "Ship Friday. Preserve this source note.")
            .await
            .unwrap();
        runtime.context.store.ensure_summary("note").await.unwrap();
        let note_path = root.path().join("vault/sessions/note/notes.md");
        let before = std::fs::read(&note_path).unwrap();
        let source = Arc::new(VaultSummarySource::new(
            &runtime.context,
            SummaryIdentity::LocalOwner,
            false,
        ));
        runtime
            .enable_processing(
                Arc::new(SummaryGenerator::new(
                    source,
                    Arc::new(server.model(provider)),
                )),
                Arc::new(Allow),
            )
            .await
            .unwrap();
        let baseline = runtime
            .context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap();
        let journal = runtime.context.processing_jobs().unwrap();
        let job = journal
            .submit(
                "device".into(),
                uuid::Uuid::new_v4().to_string(),
                SummaryJobRequest {
                    session_id: "note".into(),
                    target: DocumentTarget::Summary,
                    expected_revision: baseline.revision,
                },
            )
            .await
            .unwrap();
        let final_job = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = journal.get(job.id.clone()).await.unwrap().unwrap();
                if snapshot.state.is_terminal() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let result = runtime
            .context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap();
        if interrupted {
            assert_eq!(final_job.state, ProcessingJobState::Interrupted);
            assert_eq!(final_job.failure, Some(ProcessingJobFailure::Interrupted));
            assert_eq!(result.document, baseline.document);
        } else {
            assert_eq!(final_job.state, ProcessingJobState::Succeeded);
            assert_eq!(
                result.document.unwrap().markdown,
                "# Release plan\n- Ship Friday."
            );
        }
        assert_eq!(std::fs::read(note_path).unwrap(), before);
        {
            let requests = server.state.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert!(
                requests[0]
                    .body
                    .to_string()
                    .contains("Ship Friday. Preserve this source note.")
            );
        }
        if interrupted {
            let retry = journal
                .submit(
                    "device".into(),
                    uuid::Uuid::new_v4().to_string(),
                    SummaryJobRequest {
                        session_id: "note".into(),
                        target: DocumentTarget::Summary,
                        expected_revision: result.revision,
                    },
                )
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let snapshot = journal.get(retry.id.clone()).await.unwrap().unwrap();
                    if snapshot.state.is_terminal() {
                        assert_eq!(snapshot.state, ProcessingJobState::Succeeded);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        }
        runtime.shutdown().await.unwrap();
        server.close().await;
    }
}

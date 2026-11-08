use super::*;
use axum::{Extension, extract::Path};

const VAULT: &str = "6114bd31-62da-4d90-9a0d-41b9a8872210";
const CAPTURE: &str = "6114bd31-62da-4d90-9a0d-41b9a8872211";
const GENERATION: &str = "6114bd31-62da-4d90-9a0d-41b9a8872212";
struct Live {
    source: Mutex<Option<CaptureIngestStatus>>,
    mode: AtomicUsize,
    metadata: AtomicUsize,
    connections: AtomicUsize,
    cancels: AtomicUsize,
    cursors: Mutex<Vec<String>>,
}
impl Live {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            source: Mutex::new(None),
            mode: AtomicUsize::new(0),
            metadata: AtomicUsize::new(0),
            connections: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
            cursors: Mutex::new(Vec::new()),
        })
    }
    fn routes(self: &Arc<Self>) -> Router<Arc<Provider>> {
        Router::new()
            .route(
                &format!("/api/v1/vaults/{VAULT}/transcription/policy"),
                get(policy),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/captures/{CAPTURE}"),
                get(metadata),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/captures/{CAPTURE}/live"),
                post(start),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/captures/{CAPTURE}/live/{{generation}}"),
                get(status),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/captures/{CAPTURE}/live/{{generation}}/captions"),
                get(captions),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/captures/{CAPTURE}/live/{{generation}}/result"),
                get(|| async { Json(Value::Null) }),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/captures/{CAPTURE}/live/{{generation}}/cancel"),
                post(cancel),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/captures/{CAPTURE}/live/{{generation}}/events"),
                get(stream),
            )
            .layer(Extension(self.clone()))
    }
    async fn configure(&self, server: &str) -> CaptureLiveBinding {
        let identity = CaptureIdentity {
            scope: CaptureScope {
                server: server.into(),
                member_id: "member".into(),
                vault_id: VAULT.into(),
            },
            capture_id: CAPTURE.into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            target: CaptureTarget::ExistingSession {
                session_id: "note".into(),
                expected_audio_sha256: None,
            },
        };
        let format = CaptureFormat::stereo();
        let fingerprint = capture_fingerprint(&identity, &format);
        *self.source.lock().await = Some(CaptureIngestStatus {
            identity,
            format: format.clone(),
            prefix: CapturePrefixReceipt {
                capture_fingerprint: fingerprint.clone(),
                chunk_count: 0,
                frames: 0,
                pcm_bytes: 0,
                chain_digest: capture_chain_seed(&fingerprint),
            },
            seal: None,
            first_admitted_audio_observed_at_ms: 1234,
            session_id: Some("note".into()),
            publication: None,
        });
        CaptureLiveBinding {
            capture_id: CAPTURE.into(),
            generation_id: GENERATION.into(),
            capture_fingerprint: fingerprint,
            session_id: "note".into(),
            format,
        }
    }
    async fn snapshot(&self, sequence: u64, state: &str) -> Value {
        json!({"capture_id":CAPTURE,"generation_id":GENERATION,"capture_fingerprint":self.source.lock().await.as_ref().unwrap().prefix.capture_fingerprint,"session_id":"note","state":state,"failure":null,"admitted_end_frame":32000,"input_consumed_end_frame":16000,"completed_through_frame":null,"next_caption_sequence":sequence,"result_available":false,"publication":null})
    }
    async fn page(&self, after: Option<u64>, terminal: bool) -> Value {
        let sequence = after.map_or(0, |value| value + 1);
        let snapshot = self
            .snapshot(sequence + 1, if terminal { "cancelled" } else { "running" })
            .await;
        json!({"snapshot":snapshot,"captions":[{"generation_id":GENERATION,"sequence":sequence,"physical_channel":0,"role":"microphone","new_words":[{"id":format!("word{sequence}"),"text":" hello","start_ms":0,"end_ms":1000,"channel":0,"state":"final"}],"partials":[{"text":" next","start_ms":1000,"end_ms":1100,"channel":0}],"replaced_ids":[],"languages":["en"]}],"next_cursor":null})
    }
}
fn check(headers: &HeaderMap) {
    assert!(headers.contains_key("authorization"));
    assert_eq!(headers["x-loofah-api-major"], "1");
}
async fn metadata(
    Extension(state): Extension<Arc<Live>>,
    headers: HeaderMap,
) -> Json<CaptureIngestStatus> {
    check(&headers);
    state.metadata.fetch_add(1, Ordering::SeqCst);
    Json(state.source.lock().await.clone().unwrap())
}
async fn start(
    Extension(state): Extension<Arc<Live>>,
    headers: HeaderMap,
    Json(_input): Json<CaptureLiveRequest>,
) -> Json<Value> {
    check(&headers);
    assert_eq!(headers["x-request-id"], GENERATION);
    Json(state.snapshot(0, "starting").await)
}
async fn status(
    Extension(state): Extension<Arc<Live>>,
    Path(generation): Path<String>,
    headers: HeaderMap,
) -> Json<Value> {
    check(&headers);
    assert_eq!(generation, GENERATION);
    let mut value = state.snapshot(0, "running").await;
    match state.mode.load(Ordering::SeqCst) {
        1 => value["capture_id"] = json!(uuid::Uuid::new_v4().to_string()),
        2 => value["capture_fingerprint"] = json!("a".repeat(64)),
        3 => value["session_id"] = json!("other"),
        4 => value["input_consumed_end_frame"] = json!(32001),
        5 => value["state"] = json!("completed"),
        _ => {}
    }
    Json(value)
}
async fn captions(
    Extension(state): Extension<Arc<Live>>,
    Query(input): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Json<Value> {
    check(&headers);
    let after = input.get("after").map(|value| value.parse().unwrap());
    let mut page = state.page(after, false).await;
    match state.mode.load(Ordering::SeqCst) {
        6 => page["captions"][0]["sequence"] = json!(5),
        7 => page["captions"][0]["role"] = json!("system_audio"),
        8 => page["captions"][0]["new_words"][0]["state"] = json!("pending"),
        9 => page["next_cursor"] = json!(1),
        _ => {}
    }
    Json(page)
}
async fn cancel(Extension(state): Extension<Arc<Live>>, headers: HeaderMap) -> Json<Value> {
    check(&headers);
    state.cancels.fetch_add(1, Ordering::SeqCst);
    Json(state.snapshot(0, "cancelling").await)
}
async fn stream(
    Extension(state): Extension<Arc<Live>>,
    headers: HeaderMap,
) -> axum::response::Response {
    check(&headers);
    let index = state.connections.fetch_add(1, Ordering::SeqCst);
    let cursor = headers["last-event-id"].to_str().unwrap().to_owned();
    state.cursors.lock().await.push(cursor.clone());
    let values = if index == 0 {
        assert_eq!(cursor, format!("{GENERATION}:initial"));
        let first = state.page(None, false).await;
        let mut second = first.clone();
        second["captions"] = json!([]);
        second["snapshot"]["input_consumed_end_frame"] = json!(32000);
        vec![
            Event::default()
                .id(format!("{GENERATION}:0"))
                .event("page")
                .data(first.to_string()),
            Event::default()
                .id(format!("{GENERATION}:0"))
                .event("page")
                .data(second.to_string()),
        ]
    } else {
        assert_eq!(cursor, format!("{GENERATION}:0"));
        vec![
            Event::default()
                .id(format!("{GENERATION}:1"))
                .event("page")
                .data(state.page(Some(0), true).await.to_string()),
        ]
    };
    Sse::new(futures_util::stream::iter(
        values.into_iter().map(Ok::<_, std::convert::Infallible>),
    ))
    .into_response()
}
fn enable(client: &mut Arc<RemoteClient>) {
    Arc::get_mut(client).unwrap().capabilities.features.extend([
        "capture_ingestion".into(),
        "capture_live_transcription".into(),
    ]);
}
fn input(binding: &CaptureLiveBinding) -> CaptureLiveRequest {
    CaptureLiveRequest {
        capture_fingerprint: binding.capture_fingerprint.clone(),
        expected_transcript: AbsentTranscriptExpectation::Absent,
        model: "cpu-base".into(),
        languages: vec!["en".into()],
        keywords: vec![],
    }
}
fn operations(binding: &CaptureLiveBinding) -> Vec<RemoteOperation> {
    vec![
        RemoteOperation::StartCaptureLive {
            vault_id: VAULT.into(),
            binding: binding.clone(),
            input: input(binding),
        },
        RemoteOperation::CaptureLiveStatus {
            vault_id: VAULT.into(),
            binding: binding.clone(),
        },
        RemoteOperation::CaptureLiveCaptions {
            vault_id: VAULT.into(),
            binding: binding.clone(),
            after: None,
            limit: 128,
        },
        RemoteOperation::CaptureLiveResult {
            vault_id: VAULT.into(),
            binding: binding.clone(),
        },
        RemoteOperation::CancelCaptureLive {
            vault_id: VAULT.into(),
            binding: binding.clone(),
        },
    ]
}
async fn execute(client: &Arc<RemoteClient>, operation: RemoteOperation) -> Result<RemoteReply> {
    client
        .execute(operation, GENERATION, CancellationToken::new())
        .await
}

#[tokio::test]
async fn live_requests_and_subscription_gate_and_bind_before_credentials() {
    let state = Live::new();
    let fixture = Fixture::with_extra_routes(None, state.routes()).await;
    let mut client = fixture.client().await;
    let binding = state.configure(client.address()).await;
    let loads = fixture.credentials.loads.load(Ordering::SeqCst);
    for operation in operations(&binding) {
        assert_eq!(
            execute(&client, operation).await.unwrap_err().code,
            ErrorCode::IncompatibleVersion
        );
    }
    assert!(
        client
            .subscribe_capture_live(VAULT.into(), binding.clone(), None, |_| true)
            .is_err()
    );
    enable(&mut client);
    let mut malformed = binding.clone();
    malformed.generation_id = "../other".into();
    for operation in operations(&malformed) {
        assert!(execute(&client, operation).await.is_err());
    }
    assert!(
        client
            .subscribe_capture_live(VAULT.into(), malformed, None, |_| true)
            .is_err()
    );
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), loads);
}

#[tokio::test]
async fn live_sdk_validates_private_snapshots_durable_prefix_and_protocol_channel_rules() {
    let state = Live::new();
    let fixture = Fixture::with_extra_routes(None, state.routes()).await;
    let mut client = fixture.client().await;
    enable(&mut client);
    let binding = state.configure(client.address()).await;
    fixture.login(&client).await;
    for operation in operations(&binding) {
        execute(&client, operation).await.unwrap();
    }
    for mode in 1..=9 {
        state.mode.store(mode, Ordering::SeqCst);
        let operation = operations(&binding).remove(if mode < 6 { 1 } else { 2 });
        assert!(execute(&client, operation).await.is_err(), "mode{mode}");
    }
    assert_eq!(state.cancels.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn live_https_stream_reconnects_exact_prefix_keeps_state_only_pages_and_never_cancels_worker()
{
    for millis in [50, 150, 500] {
        let state = Live::new();
        let fixture =
            Fixture::with_extra_routes(Some(Duration::from_millis(millis)), state.routes()).await;
        let mut client = fixture.client().await;
        enable(&mut client);
        let binding = state.configure(client.address()).await;
        fixture.login(&client).await;
        let (send, mut recv) = mpsc::unbounded_channel();
        let subscription = client
            .subscribe_capture_live(VAULT.into(), binding, None, move |event| {
                send.send(event).is_ok()
            })
            .unwrap();
        let mut pages = Vec::new();
        tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(event) = recv.recv().await {
                match event {
                    CaptureLiveEvent::Page { page } => pages.push(page),
                    CaptureLiveEvent::Connection {
                        state: ConnectionState::Closed,
                        ..
                    } => break,
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        subscription.close().await;
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0].captions[0].sequence, 0);
        assert!(pages[1].captions.is_empty());
        assert_eq!(pages[2].captions[0].sequence, 1);
        assert_eq!(state.metadata.load(Ordering::SeqCst), 1);
        assert_eq!(state.cancels.load(Ordering::SeqCst), 0);
        assert_eq!(state.connections.load(Ordering::SeqCst), 2);
    }
}

async fn policy(Extension(state): Extension<Arc<Live>>, headers: HeaderMap) -> Json<Value> {
    check(&headers);
    let mut value = json!({"model":"operator-cpu-base","availability":"ready","batch_enabled":true,"live_enabled":true});
    match state.mode.load(Ordering::SeqCst) {
        10 => value["model_path"] = json!("/private/model.bin"),
        11 => value["model"] = json!(" alias"),
        12 => value["availability"] = json!("disabled"),
        13 => value["model"] = Value::Null,
        14 => value["availability"] = json!("unavailable"),
        15 => value["model"] = json!("x".repeat(513)),
        _ => {}
    }
    Json(value)
}
#[tokio::test]
async fn transcription_policy_gates_before_credentials_and_validates_public_model_metadata() {
    let state = Live::new();
    let fixture = Fixture::with_extra_routes(None, state.routes()).await;
    let mut client = fixture.client().await;
    let operation = || RemoteOperation::TranscriptionPolicy {
        vault_id: VAULT.into(),
    };
    let loads = fixture.credentials.loads.load(Ordering::SeqCst);
    assert_eq!(
        execute(&client, operation()).await.unwrap_err().code,
        ErrorCode::IncompatibleVersion
    );
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), loads);
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("transcription_policy".into());
    fixture.login(&client).await;
    let RemoteReply::TranscriptionPolicy(value) = execute(&client, operation()).await.unwrap()
    else {
        panic!("Expected policy")
    };
    assert_eq!(value.model.as_deref(), Some("operator-cpu-base"));
    for mode in [10, 11, 12, 13, 15] {
        state.mode.store(mode, Ordering::SeqCst);
        assert!(execute(&client, operation()).await.is_err(), "mode{mode}");
    }
    state.mode.store(14, Ordering::SeqCst);
    let RemoteReply::TranscriptionPolicy(value) = execute(&client, operation()).await.unwrap()
    else {
        panic!("Expected policy")
    };
    assert_eq!(value.availability, TranscriptionAvailability::Unavailable);
}

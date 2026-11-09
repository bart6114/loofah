use super::*;
use axum::Extension;

const VAULT: &str = "5d350bbe-aa5f-4949-b07e-261125e40710";
const CAPTURE: &str = "5d350bbe-aa5f-4949-b07e-261125e40711";
const INTENT: &str = "5d350bbe-aa5f-4949-b07e-261125e40712";
const MEMBER: &str = "5d350bbe-aa5f-4949-b07e-261125e40713";

struct Processing {
    mode: AtomicUsize,
    calls: AtomicUsize,
    declarations: AtomicUsize,
    cancel_calls: AtomicUsize,
    delay: AtomicUsize,
    snapshot: Mutex<Option<Value>>,
    admitted: tokio::sync::Notify,
}
impl Processing {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            mode: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            declarations: AtomicUsize::new(0),
            cancel_calls: AtomicUsize::new(0),
            delay: AtomicUsize::new(0),
            snapshot: Mutex::new(None),
            admitted: tokio::sync::Notify::new(),
        })
    }
    fn routes(self: &Arc<Self>) -> Router<Arc<Provider>> {
        Router::new()
            .route(
                &format!("/api/v1/vaults/{VAULT}/captures/{CAPTURE}/processing"),
                get(status).put(declare),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/captures/{CAPTURE}/processing/cancel"),
                post(cancel),
            )
            .layer(Extension(self.clone()))
    }
    async fn reply(&self) -> Value {
        let mut value = self.snapshot.lock().await.clone().unwrap();
        match self.mode.load(Ordering::SeqCst) {
            1 => value["capture_id"] = json!(uuid::Uuid::new_v4().to_string()),
            2 => value["capture_fingerprint"] = json!("b".repeat(64)),
            3 => value["intent_id"] = json!(uuid::Uuid::new_v4().to_string()),
            4 => value["request"]["languages"] = json!(["nl"]),
            5 => value["model"] = json!("different-model"),
            6 => value["model"] = json!(" alias"),
            7 => value["session_id"] = json!("another-note"),
            8 => value["live_generation_id"] = json!(uuid::Uuid::new_v4().to_string()),
            9 => value["repair_job_id"] = json!("not-a-job"),
            10 => value["summary_job_id"] = json!(uuid::Uuid::nil().to_string()),
            11 => {
                value["transcript_publication"] = json!({"revision":"bad","transcript_id":INTENT})
            }
            12 => value["state"] = json!("succeeded"),
            13 => value["created_at"] = json!("\n"),
            14 => value["model"] = json!("x".repeat(CAPTURE_PROCESSING_RESPONSE_LIMIT)),
            15 => value["resolved_model"] = json!("must-not-be-used"),
            16 => value["session_id"] = json!("../outside"),
            17 => value["request"]["summary"]["title_policy"] = json!("preserve"),
            _ => {}
        }
        value
    }
}
fn identity(server: &str) -> CaptureIdentity {
    CaptureIdentity {
        scope: CaptureScope {
            server: server.into(),
            vault_id: VAULT.into(),
            member_id: MEMBER.into(),
        },
        capture_id: CAPTURE.into(),
        request_id: INTENT.into(),
        target: CaptureTarget::ExistingSession {
            session_id: "note".into(),
            expected_audio_sha256: None,
        },
    }
}
fn input() -> CaptureProcessingRequest {
    CaptureProcessingRequest {
        capture_fingerprint: "a".repeat(64),
        model: Some("operator-cpu-base".into()),
        languages: vec!["en".into()],
        keywords: vec!["Loofah".into()],
        summary: Some(CaptureProcessingSummary {
            target: DocumentTarget::Summary,
            title_policy: TitlePolicy::GenerateIfEmpty,
        }),
    }
}
async fn configure(state: &Processing, input: &CaptureProcessingRequest) {
    *state.snapshot.lock().await = Some(json!({
        "capture_id":CAPTURE,"capture_fingerprint":input.capture_fingerprint,"intent_id":INTENT,
        "session_id":"note","model":"operator-cpu-base","request":input,"state":"waiting",
        "live_generation_id":null,"repair_job_id":null,"summary_job_id":null,"transcript_publication":null,
        "failure":null,"created_at":"2026-11-08T10:00:00Z","updated_at":"2026-11-08T10:00:00Z"
    }));
}
fn check(headers: &HeaderMap) {
    assert!(headers.contains_key("authorization"));
    assert_eq!(headers["x-request-id"], INTENT);
    assert_eq!(headers["x-loofah-api-major"], "1");
}
async fn declare(
    Extension(state): Extension<Arc<Processing>>,
    headers: HeaderMap,
    Json(input): Json<CaptureProcessingRequest>,
) -> Json<Value> {
    check(&headers);
    let call = state.calls.fetch_add(1, Ordering::SeqCst);
    assert_eq!(
        state.snapshot.lock().await.as_ref().unwrap()["request"],
        json!(input)
    );
    if call == 0 {
        state.declarations.fetch_add(1, Ordering::SeqCst);
    }
    state.admitted.notify_one();
    tokio::time::sleep(Duration::from_millis(
        state.delay.load(Ordering::SeqCst) as u64
    ))
    .await;
    Json(state.reply().await)
}
async fn status(Extension(state): Extension<Arc<Processing>>, headers: HeaderMap) -> Json<Value> {
    check(&headers);
    Json(state.reply().await)
}
async fn cancel(Extension(state): Extension<Arc<Processing>>, headers: HeaderMap) -> Json<Value> {
    check(&headers);
    state.cancel_calls.fetch_add(1, Ordering::SeqCst);
    Json(state.reply().await)
}
fn enable(client: &mut Arc<RemoteClient>) {
    Arc::get_mut(client)
        .unwrap()
        .capabilities
        .features
        .push("capture_processing".into());
}
fn operations(
    identity: &CaptureIdentity,
    input: &CaptureProcessingRequest,
) -> Vec<RemoteOperation> {
    vec![
        RemoteOperation::DeclareCaptureProcessing {
            identity: identity.clone(),
            input: input.clone(),
        },
        RemoteOperation::CaptureProcessingStatus {
            identity: identity.clone(),
            input: input.clone(),
        },
        RemoteOperation::CancelCaptureProcessing {
            identity: identity.clone(),
            input: input.clone(),
        },
    ]
}

#[tokio::test]
async fn processing_sdk_validates_capability_owner_request_and_cancellation_before_credentials() {
    let state = Processing::new();
    let fixture = Fixture::with_extra_routes(None, state.routes()).await;
    let mut client = fixture.client().await;
    let identity = identity(client.address());
    let input = input();
    let loads = fixture.credentials.loads.load(Ordering::SeqCst);
    for (index, operation) in operations(&identity, &input).into_iter().enumerate() {
        let error = client
            .execute(operation, INTENT, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::IncompatibleVersion);
        assert_eq!(
            error.mutation_outcome,
            (index != 1).then_some(MutationOutcome::NotCommitted)
        );
    }
    enable(&mut client);
    for mode in 0..7 {
        let mut owner = identity.clone();
        let mut request = input.clone();
        match mode {
            0 => owner.scope.server = "https://another.example/".into(),
            1 => owner.scope.member_id = "another-member".into(),
            2 => owner.scope.vault_id = "../vault".into(),
            3 => owner.capture_id = uuid::Uuid::nil().to_string(),
            4 => owner.request_id = "not-an-intent".into(),
            5 => request.capture_fingerprint = "bad".into(),
            6 => {
                owner.target = CaptureTarget::ExistingSession {
                    session_id: "../note".into(),
                    expected_audio_sha256: None,
                }
            }
            _ => unreachable!(),
        }
        for operation in operations(&owner, &request) {
            assert_eq!(
                client
                    .execute(operation, INTENT, CancellationToken::new())
                    .await
                    .unwrap_err()
                    .code,
                ErrorCode::Validation
            );
        }
    }
    let error = client
        .execute(
            operations(&identity, &input).remove(0),
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
    for (index, operation) in operations(&identity, &input).into_iter().enumerate() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let error = client
            .execute(operation, INTENT, cancellation)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Cancelled);
        assert_eq!(
            error.mutation_outcome,
            (index != 1).then_some(MutationOutcome::NotCommitted)
        );
    }
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), loads);
}

#[tokio::test]
async fn processing_sdk_binds_all_snapshots_and_rejects_malformed_or_oversize_https_replies() {
    let state = Processing::new();
    let fixture =
        Fixture::with_extra_routes(Some(Duration::from_millis(150)), state.routes()).await;
    let mut client = fixture.client().await;
    enable(&mut client);
    let owner = identity(client.address());
    let request = input();
    configure(&state, &request).await;
    fixture.login(&client).await;
    client
        .declare_capture_processing(&owner, &request, CancellationToken::new())
        .await
        .unwrap();
    client
        .capture_processing_status(&owner, &request, CancellationToken::new())
        .await
        .unwrap();
    client
        .cancel_capture_processing(&owner, &request, CancellationToken::new())
        .await
        .unwrap();
    for mode in 1..=17 {
        state.mode.store(mode, Ordering::SeqCst);
        for operation in operations(&owner, &request) {
            let write = !matches!(operation, RemoteOperation::CaptureProcessingStatus { .. });
            let error = client
                .execute(operation, INTENT, CancellationToken::new())
                .await
                .unwrap_err();
            assert!(
                matches!(
                    error.code,
                    ErrorCode::IncompatibleVersion | ErrorCode::ResourceLimit
                ),
                "mode{mode}: {error:?}"
            );
            assert_eq!(
                error.mutation_outcome,
                write.then_some(MutationOutcome::Unknown)
            );
        }
    }
}

#[tokio::test]
async fn processing_sdk_accepts_operator_resolution_and_portable_pending_session_publication() {
    let state = Processing::new();
    let fixture = Fixture::with_extra_routes(None, state.routes()).await;
    let mut client = fixture.client().await;
    enable(&mut client);
    let mut owner = identity(client.address());
    owner.target = CaptureTarget::PendingSession {
        create_request_id: uuid::Uuid::new_v4().to_string(),
    };
    let mut request = input();
    request.model = None;
    configure(&state, &request).await;
    state.snapshot.lock().await.as_mut().unwrap()["session_id"] = Value::Null;
    fixture.login(&client).await;
    assert_eq!(
        client
            .declare_capture_processing(&owner, &request, CancellationToken::new())
            .await
            .unwrap()
            .session_id,
        None
    );
    {
        let mut value = state.snapshot.lock().await;
        let value = value.as_mut().unwrap();
        value["session_id"] = json!("assigned-note");
        value["state"] = json!("succeeded");
        value["summary_job_id"] = json!(uuid::Uuid::new_v4().to_string());
        value["transcript_publication"] =
            json!({"transcript_id":uuid::Uuid::new_v4().to_string(),"revision":"c".repeat(64)});
    }
    let value = client
        .capture_processing_status(&owner, &request, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(value.model, "operator-cpu-base");
    assert_eq!(value.session_id.as_deref(), Some("assigned-note"));
}

#[tokio::test]
async fn processing_sdk_lost_ack_never_retries_and_explicit_replay_preserves_original_intent_over_real_latency()
 {
    for millis in [50, 150, 500] {
        let state = Processing::new();
        let fixture =
            Fixture::with_extra_routes(Some(Duration::from_millis(millis)), state.routes()).await;
        let mut client = fixture.client().await;
        enable(&mut client);
        let owner = identity(client.address());
        let request = input();
        configure(&state, &request).await;
        fixture.login(&client).await;
        state.delay.store(1_000, Ordering::SeqCst);
        let cancellation = CancellationToken::new();
        let task = tokio::spawn({
            let client = client.clone();
            let owner = owner.clone();
            let request = request.clone();
            let cancellation = cancellation.clone();
            async move {
                client
                    .declare_capture_processing(&owner, &request, cancellation)
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(10), state.admitted.notified())
            .await
            .expect("declaration should reach the fixture before cancellation");
        cancellation.cancel();
        let error = task.await.unwrap().unwrap_err();
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
        assert_eq!(error.request_id.as_deref(), Some(INTENT));
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
        state.delay.store(0, Ordering::SeqCst);
        let retained = client
            .capture_processing_status(&owner, &request, CancellationToken::new())
            .await
            .unwrap();
        let replay = client
            .declare_capture_processing(&owner, &request, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(replay, retained);
        assert_eq!(replay.intent_id, owner.request_id);
        assert_eq!(state.calls.load(Ordering::SeqCst), 2);
        assert_eq!(state.declarations.load(Ordering::SeqCst), 1);
        assert_eq!(state.cancel_calls.load(Ordering::SeqCst), 0);
    }
}

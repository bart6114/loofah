use super::*;
use axum::{Extension, extract::Path};
use std::collections::BTreeMap;
use tokio::sync::Semaphore;

const VAULT: &str = "6114bd31-62da-4d90-9a0d-41b9a8872210";
const JOB: &str = "6114bd31-62da-4d90-9a0d-41b9a8872211";
const MEMBER: &str = "6114bd31-62da-4d90-9a0d-41b9a8872212";
const TRANSCRIPT: &str = "6114bd31-62da-4d90-9a0d-41b9a8872213";

struct Jobs {
    mode: AtomicUsize,
    admitted: Mutex<BTreeMap<String, Value>>,
    calls: AtomicUsize,
    requests: Mutex<Vec<String>>,
    delay: AtomicBool,
    entered: Semaphore,
    finish: Semaphore,
}
impl Jobs {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            mode: AtomicUsize::new(0),
            admitted: Mutex::new(BTreeMap::new()),
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            delay: AtomicBool::new(false),
            entered: Semaphore::new(0),
            finish: Semaphore::new(0),
        })
    }
    fn routes(self: &Arc<Self>) -> Router<Arc<Provider>> {
        Router::new()
            .route(
                &format!("/api/v1/vaults/{VAULT}/transcription/jobs"),
                get(list).post(submit),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/transcription/jobs/{{job}}"),
                get(status),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/transcription/jobs/{{job}}/result"),
                get(result),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/transcription/jobs/{{job}}/cancel"),
                post(action),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/transcription/jobs/{{job}}/resume"),
                post(action),
            )
            .layer(Extension(self.clone()))
    }
}
fn input() -> TranscriptionJobRequest {
    serde_json::from_value(json!({"session_id":"note","expected_recording_sha256":"a".repeat(64),
        "expected_transcript":"absent","model":"cpu-base","languages":["nl"],"keywords":["Loofah"]})).unwrap()
}
fn snapshot() -> Value {
    json!({"id":JOB,"session_id":"note","state":"succeeded","phase":null,
        "created_at":"2026-11-08T10:00:00Z","updated_at":"2026-11-08T10:05:00Z",
        "result_available":true,"failure":null,"publication":{"revision":"b".repeat(64),"transcript_id":TRANSCRIPT}})
}
fn envelope() -> Value {
    json!({"job_id":JOB,"session_id":"note","transcript":{
        "id":TRANSCRIPT,"session_id":"note","user_id":MEMBER,"created_at":"2026-11-08T10:00:00Z",
        "started_at":1234.5,"ended_at":null,"memo_md":"{\"snapshot\":true}",
        "words":[{"id":"word","text":" Hello.","start_ms":0.5,"end_ms":80.5,"channel":2.0,
            "speaker":null,"metadata":{"future":{"preserve":[true,42]}}}],
        "speaker_hints":[{"id":"hint","word_id":"word","type":"speaker_label","value":"Self"}]}})
}
fn check(headers: &HeaderMap) -> String {
    assert_eq!(headers["x-loofah-api-major"], "1");
    assert!(
        headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Bearer ")
    );
    let request = headers["x-request-id"].to_str().unwrap();
    uuid::Uuid::parse_str(request).unwrap();
    request.into()
}
async fn submit(
    Extension(state): Extension<Arc<Jobs>>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> axum::response::Response {
    let request = check(&headers);
    let mut admitted = state.admitted.lock().await;
    if let Some(old) = admitted.get(&request) {
        if old != &input {
            return (
                StatusCode::CONFLICT,
                Json(failure(ErrorCode::Conflict, "Request changed")),
            )
                .into_response();
        }
    } else {
        admitted.insert(request, input);
        state.calls.fetch_add(1, Ordering::SeqCst);
    }
    drop(admitted);
    state.requests.lock().await.push("submit".into());
    state.entered.add_permits(1);
    if state.delay.load(Ordering::SeqCst) {
        state.finish.acquire().await.unwrap().forget();
    }
    reply(&state).into_response()
}
fn reply(state: &Jobs) -> Json<Value> {
    let mut value = snapshot();
    match state.mode.load(Ordering::SeqCst) {
        1 => value["id"] = json!(MEMBER),
        2 => value["session_id"] = json!("wrong-session"),
        3 => value["phase"] = json!("transcribing"),
        4 => value["publication"]["revision"] = json!("invalid"),
        5 => value["unexpected_padding"] = json!("x".repeat(TRANSCRIPTION_SNAPSHOT_LIMIT)),
        _ => {}
    }
    Json(value)
}
async fn status(
    Extension(state): Extension<Arc<Jobs>>,
    Path(job): Path<String>,
    headers: HeaderMap,
) -> Json<Value> {
    check(&headers);
    assert_eq!(job, JOB);
    state.requests.lock().await.push("status".into());
    reply(&state)
}
async fn action(
    Extension(state): Extension<Arc<Jobs>>,
    Path(job): Path<String>,
    headers: HeaderMap,
) -> Json<Value> {
    check(&headers);
    assert_eq!(job, JOB);
    state.requests.lock().await.push("action".into());
    reply(&state)
}
async fn list(
    Extension(state): Extension<Arc<Jobs>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Json<Value> {
    check(&headers);
    state.requests.lock().await.push("list".into());
    let mut value = json!({"jobs":[snapshot()],"next_cursor":null});
    match state.mode.load(Ordering::SeqCst) {
        6 => value["jobs"] = json!([snapshot(), snapshot()]),
        7 => value["next_cursor"] = json!(MEMBER),
        _ => {}
    }
    if let Some(limit) = query.get("limit") {
        assert_eq!(limit, "1");
    }
    Json(value)
}
async fn result(
    Extension(state): Extension<Arc<Jobs>>,
    Path(job): Path<String>,
    headers: HeaderMap,
) -> Json<Value> {
    check(&headers);
    assert_eq!(job, JOB);
    state.requests.lock().await.push("result".into());
    let mut value = envelope();
    match state.mode.load(Ordering::SeqCst) {
        8 => value["job_id"] = json!(MEMBER),
        9 => value["session_id"] = json!("wrong-session"),
        10 => value["transcript"]["words"][0]["channel"] = json!(3),
        11 => value["transcript"]["speaker_hints"][0]["word_id"] = json!("foreign-word"),
        12 => value["transcript"]["ended_at"] = json!(9999),
        13 => value = Value::Null,
        _ => {}
    }
    Json(value)
}
fn operations() -> Vec<RemoteOperation> {
    vec![
        RemoteOperation::SubmitTranscription {
            vault_id: VAULT.into(),
            input: input(),
        },
        RemoteOperation::TranscriptionJobs {
            vault_id: VAULT.into(),
            input: ListTranscriptionJobsInput {
                limit: Some(1),
                after: None,
            },
        },
        RemoteOperation::TranscriptionJob {
            vault_id: VAULT.into(),
            job_id: JOB.into(),
        },
        RemoteOperation::TranscriptionJobResult {
            vault_id: VAULT.into(),
            job_id: JOB.into(),
        },
        RemoteOperation::CancelTranscriptionJob {
            vault_id: VAULT.into(),
            job_id: JOB.into(),
        },
        RemoteOperation::ResumeTranscriptionResult {
            vault_id: VAULT.into(),
            job_id: JOB.into(),
        },
    ]
}
fn enable(client: &mut Arc<RemoteClient>) {
    Arc::get_mut(client)
        .unwrap()
        .capabilities
        .features
        .push("batch_transcription_jobs".into());
}
async fn execute(
    client: &Arc<RemoteClient>,
    operation: RemoteOperation,
) -> crate::Result<RemoteReply> {
    client
        .execute(
            operation,
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
}

#[tokio::test]
async fn every_batch_operation_gates_capability_and_strict_targets_before_credentials() {
    let fixture = Fixture::new().await;
    let mut client = fixture.client().await;
    let loads = fixture.credentials.loads.load(Ordering::SeqCst);
    for operation in operations() {
        let write = matches!(
            operation,
            RemoteOperation::SubmitTranscription { .. }
                | RemoteOperation::CancelTranscriptionJob { .. }
                | RemoteOperation::ResumeTranscriptionResult { .. }
        );
        let error = execute(&client, operation).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::IncompatibleVersion);
        if write {
            assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
        }
    }
    enable(&mut client);
    let mut malformed = input();
    malformed.expected_recording_sha256 = "wrong".into();
    for operation in [
        RemoteOperation::SubmitTranscription {
            vault_id: VAULT.into(),
            input: malformed,
        },
        RemoteOperation::TranscriptionJob {
            vault_id: VAULT.into(),
            job_id: "../private".into(),
        },
        RemoteOperation::CancelTranscriptionJob {
            vault_id: VAULT.into(),
            job_id: uuid::Uuid::nil().to_string(),
        },
        RemoteOperation::TranscriptionJobResult {
            vault_id: "../private".into(),
            job_id: JOB.into(),
        },
        RemoteOperation::TranscriptionJobs {
            vault_id: VAULT.into(),
            input: ListTranscriptionJobsInput {
                limit: Some(201),
                after: None,
            },
        },
    ] {
        assert_eq!(
            execute(&client, operation).await.unwrap_err().code,
            ErrorCode::Validation
        );
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = client
        .execute(
            operations().remove(0),
            &uuid::Uuid::new_v4().to_string(),
            cancel,
        )
        .await
        .unwrap_err();
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), loads);
    assert_eq!(fixture.state.issued.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sdk_validates_bound_snapshots_pages_and_exact_result_metadata() {
    let state = Jobs::new();
    let fixture = Fixture::with_extra_routes(None, state.routes()).await;
    let mut client = fixture.client().await;
    enable(&mut client);
    fixture.login(&client).await;
    let RemoteReply::TranscriptionResult(Some(value)) =
        execute(&client, operations().remove(3)).await.unwrap()
    else {
        panic!("Expected result");
    };
    assert_eq!(serde_json::to_value(value).unwrap(), envelope());
    for mode in 1..=5 {
        state.mode.store(mode, Ordering::SeqCst);
        let operation = if mode == 2 {
            operations().remove(0)
        } else {
            operations().remove(2)
        };
        assert!(
            execute(&client, operation).await.is_err(),
            "snapshot mode {mode}"
        );
    }
    for mode in 6..=7 {
        state.mode.store(mode, Ordering::SeqCst);
        assert!(
            execute(&client, operations().remove(1)).await.is_err(),
            "page mode {mode}"
        );
    }
    for mode in 8..=12 {
        state.mode.store(mode, Ordering::SeqCst);
        assert!(
            execute(&client, operations().remove(3)).await.is_err(),
            "result mode {mode}"
        );
    }
    state.mode.store(13, Ordering::SeqCst);
    assert!(matches!(
        execute(&client, operations().remove(3)).await.unwrap(),
        RemoteReply::TranscriptionResult(None)
    ));
}

#[tokio::test]
async fn lost_reply_is_unknown_and_reuses_exact_request_identity_without_duplicate_admission() {
    let state = Jobs::new();
    state.delay.store(true, Ordering::SeqCst);
    let fixture = Fixture::with_extra_routes(None, state.routes()).await;
    let mut client = fixture.client().await;
    enable(&mut client);
    fixture.login(&client).await;
    let request = uuid::Uuid::new_v4().to_string();
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let client = client.clone();
        let request = request.clone();
        let cancel = cancel.clone();
        async move {
            client
                .execute(operations().remove(0), &request, cancel)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), state.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    cancel.cancel();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
    assert_eq!(error.request_id.as_deref(), Some(request.as_str()));
    state.delay.store(false, Ordering::SeqCst);
    state.finish.add_permits(1);
    let replay = client
        .execute(operations().remove(0), &request, CancellationToken::new())
        .await
        .unwrap();
    assert!(matches!(replay,RemoteReply::TranscriptionJob(job) if job.id==JOB));
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    let mut changed = input();
    changed.model = "changed".into();
    let error = client
        .execute(
            RemoteOperation::SubmitTranscription {
                vault_id: VAULT.into(),
                input: changed,
            },
            &request,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
}

#[tokio::test]
async fn batch_sdk_roundtrips_over_trusted_https_at_50_150_and_500ms() {
    for millis in [50, 150, 500] {
        let state = Jobs::new();
        let fixture =
            Fixture::with_extra_routes(Some(Duration::from_millis(millis)), state.routes()).await;
        let mut client = fixture.client().await;
        enable(&mut client);
        fixture.login(&client).await;
        for operation in operations() {
            execute(&client, operation).await.unwrap();
        }
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.requests.lock().await.as_slice(),
            ["submit", "list", "status", "result", "action", "action"]
        );
        assert!(client.address.as_str().starts_with("https://"));
    }
}

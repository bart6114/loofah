use super::*;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use hypr_capture_store::capture_spool::{
    CaptureFormat, CaptureIdentity, CaptureProcessingOptions, CaptureScope, CaptureSpool,
    CaptureSpoolLimits,
};
use hypr_loofah_client::{ConnectionProfiles, CredentialFuture, CredentialStore};
use hypr_loofah_protocol::{CapturePrefixReceipt, CaptureProcessingRequest};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

const MEMBER: &str = "5d350bbe-aa5f-4949-b07e-261125e40713";
const VAULT: &str = "5d350bbe-aa5f-4949-b07e-261125e40710";

struct Credentials;
impl CredentialStore for Credentials {
    fn load<'a>(&'a self, _: &'a str) -> CredentialFuture<'a, Option<String>> {
        Box::pin(async {
            Ok(Some(
                json!({"schema":1,"refresh_token":"fixture-refresh"}).to_string(),
            ))
        })
    }
    fn save<'a>(&'a self, _: &'a str, _: Option<&'a str>) -> CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct Api {
    calls: AtomicUsize,
    cancels: AtomicUsize,
    mode: AtomicUsize,
    original: Mutex<Option<Value>>,
    writer: Mutex<Option<CaptureWriter>>,
    entered: tokio::sync::Notify,
    wait: tokio::sync::Notify,
}
struct Fixture {
    _temp: tempfile::TempDir,
    client: Arc<RemoteClient>,
    spool: Arc<CaptureSpool>,
    profile: Arc<ProfileLease>,
    api: Arc<Api>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let api = Arc::new(Api::default());
        let issuer = origin.trim_end_matches('/');
        let discovery = json!({"issuer":issuer,"authorization_endpoint":format!("{issuer}/authorize"),"token_endpoint":format!("{issuer}/token"),"code_challenge_methods_supported":["S256"]});
        let config = json!({"issuer":issuer,"authorization_endpoint":format!("{issuer}/authorize"),"token_endpoint":format!("{issuer}/token"),"client_id":"desktop","audience":"api","scopes":["loofah:access"]});
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(move || { let value = discovery.clone(); async { Json(value) } }))
            .route("/api/v1/auth/config", get(move || { let value = config.clone(); async { Json(value) } }))
            .route("/api/v1/capabilities", get(|| async { Json(json!({"api_major":1,"application_version":"test","features":["capture_processing","capture_ingestion","capture_live_transcription"]})) }))
            .route("/token", post(|| async { Json(json!({"token_type":"Bearer","access_token":"fixture-access","refresh_token":"fixture-refresh","expires_in":3600})) }))
            .route("/api/v1/vaults/{vault}/captures/{capture}", get(capture_source))
            .route("/api/v1/vaults/{vault}/captures/{capture}/live/{generation}/events", get(caption_events))
            .route("/api/v1/vaults/{vault}/captures/{capture}/processing", get(status).put(declare))
            .route("/api/v1/vaults/{vault}/captures/{capture}/processing/cancel", post(cancel))
            .with_state(api.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = RemoteClient::discover(&origin, Arc::new(Credentials), true)
            .await
            .unwrap();
        let profiles = ConnectionProfiles::new(root.join("profiles")).unwrap();
        let profile = profiles
            .create("fixture", "https://capture.example/")
            .await
            .unwrap();
        let spool = CaptureSpool::open(root.join("captures"), CaptureSpoolLimits::default())
            .await
            .unwrap();
        Self {
            _temp: temp,
            client,
            spool,
            profile,
            api,
            server,
        }
    }
    async fn writer(&self, declared: bool) -> CaptureWriter {
        let identity = CaptureIdentity {
            scope: CaptureScope {
                server: self.client.address().into(),
                member_id: MEMBER.into(),
                vault_id: VAULT.into(),
            },
            capture_id: uuid::Uuid::new_v4().to_string(),
            request_id: uuid::Uuid::new_v4().to_string(),
            target: CaptureTarget::ExistingSession {
                session_id: "note".into(),
                expected_audio_sha256: None,
            },
        };
        self.spool
            .begin_with_processing(
                identity,
                CaptureFormat::stereo(),
                declared.then_some(CaptureProcessingOptions {
                    model: Some("cpu".into()),
                    languages: vec!["en".into()],
                    keywords: vec![],
                    summary: None,
                }),
            )
            .await
            .unwrap()
    }
    async fn configure(&self, writer: &CaptureWriter) {
        *self.api.writer.lock().unwrap() = Some(writer.clone());
        let snapshot = writer.snapshot().await.unwrap();
        *self.api.original.lock().unwrap() = Some(json!({
            "capture_id":snapshot.identity.capture_id,"intent_id":snapshot.identity.request_id,
            "capture_fingerprint":snapshot.capture_fingerprint,
            "request":snapshot.processing.unwrap().request,"session_id":"note","model":"cpu","state":"live",
            "live_generation_id":snapshot.identity.request_id,"repair_job_id":null,"summary_job_id":null,
            "transcript_publication":null,"failure":null,"created_at":"2026-11-08T10:00:00Z","updated_at":"2026-11-08T10:00:00Z"
        }));
    }
    async fn start(
        &self,
        supervisor: &Arc<ProcessingSupervisor>,
        writer: &CaptureWriter,
    ) -> String {
        let id = self.profile.profile().await.id;
        supervisor
            .start(
                id.clone(),
                writer.clone(),
                writer.snapshot().await.unwrap(),
                self.client.clone(),
                self.profile.clone(),
            )
            .unwrap();
        id
    }
}
fn authorized(headers: &HeaderMap) {
    assert_eq!(headers["authorization"], "Bearer fixture-access");
}
async fn declare(
    State(api): State<Arc<Api>>,
    headers: HeaderMap,
    Json(request): Json<CaptureProcessingRequest>,
) -> axum::response::Response {
    authorized(&headers);
    let original = api.original.lock().unwrap().clone().unwrap();
    assert_eq!(json!(request), original["request"]);
    assert_eq!(
        headers["x-request-id"].to_str().unwrap(),
        original["intent_id"].as_str().unwrap()
    );
    api.calls.fetch_add(1, Ordering::SeqCst);
    api.entered.notify_one();
    match api.mode.load(Ordering::SeqCst) {
        1 => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(error(ErrorCode::Unavailable, "Processing is unavailable")),
            )
                .into_response();
        }
        2 => api.wait.notified().await,
        _ => {}
    }
    Json(original).into_response()
}
async fn status(State(api): State<Arc<Api>>, headers: HeaderMap) -> Json<Value> {
    authorized(&headers);
    Json(api.original.lock().unwrap().clone().unwrap())
}
async fn cancel(State(api): State<Arc<Api>>, headers: HeaderMap) -> Json<Value> {
    authorized(&headers);
    api.cancels.fetch_add(1, Ordering::SeqCst);
    let mut original = api.original.lock().unwrap().clone().unwrap();
    original["state"] = json!("cancelled");
    Json(original)
}
async fn admit(writer: &CaptureWriter) {
    writer
        .record_first_audio_observation(1_794_000_000_001)
        .await
        .unwrap();
    writer.append_chunk(0, 1, vec![0; 8]).await.unwrap();
    let snapshot = writer.snapshot().await.unwrap();
    writer
        .acknowledge_prefix(CapturePrefixReceipt {
            capture_fingerprint: snapshot.capture_fingerprint,
            chunk_count: snapshot.chunk_count,
            frames: snapshot.frames,
            pcm_bytes: snapshot.pcm_bytes,
            chain_digest: snapshot.chain_digest,
        })
        .await
        .unwrap();
}
async fn wait_finished(
    supervisor: &ProcessingSupervisor,
    profile: &str,
    identity: &CaptureIdentity,
) {
    let done = supervisor
        .entries
        .lock()
        .unwrap()
        .get(&(profile.into(), identity.capture_id.clone()))
        .unwrap()
        .finished
        .clone();
    tokio::time::timeout(Duration::from_secs(5), done.cancelled())
        .await
        .unwrap();
}

#[tokio::test]
async fn processing_supervisor_waits_for_ingestion_and_sends_durable_cancel_without_stopping_audio()
{
    let fixture = Fixture::new().await;
    let writer = fixture.writer(true).await;
    fixture.configure(&writer).await;
    let supervisor = ProcessingSupervisor::new(watch::channel(0).0);
    let profile = fixture.start(&supervisor, &writer).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(fixture.api.calls.load(Ordering::SeqCst), 0);
    admit(&writer).await;
    tokio::time::timeout(Duration::from_secs(5), fixture.api.entered.notified())
        .await
        .unwrap();
    writer.cancel_processing().await.unwrap();
    writer.append_chunk(1, 1, vec![0; 8]).await.unwrap();
    wait_finished(&supervisor, &profile, writer.identity()).await;
    let progress = supervisor.current(&profile, writer.identity()).unwrap();
    assert!(progress.declaration.cancel_requested);
    assert_eq!(
        progress.status.unwrap().state,
        hypr_loofah_protocol::CaptureProcessingState::Cancelled
    );
    assert_eq!(fixture.api.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.api.cancels.load(Ordering::SeqCst), 1);
    assert_eq!(writer.read_chunk(1).await.unwrap().bytes, vec![0; 8]);
    assert_eq!(writer.seal().await.unwrap().frames, 2);
    supervisor.wait_shutdown().await;
}

#[tokio::test]
async fn processing_supervisor_never_infers_historical_work_and_optional_failure_keeps_original_audio()
 {
    let fixture = Fixture::new().await;
    let supervisor = ProcessingSupervisor::new(watch::channel(0).0);
    let historical = fixture.writer(false).await;
    fixture.start(&supervisor, &historical).await;
    assert!(supervisor.entries.lock().unwrap().is_empty());
    assert_eq!(fixture.api.calls.load(Ordering::SeqCst), 0);
    let writer = fixture.writer(true).await;
    fixture.configure(&writer).await;
    fixture.api.mode.store(1, Ordering::SeqCst);
    admit(&writer).await;
    let profile = fixture.start(&supervisor, &writer).await;
    wait_finished(&supervisor, &profile, writer.identity()).await;
    let progress = supervisor.current(&profile, writer.identity()).unwrap();
    assert_eq!(progress.error.unwrap().code, ErrorCode::Unavailable);
    assert!(progress.status.is_none());
    assert!(
        !writer
            .snapshot()
            .await
            .unwrap()
            .processing
            .unwrap()
            .cancel_requested
    );
    writer.append_chunk(1, 1, vec![0; 8]).await.unwrap();
    assert_eq!(writer.seal().await.unwrap().frames, 2);
    writer.cancel_processing().await.unwrap();
    fixture.start(&supervisor, &writer).await;
    wait_finished(&supervisor, &profile, writer.identity()).await;
    assert_eq!(fixture.api.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.api.cancels.load(Ordering::SeqCst), 1);
    assert_eq!(
        supervisor
            .current(&profile, writer.identity())
            .unwrap()
            .status
            .unwrap()
            .state,
        hypr_loofah_protocol::CaptureProcessingState::Cancelled
    );
    supervisor.wait_shutdown().await;
}

#[tokio::test]
async fn processing_shutdown_joins_observers_without_cancelling_server_work_or_raw_capture() {
    let fixture = Fixture::new().await;
    let writer = fixture.writer(true).await;
    fixture.configure(&writer).await;
    fixture.api.mode.store(2, Ordering::SeqCst);
    admit(&writer).await;
    let supervisor = ProcessingSupervisor::new(watch::channel(0).0);
    fixture.start(&supervisor, &writer).await;
    tokio::time::timeout(Duration::from_secs(5), fixture.api.entered.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), supervisor.wait_shutdown())
        .await
        .unwrap();
    assert_eq!(fixture.api.cancels.load(Ordering::SeqCst), 0);
    assert!(
        !writer
            .snapshot()
            .await
            .unwrap()
            .processing
            .unwrap()
            .cancel_requested
    );
    writer.append_chunk(1, 1, vec![0; 8]).await.unwrap();
    assert_eq!(writer.seal().await.unwrap().frames, 2);
    fixture.api.wait.notify_waiters();
}

#[tokio::test]
async fn processing_cancel_before_ingestion_never_declares_work_and_raw_audio_can_continue() {
    let fixture = Fixture::new().await;
    let writer = fixture.writer(true).await;
    fixture.configure(&writer).await;
    writer.cancel_processing().await.unwrap();
    let supervisor = ProcessingSupervisor::new(watch::channel(0).0);
    let profile = fixture.start(&supervisor, &writer).await;
    wait_finished(&supervisor, &profile, writer.identity()).await;
    assert_eq!(fixture.api.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.api.cancels.load(Ordering::SeqCst), 0);
    admit(&writer).await;
    assert_eq!(writer.seal().await.unwrap().frames, 1);
    assert!(
        supervisor
            .current(&profile, writer.identity())
            .unwrap()
            .declaration
            .cancel_requested
    );
    supervisor.wait_shutdown().await;
}

#[tokio::test]
async fn processing_pending_session_uses_authoritative_ingest_binding_instead_of_deriving_an_id() {
    let fixture = Fixture::new().await;
    let writer = fixture.writer(true).await;
    fixture.configure(&writer).await;
    admit(&writer).await;
    let mut snapshot = writer.snapshot().await.unwrap();
    snapshot.identity.target = CaptureTarget::PendingSession {
        create_request_id: uuid::Uuid::new_v4().to_string(),
    };
    snapshot.session_id = None;
    snapshot.capture_fingerprint =
        hypr_loofah_protocol::capture_fingerprint(&snapshot.identity, &snapshot.format);
    let mut processing: CaptureProcessingSnapshot =
        serde_json::from_value(fixture.api.original.lock().unwrap().clone().unwrap()).unwrap();
    processing.session_id = Some("server-authoritative-session".into());
    processing.capture_fingerprint = snapshot.capture_fingerprint.clone();
    processing.request.capture_fingerprint = snapshot.capture_fingerprint.clone();
    snapshot
        .processing
        .as_mut()
        .unwrap()
        .request
        .capture_fingerprint = snapshot.capture_fingerprint.clone();
    let binding = hypr_loofah_protocol::CaptureIngestStatus {
        identity: snapshot.identity.clone(),
        format: snapshot.format.clone(),
        prefix: CapturePrefixReceipt {
            capture_fingerprint: snapshot.capture_fingerprint.clone(),
            chunk_count: 1,
            frames: 1,
            pcm_bytes: 8,
            chain_digest: snapshot.chain_digest.clone(),
        },
        first_admitted_audio_observed_at_ms: snapshot.first_admitted_audio_observed_at_ms.unwrap(),
        session_id: processing.session_id.clone(),
        seal: None,
        publication: None,
    };
    assert!(validate_pending_binding(&snapshot, &processing, &binding).is_ok());
    for mode in ["session", "member", "observation", "fingerprint"] {
        let mut wrong = binding.clone();
        match mode {
            "session" => wrong.session_id = Some("other-session".into()),
            "member" => wrong.identity.scope.member_id = uuid::Uuid::new_v4().to_string(),
            "observation" => wrong.first_admitted_audio_observed_at_ms += 1,
            "fingerprint" => wrong.prefix.capture_fingerprint = "b".repeat(64),
            _ => unreachable!(),
        }
        assert_eq!(
            validate_pending_binding(&snapshot, &processing, &wrong)
                .unwrap_err()
                .code,
            ErrorCode::IncompatibleVersion,
            "{mode}"
        );
    }
    snapshot.session_id = Some("originally-bound-session".into());
    assert!(validate_pending_binding(&snapshot, &processing, &binding).is_err());
}

async fn capture_source(
    State(api): State<Arc<Api>>,
    headers: HeaderMap,
) -> Json<hypr_loofah_protocol::CaptureIngestStatus> {
    authorized(&headers);
    let writer = api.writer.lock().unwrap().clone().unwrap();
    let snapshot = writer.snapshot().await.unwrap();
    Json(hypr_loofah_protocol::CaptureIngestStatus {
        identity: snapshot.identity,
        format: snapshot.format,
        prefix: CapturePrefixReceipt {
            capture_fingerprint: snapshot.capture_fingerprint,
            chunk_count: snapshot.chunk_count,
            frames: snapshot.frames,
            pcm_bytes: snapshot.pcm_bytes,
            chain_digest: snapshot.chain_digest,
        },
        first_admitted_audio_observed_at_ms: snapshot.first_admitted_audio_observed_at_ms.unwrap(),
        session_id: snapshot.session_id,
        seal: snapshot.seal,
        publication: snapshot.publication,
    })
}
async fn caption_events(State(api): State<Arc<Api>>, headers: HeaderMap) -> impl IntoResponse {
    use futures_util::StreamExt;
    authorized(&headers);
    let original = api.original.lock().unwrap().clone().unwrap();
    let generation = original["intent_id"].as_str().unwrap();
    assert_eq!(
        headers["last-event-id"].to_str().unwrap(),
        format!("{generation}:initial")
    );
    let page = json!({
        "snapshot": { "capture_id":original["capture_id"], "capture_fingerprint":original["capture_fingerprint"], "generation_id":generation, "session_id":"note", "state":"running", "failure":null, "admitted_end_frame":1, "input_consumed_end_frame":1, "completed_through_frame":null, "next_caption_sequence":1, "result_available":false, "publication":null },
        "captions":[{ "generation_id":generation, "sequence":0, "physical_channel":0, "role":"microphone", "new_words":[{"id":"word-1", "text":"hello", "start_ms":0, "end_ms":1, "channel":0, "state":"final"}], "partials":[], "replaced_ids":[], "languages":[] }], "next_cursor":null,
    });
    let event = axum::response::sse::Event::default()
        .event("page")
        .id(format!("{generation}:0"))
        .data(page.to_string());
    let stream =
        futures_util::stream::once(async move { Ok::<_, std::convert::Infallible>(event) })
            .chain(futures_util::stream::pending());
    axum::response::Sse::new(stream)
}

#[tokio::test]
async fn processing_captions_use_original_sdk_binding_and_logout_clears_visible_text() {
    let fixture = Fixture::new().await;
    let writer = fixture.writer(true).await;
    fixture.configure(&writer).await;
    admit(&writer).await;
    let supervisor = ProcessingSupervisor::new(watch::channel(0).0);
    let profile = fixture.start(&supervisor, &writer).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if supervisor
                .current(&profile, writer.identity())
                .and_then(|value| value.captions)
                .is_some_and(|captions| captions.words.iter().any(|word| word.text == "hello"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let captions = supervisor
        .current(&profile, writer.identity())
        .unwrap()
        .captions
        .unwrap();
    assert_eq!(captions.binding.capture_id, writer.identity().capture_id);
    assert_eq!(captions.binding.generation_id, writer.identity().request_id);
    assert_eq!(captions.binding.format, CaptureFormat::stereo());
    assert_eq!(captions.words.len(), 1);
    fixture.client.logout().await.unwrap();
    wait_finished(&supervisor, &profile, writer.identity()).await;
    let captions = supervisor
        .current(&profile, writer.identity())
        .unwrap()
        .captions
        .unwrap();
    assert!(captions.words.is_empty());
    assert!(captions.partials.is_empty());
    assert_eq!(
        captions.connection,
        hypr_loofah_client::ConnectionState::Closed
    );
    assert_eq!(fixture.api.cancels.load(Ordering::SeqCst), 0);
    writer.append_chunk(1, 1, vec![0; 8]).await.unwrap();
    supervisor.wait_shutdown().await;
}

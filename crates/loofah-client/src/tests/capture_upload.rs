use super::*;
use crate::{
    capture_spool::{CaptureSpool, CaptureSpoolLimits, CaptureWriter},
    capture_upload::*,
};
use axum::{Extension, body::Bytes, extract::Path};
use tokio::sync::Semaphore;

const PROFILE: &str = "6114bd31-62da-4d90-9a0d-41b9a8872230";
struct Server {
    spool: Arc<CaptureSpool>,
    declaration: Mutex<Option<CaptureIngestRequest>>,
    offline: AtomicBool,
    lost_ack: AtomicBool,
    corrupt: AtomicBool,
    missing: AtomicBool,
    accepted: Semaphore,
    begins: AtomicUsize,
    publishes: AtomicUsize,
    reads: AtomicUsize,
    publication: Mutex<Option<CapturePublication>>,
    canonical: Mutex<Vec<u8>>,
}
impl Server {
    async fn new(root: &std::path::Path) -> Arc<Self> {
        Arc::new(Self {
            spool: CaptureSpool::open(root.join("server"), CaptureSpoolLimits::default())
                .await
                .unwrap(),
            declaration: Mutex::new(None),
            offline: AtomicBool::new(false),
            lost_ack: AtomicBool::new(false),
            corrupt: AtomicBool::new(false),
            missing: AtomicBool::new(false),
            accepted: Semaphore::new(0),
            begins: AtomicUsize::new(0),
            publishes: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            publication: Mutex::new(None),
            canonical: Mutex::new(Vec::new()),
        })
    }
    fn routes(self: &Arc<Self>) -> Router<Arc<Provider>> {
        Router::new()
            .route("/api/v1/vaults/queued/captures", post(begin))
            .route(
                "/api/v1/vaults/queued/captures/{capture}/chunks/{sequence}",
                axum::routing::put(append),
            )
            .route("/api/v1/vaults/queued/captures/{capture}/seal", post(seal))
            .route(
                "/api/v1/vaults/queued/captures/{capture}/publish",
                post(publish),
            )
            .route(
                "/api/v1/vaults/queued/sessions/note/audio/leases/{lease}",
                axum::routing::put(open),
            )
            .route(
                "/api/v1/vaults/queued/audio/leases/{lease}",
                get(read).delete(close),
            )
            .layer(Extension(self.clone()))
    }
    async fn writer(&self) -> CaptureWriter {
        self.spool
            .resume(
                self.declaration
                    .lock()
                    .await
                    .as_ref()
                    .unwrap()
                    .identity
                    .clone(),
            )
            .await
            .unwrap()
    }
    async fn status(&self) -> CaptureIngestStatus {
        let snapshot = self.writer().await.snapshot().await.unwrap();
        CaptureIngestStatus {
            identity: snapshot.identity,
            format: snapshot.format,
            prefix: CapturePrefixReceipt {
                capture_fingerprint: snapshot.capture_fingerprint,
                chunk_count: snapshot.chunk_count,
                frames: snapshot.frames,
                pcm_bytes: snapshot.pcm_bytes,
                chain_digest: snapshot.chain_digest,
            },
            seal: snapshot.seal,
            session_id: snapshot.session_id,
            first_admitted_audio_observed_at_ms: snapshot
                .first_admitted_audio_observed_at_ms
                .unwrap(),
            publication: self.publication.lock().await.clone(),
        }
    }
}
fn unavailable_response() -> axum::response::Response {
    let mut cause = failure(ErrorCode::Unavailable, "Synthetic network outage");
    cause.retryable = true;
    (StatusCode::SERVICE_UNAVAILABLE, Json(cause)).into_response()
}
fn authenticated(headers: &HeaderMap) {
    assert!(
        headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Bearer access-")
    );
    assert_eq!(headers["x-loofah-api-major"], "1");
}
async fn begin(
    Extension(state): Extension<Arc<Server>>,
    State(provider): State<Arc<Provider>>,
    headers: HeaderMap,
    Json(input): Json<CaptureIngestRequest>,
) -> axum::response::Response {
    authenticated(&headers);
    if state.offline.load(Ordering::SeqCst) {
        return unavailable_response();
    }
    assert_eq!(input.identity.scope.server, format!("{}/", provider.origin));
    assert_eq!(input.identity.scope.member_id, "member");
    let mut saved = state.declaration.lock().await;
    if let Some(saved) = saved.as_ref() {
        assert_eq!(saved, &input);
    } else {
        *saved = Some(input.clone());
    }
    drop(saved);
    state.begins.fetch_add(1, Ordering::SeqCst);
    state
        .spool
        .begin_with_observation(
            input.identity,
            input.format,
            input.first_admitted_audio_observed_at_ms,
        )
        .await
        .unwrap();
    Json(state.status().await).into_response()
}
async fn append(
    Extension(state): Extension<Arc<Server>>,
    Path((capture, sequence)): Path<(String, u64)>,
    headers: HeaderMap,
    bytes: Bytes,
) -> axum::response::Response {
    authenticated(&headers);
    if state.offline.load(Ordering::SeqCst) {
        return unavailable_response();
    }
    let descriptor: CaptureChunkDescriptor =
        serde_json::from_str(headers["x-capture-chunk"].to_str().unwrap()).unwrap();
    assert_eq!(sequence, descriptor.sequence);
    assert_eq!(
        capture,
        state
            .declaration
            .lock()
            .await
            .as_ref()
            .unwrap()
            .identity
            .capture_id
    );
    let actual = state
        .writer()
        .await
        .append_chunk(sequence, descriptor.frames, bytes.to_vec())
        .await
        .unwrap();
    assert_eq!(descriptor, actual);
    state.accepted.add_permits(1);
    if state.lost_ack.swap(false, Ordering::SeqCst) {
        return axum::response::Response::new(axum::body::Body::from_stream(
            futures_util::stream::once(async {
                Err::<Bytes, std::io::Error>(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "lost append acknowledgement",
                ))
            }),
        ));
    }
    Json(state.status().await.prefix).into_response()
}
async fn seal(
    Extension(state): Extension<Arc<Server>>,
    Path(_capture): Path<String>,
    headers: HeaderMap,
    Json(seal): Json<CaptureSeal>,
) -> Json<CaptureIngestStatus> {
    authenticated(&headers);
    state.writer().await.seal_if_matches(seal).await.unwrap();
    Json(state.status().await)
}
async fn publish(
    Extension(state): Extension<Arc<Server>>,
    Path(_capture): Path<String>,
    headers: HeaderMap,
    Json(seal): Json<CaptureSeal>,
) -> Json<CaptureIngestStatus> {
    authenticated(&headers);
    state.publishes.fetch_add(1, Ordering::SeqCst);
    assert_eq!(
        state.writer().await.snapshot().await.unwrap().seal,
        Some(seal.clone())
    );
    let mut canonical = b"RIFF-synthetic-canonical-WAV".to_vec();
    for descriptor in state.writer().await.chunks(None, 200).await.unwrap() {
        canonical.extend(
            state
                .writer()
                .await
                .read_chunk(descriptor.sequence)
                .await
                .unwrap()
                .bytes,
        );
    }
    let publication = CapturePublication {
        source: seal,
        session_id: "note".into(),
        canonical_audio_sha256: hex(&canonical),
        canonical_audio_bytes: canonical.len() as u64,
    };
    *state.canonical.lock().await = canonical;
    *state.publication.lock().await = Some(publication);
    Json(state.status().await)
}
async fn open(
    Extension(state): Extension<Arc<Server>>,
    Path(lease): Path<String>,
    headers: HeaderMap,
) -> Json<Option<RecordingLease>> {
    authenticated(&headers);
    if state.missing.load(Ordering::SeqCst) {
        return Json(None);
    }
    let publication = state.publication.lock().await.clone().unwrap();
    Json(Some(RecordingLease {
        id: lease,
        vault_id: "queued".into(),
        session_id: "note".into(),
        idle_timeout_secs: 600,
        metadata: FileMetadata {
            filename: "audio.wav".into(),
            content_type: "audio/wav".into(),
            size_bytes: publication.canonical_audio_bytes,
            sha256: publication.canonical_audio_sha256,
        },
    }))
}
async fn read(
    Extension(state): Extension<Arc<Server>>,
    Path(_lease): Path<String>,
    headers: HeaderMap,
) -> axum::response::Response {
    authenticated(&headers);
    state.reads.fetch_add(1, Ordering::SeqCst);
    let publication = state.publication.lock().await.clone().unwrap();
    let mut bytes = state.canonical.lock().await.clone();
    let range = headers["range"]
        .to_str()
        .unwrap()
        .strip_prefix("bytes=")
        .unwrap();
    let (start, end) = range.split_once('-').unwrap();
    let start: usize = start.parse().unwrap();
    let end: usize = end.parse().unwrap();
    if state.corrupt.load(Ordering::SeqCst) {
        bytes[start] ^= 1;
    }
    axum::response::Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header("content-type", "audio/wav")
        .header(
            "etag",
            format!("\"sha256:{}\"", publication.canonical_audio_sha256),
        )
        .header(
            "content-range",
            format!("bytes {start}-{end}/{}", bytes.len()),
        )
        .header("content-length", (end - start + 1).to_string())
        .body(axum::body::Body::from(bytes[start..=end].to_vec()))
        .unwrap()
}
async fn close(headers: HeaderMap) -> Json<bool> {
    authenticated(&headers);
    Json(true)
}
fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn identity(client: &RemoteClient) -> CaptureIdentity {
    CaptureIdentity {
        scope: CaptureScope {
            server: client.address().into(),
            member_id: "member".into(),
            vault_id: "queued".into(),
        },
        capture_id: uuid::Uuid::new_v4().to_string(),
        request_id: uuid::Uuid::new_v4().to_string(),
        target: CaptureTarget::ExistingSession {
            session_id: "note".into(),
            expected_audio_sha256: None,
        },
    }
}
fn binding(client: &RemoteClient, writer: &CaptureWriter) -> CaptureUploadBinding {
    CaptureUploadBinding {
        profile_id: PROFILE.into(),
        scope: writer.identity().scope.clone(),
        credential_account: client.credential_account().into(),
    }
}
async fn fixture(state: &Arc<Server>, rtt: Option<Duration>) -> (Fixture, Arc<RemoteClient>) {
    let fixture = Fixture::with_extra_routes(rtt, state.routes()).await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client).unwrap().capabilities.features = vec![
        "capture_ingestion".into(),
        "capture_publication".into(),
        "recording_leases".into(),
    ];
    fixture.login(&client).await;
    (fixture, client)
}
async fn source(
    root: &std::path::Path,
    client: &RemoteClient,
) -> (Arc<CaptureSpool>, CaptureWriter) {
    let spool = CaptureSpool::open(root.join("device"), CaptureSpoolLimits::default())
        .await
        .unwrap();
    let writer = spool
        .begin_with_observation(identity(client), CaptureFormat::stereo(), 1_799_999_000_123)
        .await
        .unwrap();
    (spool, writer)
}
async fn append_source(writer: &CaptureWriter, sequence: u64) {
    writer
        .append_chunk(sequence, 16, vec![0; 128])
        .await
        .unwrap();
}
async fn done(ticket: &CaptureUploadTicket) -> CaptureUploadProgress {
    tokio::time::timeout(Duration::from_secs(20), ticket.wait_stopped())
        .await
        .unwrap()
}
struct Owner(Arc<AtomicBool>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn lost_ack_and_offline_capture_reopen_resume_exact_identity_and_hash_independent_canonical_bytes()
 {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let state = Server::new(root.path()).await;
    state.lost_ack.store(true, Ordering::SeqCst);
    let (_fixture, client) = fixture(&state, Some(Duration::from_millis(50))).await;
    let (spool, writer) = source(root.path(), &client).await;
    append_source(&writer, 0).await;
    let id = writer.identity().clone();
    let retained = Arc::new(AtomicBool::new(false));
    let uploader = CaptureUploader::new(1).unwrap();
    let ticket = uploader
        .start(
            client.clone(),
            writer.clone(),
            binding(&client, &writer),
            Box::new(Owner(retained.clone())),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), state.accepted.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    state.offline.store(true, Ordering::SeqCst);
    append_source(&writer, 1).await;
    append_source(&writer, 2).await;
    let seal = writer.seal().await.unwrap();
    assert_eq!(writer.snapshot().await.unwrap().frames, 48);
    assert!(!retained.load(Ordering::SeqCst));
    ticket.request_stop();
    assert_eq!(done(&ticket).await.phase, CaptureUploadPhase::Stopped);
    assert!(retained.load(Ordering::SeqCst));
    assert_eq!(writer.read_chunk(0).await.unwrap().bytes, vec![0; 128]);
    drop(writer);
    drop(spool);
    drop(ticket);
    uploader.wait_shutdown().await;
    drop(uploader);
    let spool = CaptureSpool::open(root.path().join("device"), CaptureSpoolLimits::default())
        .await
        .unwrap();
    let writer = spool.resume(id.clone()).await.unwrap();
    assert_eq!(writer.snapshot().await.unwrap().seal, Some(seal));
    client.disconnect();
    let mut client = _fixture.client().await;
    Arc::get_mut(&mut client).unwrap().capabilities.features = vec![
        "capture_ingestion".into(),
        "capture_publication".into(),
        "recording_leases".into(),
    ];
    _fixture.login(&client).await;
    state.offline.store(false, Ordering::SeqCst);
    let uploader = CaptureUploader::new(1).unwrap();
    let ticket = uploader
        .start(
            client.clone(),
            writer.clone(),
            binding(&client, &writer),
            Box::new(()),
        )
        .unwrap();
    ticket.source_finished();
    let result = done(&ticket).await;
    assert_eq!(
        result.phase,
        CaptureUploadPhase::Verified,
        "{:?}",
        result.error
    );
    let snapshot = writer.snapshot().await.unwrap();
    assert_eq!(snapshot.identity, id);
    assert_eq!(snapshot.acknowledged.unwrap().chunk_count, 3);
    assert!(snapshot.publication.is_some());
    assert_eq!(state.reads.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.writer().await.snapshot().await.unwrap().chunk_count,
        3
    );
    for sequence in 0..3 {
        assert_eq!(
            writer.read_chunk(sequence).await.unwrap().bytes,
            vec![0; 128]
        );
    }
    uploader.wait_shutdown().await;
}

#[tokio::test]
async fn publication_receipt_without_matching_independent_body_never_verifies_or_resurrects_audio()
{
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let state = Server::new(root.path()).await;
    state.corrupt.store(true, Ordering::SeqCst);
    let (_fixture, client) = fixture(&state, None).await;
    let (_spool, writer) = source(root.path(), &client).await;
    append_source(&writer, 0).await;
    writer.seal().await.unwrap();
    let uploader = CaptureUploader::new(1).unwrap();
    let ticket = uploader
        .start(
            client.clone(),
            writer.clone(),
            binding(&client, &writer),
            Box::new(()),
        )
        .unwrap();
    ticket.source_finished();
    assert_eq!(done(&ticket).await.phase, CaptureUploadPhase::NeedsReview);
    assert!(writer.snapshot().await.unwrap().publication.is_none());
    state.corrupt.store(false, Ordering::SeqCst);
    let ticket = uploader
        .start(
            client.clone(),
            writer.clone(),
            binding(&client, &writer),
            Box::new(()),
        )
        .unwrap();
    ticket.source_finished();
    assert_eq!(done(&ticket).await.phase, CaptureUploadPhase::Verified);
    let begins = state.begins.load(Ordering::SeqCst);
    let publishes = state.publishes.load(Ordering::SeqCst);
    state.missing.store(true, Ordering::SeqCst);
    let ticket = uploader
        .start(
            client.clone(),
            writer.clone(),
            binding(&client, &writer),
            Box::new(()),
        )
        .unwrap();
    ticket.source_finished();
    assert_eq!(done(&ticket).await.phase, CaptureUploadPhase::NeedsReview);
    assert_eq!(state.begins.load(Ordering::SeqCst), begins);
    assert_eq!(state.publishes.load(Ordering::SeqCst), publishes);
    assert_eq!(writer.read_chunk(0).await.unwrap().bytes.len(), 128);
    uploader.wait_shutdown().await;
}

#[tokio::test]
async fn immutable_profile_server_member_vault_and_session_are_never_retargeted() {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let state = Server::new(root.path()).await;
    let (_fixture, client) = fixture(&state, None).await;
    let (_spool, writer) = source(root.path(), &client).await;
    append_source(&writer, 0).await;
    writer.seal().await.unwrap();
    let uploader = CaptureUploader::new(1).unwrap();
    for field in ["profile", "server", "member", "vault", "account"] {
        let mut input = binding(&client, &writer);
        match field {
            "profile" => input.profile_id = "../foreign".into(),
            "server" => input.scope.server = "https://foreign.example/".into(),
            "member" => input.scope.member_id = "other".into(),
            "vault" => input.scope.vault_id = "other".into(),
            _ => input.credential_account = "foreign".into(),
        };
        assert!(
            uploader
                .start(client.clone(), writer.clone(), input, Box::new(()))
                .is_err()
        );
    }
    let mut other = writer.identity().clone();
    other.scope.member_id = "other-member".into();
    let spool = CaptureSpool::open(
        root.path().join("other-device"),
        CaptureSpoolLimits::default(),
    )
    .await
    .unwrap();
    let writer = spool
        .begin_with_observation(other, CaptureFormat::stereo(), 1)
        .await
        .unwrap();
    append_source(&writer, 0).await;
    writer.seal().await.unwrap();
    let ticket = uploader
        .start(
            client.clone(),
            writer.clone(),
            binding(&client, &writer),
            Box::new(()),
        )
        .unwrap();
    ticket.source_finished();
    assert_eq!(done(&ticket).await.phase, CaptureUploadPhase::NeedsReview);
    assert_eq!(state.begins.load(Ordering::SeqCst), 0);
    assert!(writer.snapshot().await.unwrap().publication.is_none());
    uploader.wait_shutdown().await;
}

#[tokio::test]
async fn pending_append_and_interrupted_sources_stay_explicit_without_canonical_publication() {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let state = Server::new(root.path()).await;
    let (fixture, client) = fixture(&state, None).await;
    let spool = CaptureSpool::open(root.path().join("device"), CaptureSpoolLimits::default())
        .await
        .unwrap();
    let loads = fixture.credentials.loads.load(Ordering::SeqCst);
    let uploader = CaptureUploader::new(1).unwrap();
    for (target, expected) in [
        (
            CaptureTarget::PendingSession {
                create_request_id: uuid::Uuid::new_v4().to_string(),
            },
            CaptureUploadPhase::PendingSession,
        ),
        (
            CaptureTarget::ExistingSession {
                session_id: "note".into(),
                expected_audio_sha256: Some("a".repeat(64)),
            },
            CaptureUploadPhase::AppendUnsupported,
        ),
        (
            CaptureTarget::ExistingSession {
                session_id: "note".into(),
                expected_audio_sha256: None,
            },
            CaptureUploadPhase::NeedsReview,
        ),
    ] {
        let mut id = identity(&client);
        id.target = target;
        let writer = spool
            .begin_with_observation(id, CaptureFormat::stereo(), 1)
            .await
            .unwrap();
        append_source(&writer, 0).await;
        let ticket = uploader
            .start(
                client.clone(),
                writer.clone(),
                binding(&client, &writer),
                Box::new(()),
            )
            .unwrap();
        ticket.source_finished();
        assert_eq!(done(&ticket).await.phase, expected);
        assert!(writer.snapshot().await.unwrap().seal.is_none());
        assert!(writer.snapshot().await.unwrap().publication.is_none());
    }
    assert_eq!(state.begins.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), loads);
    uploader.wait_shutdown().await;
}

#[tokio::test]
async fn dropping_upload_owner_cancels_active_and_queued_network_work_without_stopping_local_append()
 {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let state = Server::new(root.path()).await;
    state.offline.store(true, Ordering::SeqCst);
    let (_fixture, client) = fixture(&state, None).await;
    let (spool, first) = source(root.path(), &client).await;
    append_source(&first, 0).await;
    let second = spool
        .begin_with_observation(identity(&client), CaptureFormat::stereo(), 1)
        .await
        .unwrap();
    append_source(&second, 0).await;
    let uploader = CaptureUploader::new(1).unwrap();
    let released = Arc::new(AtomicBool::new(false));
    let first_ticket = uploader
        .start(
            client.clone(),
            first.clone(),
            binding(&client, &first),
            Box::new(Owner(released.clone())),
        )
        .unwrap();
    let second_ticket = uploader
        .start(
            client.clone(),
            second.clone(),
            binding(&client, &second),
            Box::new(()),
        )
        .unwrap();
    assert!(
        uploader
            .start(
                client.clone(),
                first.clone(),
                binding(&client, &first),
                Box::new(())
            )
            .is_err()
    );
    drop(uploader);
    assert_eq!(done(&first_ticket).await.phase, CaptureUploadPhase::Stopped);
    assert_eq!(
        done(&second_ticket).await.phase,
        CaptureUploadPhase::Stopped
    );
    assert!(released.load(Ordering::SeqCst));
    append_source(&first, 1).await;
    assert_eq!(first.snapshot().await.unwrap().chunk_count, 2);
    assert_eq!(second.snapshot().await.unwrap().chunk_count, 1);
}

#[tokio::test]
async fn unsupported_uploads_do_not_read_credentials_and_closed_clients_leave_audio_for_reauthentication()
 {
    let fixture = Fixture::new().await;
    let mut client = fixture.client().await;
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let (_spool, writer) = source(root.path(), &client).await;
    append_source(&writer, 0).await;
    writer.seal().await.unwrap();
    let uploader = CaptureUploader::new(1).unwrap();
    let loads = fixture.credentials.loads.load(Ordering::SeqCst);
    let ticket = uploader
        .start(
            client.clone(),
            writer.clone(),
            binding(&client, &writer),
            Box::new(()),
        )
        .unwrap();
    ticket.source_finished();
    let finished = done(&ticket).await;
    assert_eq!(finished.phase, CaptureUploadPhase::NeedsReview);
    assert_eq!(finished.error.unwrap().code, ErrorCode::IncompatibleVersion);
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), loads);
    drop(ticket);
    Arc::get_mut(&mut client).unwrap().capabilities.features = vec![
        "capture_ingestion".into(),
        "capture_publication".into(),
        "recording_leases".into(),
    ];
    client.disconnect();
    let ticket = uploader
        .start(
            client.clone(),
            writer.clone(),
            binding(&client, &writer),
            Box::new(()),
        )
        .unwrap();
    ticket.source_finished();
    assert_eq!(
        done(&ticket).await.phase,
        CaptureUploadPhase::AuthenticationRequired
    );
    assert_eq!(writer.read_chunk(0).await.unwrap().bytes.len(), 128);
    assert!(writer.snapshot().await.unwrap().publication.is_none());
    uploader.wait_shutdown().await;
}

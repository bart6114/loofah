use super::*;
use crate::device_capture::remote::RemoteCaptureCoordinator;
use axum::{
    Json, Router,
    extract::State,
    response::IntoResponse,
    routing::{get, post},
};
use hypr_loofah_client::{
    ConnectionProfiles, CredentialFuture, CredentialStore, ProfileLease, RemoteClient,
};
use hypr_loofah_protocol::{
    CaptureAppendAdmission, CaptureAppendBaseline, CaptureAppendRequest, CaptureChannel,
};
use serde_json::json;

struct Credentials;
impl CredentialStore for Credentials {
    fn load<'a>(&'a self, _: &'a str) -> CredentialFuture<'a, Option<String>> {
        Box::pin(async { Ok(Some(r#"{"schema":1,"refresh_token":"fixture"}"#.into())) })
    }
    fn save<'a>(&'a self, _: &'a str, _: Option<&'a str>) -> CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}
struct Server {
    requests: Mutex<Vec<CaptureAppendRequest>>,
    entered: Semaphore,
    release: Semaphore,
    pause: AtomicBool,
    lost: AtomicBool,
    mixed: bool,
}
async fn prepare(
    State(state): State<Arc<Server>>,
    Json(request): Json<CaptureAppendRequest>,
) -> axum::response::Response {
    state.requests.lock().unwrap().push(request.clone());
    state.entered.add_permits(1);
    if state.pause.load(Ordering::Acquire) {
        state.release.acquire().await.unwrap().forget();
    }
    if state.lost.swap(false, Ordering::AcqRel) {
        return "lost response".into_response();
    }
    let mut format = CaptureFormat::stereo();
    if state.mixed {
        format.channels = vec![CaptureChannel::Mixed];
    }
    Json(CaptureAppendAdmission {
        request,
        baseline: CaptureAppendBaseline {
            recording_selection: "audio.wav".into(),
            recording_sha256: "1".repeat(64),
            transcript_present: false,
            transcript_revision: "2".repeat(64),
            normalized_sha256: "3".repeat(64),
            normalized_bytes: 128044,
            frames: 16000,
            format,
        },
    })
    .into_response()
}
struct Fixture {
    _temp: tempfile::TempDir,
    profile: Arc<ProfileLease>,
    client: Arc<RemoteClient>,
    host: Arc<RemoteCaptureCoordinator>,
    spool: Arc<CaptureSpool>,
    driver: Arc<Driver>,
    server: Arc<Server>,
    task: tokio::task::JoinHandle<()>,
    request: CaptureAppendRequest,
    lifetime: CancellationToken,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.lifetime.cancel();
        self.server.release.add_permits(8);
        self.task.abort();
    }
}
impl Fixture {
    async fn new(mixed: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = Arc::new(Server {
            requests: Mutex::new(Vec::new()),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            pause: AtomicBool::new(false),
            lost: AtomicBool::new(false),
            mixed,
        });
        let config = json!({"issuer":origin,"authorization_endpoint":format!("{origin}/authorize"),"token_endpoint":format!("{origin}/token"),"client_id":"desktop","audience":"api","scopes":["loofah:access"]});
        let discovery = json!({"issuer":origin,"authorization_endpoint":format!("{origin}/authorize"),"token_endpoint":format!("{origin}/token"),"code_challenge_methods_supported":["S256"]});
        let router = Router::new()
            .route("/api/v1/capabilities",get(||async{Json(json!({"api_major":1,"application_version":"test","features":["capture_publication","capture_append"]}))}))
            .route("/api/v1/auth/config",get(move ||async move{Json(config)}))
            .route("/.well-known/openid-configuration",get(move ||async move{Json(discovery)}))
            .route("/token",post(||async{Json(json!({"access_token":"fixture","token_type":"Bearer","expires_in":3600}))}))
            .route("/api/v1/me",get(||async{Json(json!({"member":{"id":"member","name":"Fixture"},"personal_vault_id":"remote"}))}))
            .route("/api/v1/vaults/remote/capture-append-admissions",post(prepare))
            .with_state(server.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = RemoteClient::discover(&origin, Arc::new(Credentials), true)
            .await
            .unwrap();
        let profile = ConnectionProfiles::new(root.join("profiles"))
            .unwrap()
            .create("fixture", client.address())
            .await
            .unwrap();
        let mut saved = profile.profile().await;
        saved.member_id = Some("member".into());
        saved.vault_id = Some("remote".into());
        saved
            .credential_accounts
            .push(client.credential_account().into());
        profile.save(saved.clone()).await.unwrap();
        let spool = CaptureSpool::open(
            root.join("captures").join(&saved.id),
            CaptureSpoolLimits::default(),
        )
        .await
        .unwrap();
        let driver = driver(true, StartMode::Reject);
        let device = DeviceCaptureHost::with_driver(
            spool.clone(),
            Arc::new(Observer::default()),
            driver.clone(),
        );
        let host =
            RemoteCaptureCoordinator::new(root.join("captures"), Arc::new(Observer::default()))
                .unwrap();
        host.install_test_host(saved.id, spool.clone(), device);
        let request = CaptureAppendRequest {
            scope: CaptureScope {
                server: client.address().into(),
                member_id: "member".into(),
                vault_id: "remote".into(),
            },
            session_id: "note".into(),
            capture_id: uuid::Uuid::new_v4().to_string(),
            request_id: uuid::Uuid::new_v4().to_string(),
            admission_id: uuid::Uuid::new_v4().to_string(),
            processing: None,
        };
        Self {
            _temp: temp,
            profile,
            client,
            host,
            spool,
            driver,
            server,
            task,
            request,
            lifetime: CancellationToken::new(),
        }
    }
    async fn start(
        &self,
    ) -> Result<
        crate::device_capture::remote::RemoteDeviceCapture,
        hypr_loofah_protocol::ApplicationError,
    > {
        self.host
            .start_append(
                self.profile.clone(),
                self.client.clone(),
                self.request.clone(),
                "window".into(),
                self.lifetime.clone(),
            )
            .await
    }
}

#[tokio::test]
async fn lost_append_preparation_reuses_original_ids_and_authoritative_format() {
    let _capture = crate::device_capture::CAPTURE_TEST_LOCK.lock().await;
    let fixture = Fixture::new(true).await;
    fixture.server.lost.store(true, Ordering::Release);
    assert!(fixture.start().await.is_err());
    let pending = fixture
        .host
        .pending_append_requests(fixture.profile.clone(), fixture.request.scope.clone())
        .await
        .unwrap();
    assert_eq!(pending, vec![fixture.request.clone()]);
    assert!(
        fixture
            .spool
            .inventory(fixture.request.scope.clone())
            .await
            .unwrap()
            .is_empty()
    );
    let mut started = fixture.start().await.unwrap();
    until(&mut started.device, |state| {
        state.phase == DeviceHostPhase::NeverStarted
    })
    .await;
    assert_eq!(started.identity.capture_id, fixture.request.capture_id);
    let snapshots = fixture
        .spool
        .inventory(fixture.request.scope.clone())
        .await
        .unwrap();
    assert_eq!(snapshots[0].format.channels, vec![CaptureChannel::Mixed]);
    assert!(matches!(
        snapshots[0].identity.target,
        CaptureTarget::AppendSession { .. }
    ));
    let replay = fixture.start().await.unwrap();
    assert_eq!(replay.identity, started.identity);
    assert_eq!(fixture.driver.starts.load(Ordering::Acquire), 1);
    assert_eq!(
        *fixture.server.requests.lock().unwrap(),
        vec![fixture.request.clone(), fixture.request.clone()]
    );
    let mut changed = fixture.request.clone();
    changed.admission_id = uuid::Uuid::new_v4().to_string();
    assert!(
        fixture
            .host
            .start_append(
                fixture.profile.clone(),
                fixture.client.clone(),
                changed,
                "window".into(),
                fixture.lifetime.clone()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn profile_change_during_append_preparation_never_dispatches_or_refreshes_baseline() {
    let _capture = crate::device_capture::CAPTURE_TEST_LOCK.lock().await;
    let fixture = Fixture::new(false).await;
    fixture.server.pause.store(true, Ordering::Release);
    let host = fixture.host.clone();
    let profile = fixture.profile.clone();
    let client = fixture.client.clone();
    let request = fixture.request.clone();
    let lifetime = fixture.lifetime.clone();
    let operation = tokio::spawn(async move {
        host.start_append(profile, client, request, "window".into(), lifetime)
            .await
    });
    fixture.server.entered.acquire().await.unwrap().forget();
    let original = fixture.profile.profile().await;
    let mut changed = original.clone();
    changed.vault_id = Some("other".into());
    fixture.profile.save(changed).await.unwrap();
    fixture.profile.save(original).await.unwrap();
    fixture.server.release.add_permits(1);
    assert!(operation.await.unwrap().is_err());
    assert!(fixture.start().await.is_err());
    assert_eq!(fixture.driver.starts.load(Ordering::Acquire), 0);
    assert_eq!(fixture.server.requests.lock().unwrap().len(), 1);
    assert!(
        fixture
            .spool
            .inventory(fixture.request.scope.clone())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn old_unbound_target_and_legacy_hash_cannot_start_native_append() {
    let _capture = crate::device_capture::CAPTURE_TEST_LOCK.lock().await;
    let fixture = Fixture::new(false).await;
    let mut input = intent();
    input.identity.target = CaptureTarget::ExistingSession {
        session_id: "note".into(),
        expected_audio_sha256: Some("a".repeat(64)),
    };
    let host = DeviceCaptureHost::with_driver(
        fixture.spool.clone(),
        Arc::new(Observer::default()),
        fixture.driver.clone(),
    );
    assert!(host.start(input).is_err());
    fixture.lifetime.cancel();
    assert!(fixture.start().await.is_err());
    assert_eq!(fixture.driver.starts.load(Ordering::Acquire), 0);
    assert!(fixture.server.requests.lock().unwrap().is_empty());
}

struct DelayedAck {
    entered: Semaphore,
    release: Semaphore,
}
#[ractor::async_trait]
impl CaptureDriver for DelayedAck {
    fn ready(&self) -> bool {
        true
    }
    async fn start(
        &self,
        _args: tauri_plugin_transcription::DeviceCaptureArgs,
    ) -> Result<(), DeviceCaptureStartError> {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        Err(DeviceCaptureStartError::Rejected)
    }
    async fn stop(&self, _: DeviceCaptureKey) -> bool {
        false
    }
}

#[tokio::test]
async fn append_scope_commit_fence_releases_before_delayed_native_acknowledgement() {
    let _capture = crate::device_capture::CAPTURE_TEST_LOCK.lock().await;
    let fixture = Fixture::new(false).await;
    let delayed = Arc::new(DelayedAck {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let host = DeviceCaptureHost::with_driver(
        fixture.spool.clone(),
        Arc::new(Observer::default()),
        delayed.clone(),
    );
    fixture.host.install_test_host(
        fixture.profile.profile().await.id,
        fixture.spool.clone(),
        host,
    );
    let mut next = fixture.profile.profile().await;
    next.vault_id = Some("later-choice".into());
    let mut capture = fixture.start().await.unwrap();
    delayed.entered.acquire().await.unwrap().forget();
    tokio::time::timeout(Duration::from_secs(2), fixture.profile.save(next))
        .await
        .expect("profile UI must not wait for native acknowledgement")
        .unwrap();
    delayed.release.add_permits(1);
    until(&mut capture.device, |state| {
        state.phase == DeviceHostPhase::NeverStarted
    })
    .await;
    assert_eq!(capture.identity.scope, fixture.request.scope);
    assert_eq!(capture.identity.capture_id, fixture.request.capture_id);
}

#[tokio::test]
async fn copied_append_admission_requires_private_authority_and_exact_source_format() {
    let _capture = crate::device_capture::CAPTURE_TEST_LOCK.lock().await;
    let fixture = Fixture::new(true).await;
    let prepared = hypr_loofah_client::CaptureAppendPreparation::prepare(
        fixture._temp.path().join("isolated-preparation"),
        fixture.request.clone(),
    )
    .await
    .unwrap();
    let admission = fixture
        .client
        .prepare_capture_append(&prepared, fixture.lifetime.clone())
        .await
        .unwrap();
    let request = crate::device_capture::remote::DeviceCaptureStartRequest {
        request_id: fixture.request.request_id.clone(),
        scope: fixture.request.scope.clone(),
        target: CaptureTarget::AppendSession {
            session_id: "note".into(),
            admission: admission.clone(),
        },
        format: admission.baseline.format.clone(),
        processing: None,
    };
    assert!(
        fixture
            .host
            .start_scoped(
                fixture.profile.clone(),
                Some(fixture.client.clone()),
                request.clone()
            )
            .await
            .is_err()
    );
    let authority = fixture
        .client
        .bind_capture_append(
            fixture.profile.clone(),
            fixture.request.clone(),
            fixture.lifetime.clone(),
        )
        .await
        .unwrap();
    let host = DeviceCaptureHost::with_driver(
        fixture.spool.clone(),
        Arc::new(Observer::default()),
        fixture.driver.clone(),
    );
    let mut input = CaptureStartIntent {
        profile_id: fixture.profile.profile().await.id,
        identity: CaptureIdentity {
            scope: request.scope,
            capture_id: fixture.request.capture_id.clone(),
            request_id: request.request_id,
            target: request.target,
        },
        format: CaptureFormat::stereo(),
        processing: None,
    };
    assert!(
        host.start_authorized(input.clone(), Some(authority.clone()))
            .is_err()
    );
    input.format = admission.baseline.format;
    assert!(host.start(input.clone()).is_err());
    input.identity.request_id = uuid::Uuid::new_v4().to_string();
    assert!(host.start_authorized(input, Some(authority)).is_err());
    assert_eq!(fixture.driver.starts.load(Ordering::Acquire), 0);
    assert!(
        fixture
            .spool
            .inventory(fixture.request.scope.clone())
            .await
            .unwrap()
            .is_empty()
    );
}

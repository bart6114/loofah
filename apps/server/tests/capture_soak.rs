//! Real wall-clock acceptance, excluded from ordinary CI. Use a new disposable
//! LOOFAH_SOAK_ROOT and LOOFAH_SOAK_SECONDS=7200; run this target with --ignored
//! --exact capture_soak_wall_clock. Set LOOFAH_SOAK_MODEL to a provisioned model
//! and build with cpu-transcription to require recognizable speech and repair.
//! A shorter duration (minimum 60s) scales the outage schedule and is rehearsal
//! only. Artifacts and JSONL resource/progress evidence remain in the given root.
#![cfg(target_os = "linux")]

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use hypr_loofah_client::{
    ConnectionProfiles, CredentialFuture, CredentialStore, RemoteClient, RemoteOperation,
    RemoteReply, capture_spool::*, capture_upload::*,
};
use hypr_loofah_protocol::*;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const CA: &[u8] = include_bytes!("../../../crates/loofah-client/tests/fixtures/localhost-ca.pem");
const CERT: &[u8] = include_bytes!("../../../crates/loofah-client/tests/fixtures/localhost.pem");
const KEY: &[u8] = include_bytes!("../../../crates/loofah-client/tests/fixtures/localhost.key");
fn uid() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
fn hash(bytes: impl AsRef<[u8]>) -> String {
    Sha256::digest(bytes.as_ref())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn record(root: &Path, value: Value) {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("events.jsonl"))
        .unwrap();
    let mut bytes = serde_json::to_vec(&json!({"at_ms":millis(),"event":value})).unwrap();
    bytes.push(b'\n');
    f.write_all(&bytes).unwrap();
}
fn root() -> PathBuf {
    PathBuf::from(std::env::var("LOOFAH_SOAK_ROOT").expect("set a new disposable LOOFAH_SOAK_ROOT"))
}
fn seconds() -> u64 {
    std::env::var("LOOFAH_SOAK_SECONDS")
        .map(|s| s.parse().unwrap())
        .unwrap_or(7200)
}
const SPEECH: &[u8] = include_bytes!("../../../crates/data/src/english_2/audio.wav");
fn speech() -> &'static [f32] {
    static SAMPLES: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    SAMPLES.get_or_init(|| {
        assert_eq!(
            hash(SPEECH),
            "c319b4abca767b124e41432d364fd7df006cb26bb79d09326c487d606a134e6e"
        );
        let mut reader = hound::WavReader::new(std::io::Cursor::new(SPEECH)).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().channels, 1);
        let samples = reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect::<Vec<_>>();
        assert_eq!(samples.len(), 480_000);
        samples
    })
}
fn pcm(second: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(128_000);
    for offset in 0..16_000u64 {
        let frame = second * 16_000 + offset;
        let speech_frame = (second % 300) * 16_000 + offset;
        // One committed 30s speech fixture every five minutes. Quiet exact binary
        // markers retain distinct channels and expose ordering errors between speech.
        let mic = if speech_frame < 480_000 {
            speech()[speech_frame as usize]
        } else {
            ((frame % 1021) as f32 - 510.0) / 67_108_864.0
        };
        let system = ((frame.wrapping_mul(17) % 997) as f32 - 498.0) / 134_217_728.0;
        bytes.extend(mic.to_le_bytes());
        bytes.extend(system.to_le_bytes());
    }
    bytes
}

struct Credentials;
impl CredentialStore for Credentials {
    fn load<'a>(&'a self, _: &'a str) -> CredentialFuture<'a, Option<String>> {
        Box::pin(async {
            Ok(Some(
                json!({"schema":1,"refresh_token":"soak-fixture-refresh"}).to_string(),
            ))
        })
    }
    fn save<'a>(&'a self, _: &'a str, _: Option<&'a str>) -> CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}
async fn client(origin: &str) -> Arc<RemoteClient> {
    RemoteClient::discover_fixture(origin, Arc::new(Credentials), CA)
        .await
        .unwrap()
}
async fn me(client: &Arc<RemoteClient>) -> Me {
    match client
        .execute(RemoteOperation::Me, &uid(), CancellationToken::new())
        .await
        .unwrap()
    {
        RemoteReply::Me(value) => value,
        _ => panic!("me reply"),
    }
}
fn token(issuer: &str) -> String {
    let mut h = Header::new(Algorithm::RS256);
    h.typ = Some("at+jwt".into());
    h.kid = Some("test-1".into());
    jsonwebtoken::encode(&h,&json!({"iss":issuer,"sub":"soak-operator","aud":"loofah-api","exp":millis()/1000+3600,"iat":millis()/1000,"scope":"openid profile loofah:access","name":"Soak fixture"}),&EncodingKey::from_rsa_pem(include_bytes!("fixtures/oidc-private.pem")).unwrap()).unwrap()
}
async fn provider() -> (String, tokio::task::JoinHandle<()>) {
    provider_at(0).await
}
async fn provider_at(port: u16) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let metadata = json!({"issuer":issuer,"jwks_uri":format!("{issuer}/keys"),"authorization_endpoint":format!("{issuer}/authorize"),"token_endpoint":format!("{issuer}/token"),"code_challenge_methods_supported":["S256"]});
    let keys: Value = serde_json::from_str(include_str!("fixtures/jwks.json")).unwrap();
    let signing = issuer.clone();
    let router=Router::new().route("/.well-known/openid-configuration",get(move||async move{Json(metadata)})).route("/keys",get(move||async move{Json(keys)})).route("/token",post(move||{let issuer=signing.clone();async move{Json(json!({"access_token":token(&issuer),"token_type":"Bearer","expires_in":300,"refresh_token":"soak-fixture-refresh"}))}}));
    (
        issuer,
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
    )
}
struct Faults {
    upstream: RwLock<String>,
    offline: AtomicBool,
    lose_chunk: AtomicBool,
    lose_publish: AtomicBool,
    dropped_chunks: AtomicU64,
    dropped_publications: AtomicU64,
    requests: AtomicU64,
    root: PathBuf,
    http: reqwest::Client,
}
async fn forward(State(state): State<Arc<Faults>>, request: Request) -> Response {
    state.requests.fetch_add(1, Ordering::SeqCst);
    if state.offline.load(Ordering::SeqCst) {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE,Json(json!({"code":"unavailable","message":"operator-injected link outage","retryable":true}))).into_response();
    }
    let path = request.uri().to_string();
    let method = request.method().clone();
    let headers = request.headers().clone();
    let bytes = axum::body::to_bytes(request.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let mut outbound = state.http.request(
        method.clone(),
        format!("{}{path}", state.upstream.read().await),
    );
    for (name, value) in &headers {
        if name != "host" && name != "content-length" {
            outbound = outbound.header(name, value);
        }
    }
    if path.ends_with("/retirement") {
        outbound = outbound.timeout(Duration::from_secs(300));
    }
    let response = match outbound.body(bytes).send().await {
        Ok(v) => v,
        Err(_) => return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let status = response.status();
    let headers = response.headers().clone();
    if headers
        .get("content-type")
        .is_some_and(|value| value.as_bytes().starts_with(b"text/event-stream"))
    {
        use futures_util::StreamExt;
        let mut upstream = response.bytes_stream();
        let stream = async_stream::stream! {loop {
            tokio::select! {
                bytes=upstream.next()=>match bytes {Some(Ok(bytes))=>yield Ok::<_,std::io::Error>(bytes),Some(Err(_))=>{yield Err(std::io::Error::other("upstream SSE disconnected"));break},None=>break},
                _=tokio::time::sleep(Duration::from_secs(1))=>{}
            }
            if state.offline.load(Ordering::SeqCst) {yield Err(std::io::Error::other("injected SSE link outage"));break;}
        }};
        return Response::builder()
            .status(status)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap();
    }
    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(_) => return axum::http::StatusCode::BAD_GATEWAY.into_response(),
    };
    let lost_chunk = status.is_success()
        && method == "PUT"
        && path.contains("/chunks/")
        && state.lose_chunk.swap(false, Ordering::SeqCst);
    let lost_publish = status.is_success()
        && path.ends_with("/publish")
        && state.lose_publish.swap(false, Ordering::SeqCst);
    if lost_chunk || lost_publish {
        if lost_chunk {
            state.dropped_chunks.fetch_add(1, Ordering::SeqCst);
        } else {
            state.dropped_publications.fetch_add(1, Ordering::SeqCst);
        }
        record(
            &state.root,
            json!({"lost_committed_reply":path,"status":status.as_u16()}),
        );
        return Response::builder()
            .status(status)
            .body(Body::from_stream(futures_util::stream::once(async {
                Err::<Bytes, _>(std::io::Error::other("injected committed response loss"))
            })))
            .unwrap();
    }
    let mut builder = Response::builder().status(status);
    for (name, value) in &headers {
        if name != "transfer-encoding" && name != "connection" {
            builder = builder.header(name, value);
        }
    }
    builder.body(Body::from(bytes)).unwrap()
}
async fn proxy(root: &Path) -> (String, Arc<Faults>, Vec<tokio::task::JoinHandle<()>>) {
    proxy_at(root, 0).await
}
async fn proxy_at(
    root: &Path,
    port: u16,
) -> (String, Arc<Faults>, Vec<tokio::task::JoinHandle<()>>) {
    let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_address = http.local_addr().unwrap();
    let state = Arc::new(Faults {
        upstream: RwLock::new(String::new()),
        offline: AtomicBool::new(false),
        lose_chunk: AtomicBool::new(true),
        lose_publish: AtomicBool::new(true),
        dropped_chunks: AtomicU64::new(0),
        dropped_publications: AtomicU64::new(0),
        requests: AtomicU64::new(0),
        root: root.into(),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap(),
    });
    let router = Router::new()
        .fallback(any(forward))
        .with_state(state.clone());
    let http_task = tokio::spawn(async move { axum::serve(http, router).await.unwrap() });
    use tokio_rustls::rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    };
    let certs = CertificateDer::pem_slice_iter(CERT)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_slice(KEY).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .unwrap();
    let tls = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let origin = format!("https://localhost:{}/", tls.local_addr().unwrap().port());
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let tls_task = tokio::spawn(async move {
        loop {
            let (socket, _) = tls.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut secure) = acceptor.accept(socket).await {
                    if let Ok(mut plain) = tokio::net::TcpStream::connect(http_address).await {
                        let _ = tokio::io::copy_bidirectional(&mut secure, &mut plain).await;
                    }
                }
            });
        }
    });
    (origin, state, vec![http_task, tls_task])
}
async fn server(root: &Path, origin: &str, issuer: &str, faults: &Faults) -> tokio::process::Child {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let id = uid();
    let config = root.join(format!("server-{id}.json"));
    let mut settings = json!({"storage_root":root.join("server"),"cache_root":root.join("cache"),"listen":address,"public_url":origin,"oidc":{"issuer":issuer,"audience":"loofah-api","client_id":"loofah-desktop","required_scope":"loofah:access"},"allow_insecure_loopback":true});
    if let Ok(model) = std::env::var("LOOFAH_SOAK_MODEL") {
        let path = PathBuf::from(model);
        assert_eq!(fs::metadata(&path).unwrap().len(), 81768585);
        let bytes = fs::read(&path).unwrap();
        assert_eq!(
            hash(&bytes),
            "c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9"
        );
        settings["transcription"] = json!({"model":"soak-base-q8","model_path":path,"model_size_bytes":bytes.len(),"model_sha256":hash(bytes),"workload_groups":1});
    }
    fs::write(&config, serde_json::to_vec(&settings).unwrap()).unwrap();
    let log = fs::File::create(root.join(format!("server-{id}.log"))).unwrap();
    let mut child = tokio::process::Command::new(
        std::env::var_os("LOOFAH_SOAK_SERVER_BIN")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_loofah-server").into()),
    )
    .arg("--config")
    .arg(config)
    .stdout(Stdio::from(log.try_clone().unwrap()))
    .stderr(Stdio::from(log))
    .kill_on_drop(true)
    .spawn()
    .unwrap();
    let upstream = format!("http://{address}");
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "server exited; inspect retained log"
            );
            if reqwest::get(format!("{upstream}/health"))
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    *faults.upstream.write().await = upstream;
    record(root, json!({"server_started_pid":child.id()}));
    child
}
async fn stop(child: &mut tokio::process::Child) {
    let status = tokio::process::Command::new("/bin/kill")
        .args(["-TERM", &child.id().unwrap().to_string()])
        .status()
        .await
        .unwrap();
    assert!(status.success());
    assert!(
        tokio::time::timeout(Duration::from_secs(60), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}
fn spawn_worker(root: &Path, mode: &str, realtime: bool) -> tokio::process::Child {
    let log = fs::File::create(root.join(format!("device-{mode}.log"))).unwrap();
    tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "capture_soak_device_worker",
            "--nocapture",
        ])
        .env("LOOFAH_SOAK_WORKER", mode)
        .env("LOOFAH_SOAK_REALTIME", if realtime { "1" } else { "0" })
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}
fn rss(pid: u32) -> Value {
    let text = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    json!(
        text.lines()
            .filter(|l| l.starts_with("VmRSS:") || l.starts_with("VmHWM:"))
            .collect::<Vec<_>>()
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real wall-clock operator acceptance; set a new LOOFAH_SOAK_ROOT"]
async fn capture_soak_wall_clock() {
    run_capture_acceptance(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "non-paced full-size publication/memory replay; never qualifies wall-clock capture"]
async fn capture_publication_memory_replay() {
    run_capture_acceptance(false).await;
}

async fn run_capture_acceptance(realtime: bool) {
    let root = root();
    assert!(!root.exists(), "acceptance requires new disposable storage");
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let duration = seconds();
    assert!(duration >= 60);
    record(
        &root,
        json!({"start":true,"seconds":duration,"paced":realtime,"paced_prefix_seconds":if realtime {duration} else {duration.min(30)},"real_time_qualification":realtime&&duration>=7200,"full_size_memory_replay":!realtime&&duration>=7200,"generated_audio_not_physical_device":true,"speech_fixture_sha256":hash(SPEECH),"model":std::env::var("LOOFAH_SOAK_MODEL").ok()}),
    );
    let (issuer, oidc) = provider().await;
    let (origin, faults, proxy_tasks) = proxy(&root).await;
    let mut server = server(&root, &origin, &issuer, &faults).await;
    let connection = client(&origin).await;
    connection
        .execute(RemoteOperation::Provision, &uid(), CancellationToken::new())
        .await
        .unwrap();
    let member = me(&connection).await;
    let created = connection
        .execute(
            RemoteOperation::CreateSession {
                vault_id: member.personal_vault_id.clone(),
                input: CreateSessionRequest {
                    title: "Real-time durability fixture".into(),
                    markdown: "Disposable generated capture".into(),
                },
            },
            &uid(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let RemoteReply::Session(created) = created else {
        panic!("create reply")
    };
    let identity = CaptureIdentity {
        scope: CaptureScope {
            server: origin.clone(),
            member_id: member.member.id,
            vault_id: member.personal_vault_id,
        },
        capture_id: uid(),
        request_id: uid(),
        target: CaptureTarget::ExistingSession {
            session_id: created.meta.id.clone(),
            expected_audio_sha256: None,
        },
    };
    let profiles = ConnectionProfiles::new(root.join("profiles")).unwrap();
    let profile = profiles.create("soak", &origin).await.unwrap();
    let mut saved = profile.profile().await;
    saved.member_id = Some(identity.scope.member_id.clone());
    saved.vault_id = Some(identity.scope.vault_id.clone());
    saved
        .credential_accounts
        .push(connection.credential_account().into());
    profile.save(saved).await.unwrap();
    fs::write(
        root.join("identity.json"),
        serde_json::to_vec(&identity).unwrap(),
    )
    .unwrap();
    let mut producer = spawn_worker(&root, "produce", realtime);
    let start = Instant::now();
    let long = if duration >= 7200 { 600 } else { duration / 5 };
    let events = [
        (duration / 10, 3),
        (duration / 3, long),
        (duration * 3 / 4, 5),
    ];
    let mut previous = false;
    let mut restarted = false;
    let mut next_log = 0;
    let producer_budget = if realtime { duration + 5 } else { 1800 };
    while start.elapsed().as_secs() < producer_budget {
        let elapsed = start.elapsed().as_secs();
        let offline = realtime
            && events
                .iter()
                .any(|(at, length)| elapsed >= *at && elapsed < at + length);
        faults.offline.store(offline, Ordering::SeqCst);
        if offline != previous {
            record(&root, json!({"link_offline":offline,"elapsed":elapsed}));
            previous = offline;
        }
        if realtime && !restarted && elapsed >= duration / 2 + long {
            faults.offline.store(true, Ordering::SeqCst);
            stop(&mut server).await;
            server = self::server(&root, &origin, &issuer, &faults).await;
            restarted = true;
            faults.lose_chunk.store(true, Ordering::SeqCst);
        }
        if elapsed >= next_log {
            record(
                &root,
                json!({"elapsed":elapsed,"producer_rss":rss(producer.id().unwrap_or(0)),"server_rss":rss(server.id().unwrap()),"proxy_rss":rss(std::process::id()),"http_requests":faults.requests.load(Ordering::SeqCst)}),
            );
            next_log = elapsed + 30;
        }
        if let Some(status) = producer.try_wait().unwrap() {
            assert!(
                status.success(),
                "producer failed; inspect device-produce.log"
            );
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(60), producer.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    if realtime {
        assert!(start.elapsed() >= Duration::from_secs(duration));
    } else {
        stop(&mut server).await;
        server = self::server(&root, &origin, &issuer, &faults).await;
        faults.lose_chunk.store(true, Ordering::SeqCst);
        restarted = true;
    }
    assert!(restarted);
    faults.offline.store(false, Ordering::SeqCst);
    record(
        &root,
        json!({"producer_process_exited":true,"wall_elapsed_ms":start.elapsed().as_millis()}),
    );
    let mut resume = spawn_worker(&root, "resume", realtime);
    let resume_deadline = Instant::now() + Duration::from_secs(3600);
    loop {
        tokio::select! {
            status=resume.wait()=>{assert!(status.unwrap().success(),"resume failed; inspect device-resume.log");break;},
            _=tokio::time::sleep(Duration::from_secs(30))=>{
                record(&root,json!({"resume_rss":rss(resume.id().unwrap()),"server_rss":rss(server.id().unwrap()),"http_requests":faults.requests.load(Ordering::SeqCst)}));
                assert!(Instant::now()<resume_deadline,"bounded recovery deadline exceeded");
            }
        }
    }
    let publication: CapturePublication =
        serde_json::from_slice(&fs::read(root.join("publication.json")).unwrap()).unwrap();
    assert_eq!(publication.source.frames, duration * 16_000);
    assert_eq!(publication.source.chunk_count, duration);
    assert_eq!(publication.source.pcm_bytes, duration * 128_000);
    let canonical = root
        .join("server/vaults")
        .join(&identity.scope.vault_id)
        .join("sessions")
        .join(&created.meta.id)
        .join("audio.wav");
    let mut raw = fs::File::open(&canonical).unwrap();
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let n = raw.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    let actual_hash: String = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(actual_hash, publication.canonical_audio_sha256);
    assert_eq!(
        fs::metadata(&canonical).unwrap().len(),
        publication.canonical_audio_bytes
    );
    let mut wav = hound::WavReader::open(&canonical).unwrap();
    assert_eq!(wav.spec().channels, 2);
    assert_eq!(wav.spec().sample_rate, 16_000);
    assert_eq!(wav.duration() as u64, duration * 16_000);
    let mut samples = wav.samples::<f32>();
    for second in 0..duration {
        for expected in pcm(second).chunks_exact(4) {
            assert_eq!(
                samples.next().unwrap().unwrap().to_le_bytes(),
                expected,
                "PCM mismatch at second {second}"
            );
        }
    }
    assert!(samples.next().is_none());
    assert!(faults.dropped_chunks.load(Ordering::SeqCst) >= if realtime { 2 } else { 1 });
    assert_eq!(faults.dropped_publications.load(Ordering::SeqCst), 1);
    if !realtime {
        let status = fs::read_to_string(format!("/proc/{}/status", server.id().unwrap())).unwrap();
        let peak: u64 = status
            .lines()
            .find_map(|line| line.strip_prefix("VmHWM:"))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let limit: u64 = std::env::var("LOOFAH_SOAK_SERVER_RSS_LIMIT_KIB")
            .map(|value| value.parse().unwrap())
            .unwrap_or(768 * 1024);
        record(
            &root,
            json!({"server_peak_rss_kib":peak,"server_rss_limit_kib":limit}),
        );
        assert!(
            peak <= limit,
            "publication/repair exceeded the server RSS budget: {peak} > {limit} KiB"
        );
    }
    let spool = CaptureSpool::open(root.join("device"), CaptureSpoolLimits::default())
        .await
        .unwrap();
    let admission = spool.admit_retirement(identity.clone()).await.unwrap();
    let snapshot = admission.snapshot().clone();
    let request = CaptureRetirementRequest {
        cleanup_request_id: uid(),
        identity: identity.clone(),
        format: snapshot.format.clone(),
        source: snapshot.seal.clone().unwrap(),
        publication: publication.clone(),
        processing: snapshot.processing.map(|p| p.request),
    };
    let admission = admission.declare_request(request.clone()).await.unwrap();
    let generation = profile.generation().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(1800);
    let proof = loop {
        drop(profile.guard_generation(&generation).await.unwrap());
        match connection
            .verify_capture_retirement(
                profile.clone(),
                profile.generation().await.unwrap(),
                request.clone(),
                CancellationToken::new(),
            )
            .await
        {
            Ok(proof) => break proof,
            Err(error)
                if error.retryable
                    && matches!(
                        error.code,
                        ErrorCode::Unavailable | ErrorCode::ResourceLimit
                    ) =>
            {
                record(
                    &root,
                    json!({"explicit_same_uuid_cleanup_retry":request.cleanup_request_id,"error":error}),
                );
                assert!(Instant::now() < deadline, "cleanup proof deadline exceeded");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(error) => panic!("cleanup proof rejected: {error:?}"),
        }
    };
    drop(profile.guard_generation(&generation).await.unwrap());
    let retired = proof
        .retire_device(admission, JoinedDeviceProcesses)
        .await
        .unwrap();
    assert_eq!(
        retired.retirement.as_ref().unwrap().state,
        CaptureRetirementState::Retired
    );
    assert_eq!(retired.retirement.as_ref().unwrap().remaining_chunks, 0);
    assert!(spool.resume(identity.clone()).await.is_err());
    record(
        &root,
        json!({"verified_device_retirement":retired.retirement}),
    );
    drop(spool);
    record(
        &root,
        json!({"accepted":true,"seconds":duration,"paced":realtime,"real_time_qualification":realtime&&duration>=7200,"frames":publication.source.frames,"canonical_sha256":actual_hash,"lost_chunk_replies":faults.dropped_chunks.load(Ordering::SeqCst),"lost_publication_replies":faults.dropped_publications.load(Ordering::SeqCst),"http_requests":faults.requests.load(Ordering::SeqCst)}),
    );
    stop(&mut server).await;
    connection.disconnect();
    oidc.abort();
    for task in proxy_tasks {
        task.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "child of capture_soak_wall_clock"]
async fn capture_soak_device_worker() {
    let Ok(mode) = std::env::var("LOOFAH_SOAK_WORKER") else {
        return;
    };
    let root = root();
    let duration = seconds();
    let realtime = match std::env::var("LOOFAH_SOAK_REALTIME").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => panic!("producer requires explicit pacing authority from its parent harness"),
    };
    let identity: CaptureIdentity =
        serde_json::from_slice(&fs::read(root.join("identity.json")).unwrap()).unwrap();
    let client = client(&identity.scope.server).await;
    assert_eq!(me(&client).await.member.id, identity.scope.member_id);
    let spool = CaptureSpool::open(root.join("device"), CaptureSpoolLimits::default())
        .await
        .unwrap();
    let writer = if mode == "produce" {
        let options = std::env::var("LOOFAH_SOAK_MODEL")
            .ok()
            .map(|_| CaptureProcessingOptions {
                model: Some("soak-base-q8".into()),
                languages: vec!["en".into()],
                keywords: vec![],
                summary: None,
            });
        let writer = spool
            .begin_with_processing(identity.clone(), CaptureFormat::stereo(), options)
            .await
            .unwrap();
        writer
            .record_first_audio_observation(millis())
            .await
            .unwrap();
        writer
    } else {
        spool.resume(identity.clone()).await.unwrap()
    };
    let uploader = CaptureUploader::new(1).unwrap();
    let ticket = uploader
        .start(
            client.clone(),
            writer.clone(),
            CaptureUploadBinding {
                profile_id: ConnectionProfiles::new(root.join("profiles"))
                    .unwrap()
                    .list()
                    .await
                    .unwrap()[0]
                    .id
                    .clone(),
                scope: identity.scope.clone(),
                credential_account: client.credential_account().into(),
            },
            Box::new(()),
        )
        .unwrap();
    let processing_stop = CancellationToken::new();
    let mut processing_observer = None;
    if mode == "produce" {
        if let Some(processing) = writer.snapshot().await.unwrap().processing {
            client
                .begin_capture(
                    &CaptureIngestRequest {
                        identity: identity.clone(),
                        format: CaptureFormat::stereo(),
                        first_admitted_audio_observed_at_ms: writer
                            .snapshot()
                            .await
                            .unwrap()
                            .first_admitted_audio_observed_at_ms
                            .unwrap(),
                    },
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            let declared = client
                .declare_capture_processing(
                    &identity,
                    &processing.request,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            record(&root, json!({"processing_declared":declared}));
            let observer_client = client.clone();
            let observer_identity = identity.clone();
            let observer_root = root.clone();
            let stop = processing_stop.clone();
            processing_observer = Some(tokio::spawn(observe_processing(
                observer_client,
                observer_identity,
                processing.request,
                observer_root,
                stop,
            )));
        }
        let start = tokio::time::Instant::now();
        let mut max_lateness = 0u128;
        for second in 0..duration {
            let deadline = start + Duration::from_secs(second + 1);
            if realtime || second < 30 {
                tokio::time::sleep_until(deadline).await;
                max_lateness = max_lateness.max(deadline.elapsed().as_millis());
            }
            writer
                .append_chunk(second, 16_000, pcm(second))
                .await
                .unwrap();
            if second % 30 == 0 {
                record(
                    &root,
                    json!({"produced_seconds":second+1,"upload":ticket.current(),"max_lateness_ms":max_lateness}),
                );
            }
            assert!(
                !ticket.is_finished(),
                "uploader exited during capture: {:?}",
                ticket.current()
            );
        }
        assert!(
            max_lateness < 1000,
            "producer missed a full real-time chunk deadline: {max_lateness}ms"
        );
        // Stop the original network owner before sealing: publication must be recovered by a new process.
        uploader.wait_shutdown().await;
        writer.seal().await.unwrap();
        record(
            &root,
            json!({"sealed_before_process_exit":writer.snapshot().await.unwrap(),"max_lateness_ms":max_lateness}),
        );
    } else {
        let snapshot = writer.snapshot().await.unwrap();
        assert!(snapshot.seal.is_some());
        assert_eq!(snapshot.chunk_count, duration);
        let mut previous = capture_chain_seed(&snapshot.capture_fingerprint);
        for second in 0..duration {
            let chunk = writer.read_chunk(second).await.unwrap();
            assert_eq!(chunk.bytes, pcm(second));
            assert_eq!(chunk.descriptor.sha256, hash(&chunk.bytes));
            assert_eq!(chunk.descriptor.sequence, second);
            assert_eq!(chunk.descriptor.first_frame, second * 16_000);
            assert_eq!(chunk.descriptor.pcm_offset, second * 128_000);
            assert_eq!(chunk.descriptor.frames, 16_000);
            assert_eq!(chunk.descriptor.bytes, 128_000);
            assert_eq!(
                chunk.descriptor.digest,
                capture_chunk_digest(&chunk.descriptor)
            );
            assert_eq!(chunk.descriptor.previous_digest, previous);
            previous = chunk.descriptor.digest;
        }
        assert_eq!(previous, snapshot.chain_digest);
        ticket.source_finished();
        let done = tokio::time::timeout(Duration::from_secs(1800), ticket.wait_stopped())
            .await
            .unwrap();
        assert_eq!(done.phase, CaptureUploadPhase::Verified, "{done:?}");
        let publication = done.publication.unwrap();
        fs::write(
            root.join("publication.json"),
            serde_json::to_vec(&publication).unwrap(),
        )
        .unwrap();
        if let Some(processing) = snapshot.processing {
            let end = Instant::now() + Duration::from_secs(1800);
            loop {
                let status = client
                    .capture_processing_status(
                        &identity,
                        &processing.request,
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap();
                record(&root, json!({"processing_status":status}));
                if status.state.is_terminal() {
                    assert!(
                        status.state == CaptureProcessingState::Succeeded,
                        "real CPU processing did not finish safely: {status:?}"
                    );
                    assert!(
                        status.live_generation_id.is_some(),
                        "real live generation must precede repair"
                    );
                    assert!(
                        status.repair_job_id.is_some(),
                        "outage/restart must require canonical repair"
                    );
                    break;
                }
                assert!(Instant::now() < end);
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
        uploader.wait_shutdown().await;
        if std::env::var("LOOFAH_SOAK_MODEL").is_ok() {
            let CaptureTarget::ExistingSession { session_id, .. } = &identity.target else {
                panic!("fixture target")
            };
            let result = client
                .execute(
                    RemoteOperation::Transcripts {
                        vault_id: identity.scope.vault_id.clone(),
                        session_id: session_id.clone(),
                    },
                    &uid(),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            let RemoteReply::Transcripts(transcripts) = result else {
                panic!("transcript reply")
            };
            let mut ids = std::collections::HashSet::new();
            for transcript in &transcripts {
                assert_eq!(&transcript.session_id, session_id);
                for word in &transcript.words {
                    assert!(word.start_ms.is_finite() && word.end_ms.is_finite());
                    assert!(
                        word.start_ms >= 0.0
                            && word.end_ms >= word.start_ms
                            && word.end_ms <= duration as f64 * 1000.0
                    );
                    assert!(word.channel == 0.0 || word.channel == 1.0);
                    if let Some(id) = &word.id {
                        assert!(ids.insert(id), "duplicate canonical word identity");
                    }
                }
            }
            for window in [0, ((duration - 1) / 300) * 300] {
                let text = transcripts
                    .iter()
                    .flat_map(|t| &t.words)
                    .filter(|w| {
                        w.channel == 0.0
                            && w.start_ms >= window as f64 * 1000.0
                            && w.start_ms < (window + 45) as f64 * 1000.0
                    })
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase();
                assert!(
                    text.contains("hello"),
                    "English2 microphone speech missing hello in window {window}: {text}"
                );
                assert!(
                    text.contains("jersey"),
                    "English2 microphone speech missing jersey in window {window}: {text}"
                );
            }
            fs::write(
                root.join("verified-transcripts.json"),
                serde_json::to_vec(&transcripts).unwrap(),
            )
            .unwrap();
            record(
                &root,
                json!({"semantic_speech_verified":["hello","jersey"],"speech_fixture_sha256":hash(SPEECH)}),
            );
        }
    }
    processing_stop.cancel();
    if let Some(observer) = processing_observer {
        observer.await.unwrap();
    }
    writer.wait_for_io().await;
    drop(ticket);
    drop(uploader);
    drop(writer);
    drop(spool);
    client.disconnect();
}

struct JoinedDeviceProcesses;
impl CaptureRetirementAuthority for JoinedDeviceProcesses {
    fn check_current(&self) -> hypr_loofah_client::Result<()> {
        Ok(())
    }
}

async fn observe_processing(
    client: Arc<RemoteClient>,
    identity: CaptureIdentity,
    request: CaptureProcessingRequest,
    root: PathBuf,
    stop: CancellationToken,
) {
    let mut subscription = None;
    let sequences = Arc::new(std::sync::Mutex::new(std::collections::BTreeSet::new()));
    let invalid = Arc::new(AtomicBool::new(false));
    loop {
        tokio::select! {biased; _=stop.cancelled()=>break, _=tokio::time::sleep(Duration::from_secs(15))=>{}}
        let request = client.capture_processing_status(&identity, &request, stop.clone());
        let status = tokio::select! {biased; _=stop.cancelled()=>break, status=tokio::time::timeout(Duration::from_secs(10),request)=>status};
        let Ok(Ok(status)) = status else {
            record(&root, json!({"live_observer_disconnected":true}));
            continue;
        };
        if subscription.is_none() {
            if let (Some(generation), Some(session)) =
                (&status.live_generation_id, &status.session_id)
            {
                let binding = hypr_loofah_client::CaptureLiveBinding {
                    capture_id: identity.capture_id.clone(),
                    capture_fingerprint: request_fingerprint(&identity),
                    generation_id: generation.clone(),
                    session_id: session.clone(),
                    format: CaptureFormat::stereo(),
                };
                let observed = sequences.clone();
                let bad = invalid.clone();
                let output = root.clone();
                subscription = Some(
                    client
                        .subscribe_capture_live(
                            identity.scope.vault_id.clone(),
                            binding,
                            None,
                            move |event| {
                                if let hypr_loofah_client::CaptureLiveEvent::Page { page } = &event
                                {
                                    let mut seen = observed.lock().unwrap();
                                    for caption in &page.captions {
                                        if seen.len() >= 65_536 || !seen.insert(caption.sequence) {
                                            bad.store(true, Ordering::SeqCst);
                                            return false;
                                        }
                                    }
                                }
                                record(&output, json!({"live_sse":event}));
                                true
                            },
                        )
                        .unwrap(),
                );
            }
        }
        record(&root, json!({"live_processing_observation":status}));
    }
    if let Some(subscription) = subscription {
        subscription.close().await;
    }
    assert!(
        !invalid.load(Ordering::SeqCst),
        "duplicate or over-budget live caption delivery"
    );
    let count = sequences.lock().unwrap().len();
    assert!(
        count > 0,
        "actual SDK SSE must deliver native captions before stop"
    );
    record(&root, json!({"live_sse_unique_caption_sequences":count}));
}
fn request_fingerprint(identity: &CaptureIdentity) -> String {
    capture_fingerprint(identity, &CaptureFormat::stereo())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit existing failed-run cleanup retry, no capture or inference dispatch"]
async fn capture_retirement_existing_replay() {
    let root = root().canonicalize().unwrap();
    let identity: CaptureIdentity =
        serde_json::from_slice(&fs::read(root.join("identity.json")).unwrap()).unwrap();
    let saved_config = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("server-")
                && path.extension().is_some_and(|ext| ext == "json")
        })
        .unwrap();
    let config: Value = serde_json::from_slice(&fs::read(saved_config).unwrap()).unwrap();
    let original_issuer = config["oidc"]["issuer"].as_str().unwrap();
    let (issuer, oidc) = provider_at(
        reqwest::Url::parse(original_issuer)
            .unwrap()
            .port()
            .unwrap(),
    )
    .await;
    assert_eq!(issuer, original_issuer);
    let (origin, faults, proxy_tasks) = proxy_at(
        &root,
        reqwest::Url::parse(&identity.scope.server)
            .unwrap()
            .port()
            .unwrap(),
    )
    .await;
    assert_eq!(origin, identity.scope.server);
    let mut server = server(&root, &origin, &issuer, &faults).await;
    let connection = client(&origin).await;
    assert_eq!(me(&connection).await.member.id, identity.scope.member_id);
    let profiles = ConnectionProfiles::new(root.join("profiles")).unwrap();
    let profile = profiles.open("soak").await.unwrap();
    let spool = CaptureSpool::open(root.join("device"), CaptureSpoolLimits::default())
        .await
        .unwrap();
    let admission = spool.admit_retirement(identity.clone()).await.unwrap();
    let request = admission
        .snapshot()
        .retirement
        .as_ref()
        .expect("original cleanup declaration must exist")
        .request
        .clone();
    assert_eq!(request.identity, identity);
    let before = connection
        .capture_processing_status(
            &identity,
            request.processing.as_ref().unwrap(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(before.state, CaptureProcessingState::Succeeded);
    record(
        &root,
        json!({"explicit_retirement_resume":request.cleanup_request_id,"repair_job_id":before.repair_job_id,"paced":false,"new_capture_or_inference_dispatch":false}),
    );
    let started = Instant::now();
    let proof = connection
        .verify_capture_retirement(
            profile.clone(),
            profile.generation().await.unwrap(),
            request.clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let retired = proof
        .retire_device(admission, JoinedDeviceProcesses)
        .await
        .unwrap();
    assert_eq!(
        retired.retirement.as_ref().unwrap().state,
        CaptureRetirementState::Retired
    );
    assert_eq!(retired.retirement.as_ref().unwrap().remaining_chunks, 0);
    assert!(spool.resume(identity.clone()).await.is_err());
    let after = connection
        .capture_processing_status(
            &identity,
            request.processing.as_ref().unwrap(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        before, after,
        "cleanup must not restart or change completed processing"
    );
    record(
        &root,
        json!({"explicit_retirement_resume_accepted":true,"cleanup_request_id":request.cleanup_request_id,"repair_job_id":after.repair_job_id,"elapsed_ms":started.elapsed().as_millis(),"retirement":retired.retirement,"server_rss":rss(server.id().unwrap())}),
    );
    stop(&mut server).await;
    connection.disconnect();
    oidc.abort();
    for task in proxy_tasks {
        task.abort();
    }
}

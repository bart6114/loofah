//! SDK driver for backend_deployment_acceptance.py. This ignored test requires
//! the real packaged server and Caddy on a disposable Linux host, not a mock API.

use std::{path::Path, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    routing::{get, post},
};
use hypr_loofah_client::{
    CaptureAppendPreparation, ConnectionState, CredentialFuture, CredentialStore, RemoteClient,
    RemoteEvent, RemoteOperation, RemoteReply,
};
use hypr_loofah_protocol::*;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const CA: &[u8] = include_bytes!("../../../crates/loofah-client/tests/fixtures/localhost-ca.pem");
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn cancel() -> CancellationToken {
    CancellationToken::new()
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn write(root: &Path, name: &str, value: Value) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(root.join(name))
        .unwrap();
    file.write_all(&serde_json::to_vec_pretty(&value).unwrap())
        .unwrap();
    file.sync_all().unwrap();
}
async fn marker(root: &Path, name: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            if let Ok(bytes) = tokio::fs::read(root.join(name)).await
                && let Ok(value) = serde_json::from_slice(&bytes)
            {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("coordinator marker timed out: {name}"))
}
struct Credentials;
impl CredentialStore for Credentials {
    fn load<'a>(&'a self, _: &'a str) -> CredentialFuture<'a, Option<String>> {
        Box::pin(async {
            Ok(Some(
                json!({"schema":1,"refresh_token":"deployment-only"}).to_string(),
            ))
        })
    }
    fn save<'a>(&'a self, _: &'a str, _: Option<&'a str>) -> CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}
async fn issuer() -> (String, CancellationToken, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let metadata = json!({"issuer":origin,"jwks_uri":format!("{origin}/keys"),
        "authorization_endpoint":format!("{origin}/authorize"),
        "token_endpoint":format!("{origin}/token"),"code_challenge_methods_supported":["S256"]});
    let keys: Value = serde_json::from_str(include_str!("fixtures/jwks.json")).unwrap();
    let signing = origin.clone();
    let router = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || async move { Json(metadata) }),
        )
        .route("/keys", get(move || async move { Json(keys) }))
        .route(
            "/token",
            post(move || {
                let origin = signing.clone();
                async move {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    let mut header = Header::new(Algorithm::RS256);
                    header.typ = Some("at+jwt".into());
                    header.kid = Some("test-1".into());
                    let token = jsonwebtoken::encode(
                        &header,
                        &json!({"iss":origin,"sub":"deployment-operator","aud":"loofah-api",
                        "exp":now+3600,"iat":now,"scope":"openid profile loofah:access"}),
                        &EncodingKey::from_rsa_pem(include_bytes!("fixtures/oidc-private.pem"))
                            .unwrap(),
                    )
                    .unwrap();
                    Json(
                        json!({"access_token":token,"token_type":"Bearer","expires_in":300,
                    "refresh_token":"deployment-only"}),
                    )
                }
            }),
        );
    let stop = cancel();
    let stopped = stop.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
            .unwrap();
    });
    (origin, stop, task)
}
async fn create(client: &Arc<RemoteClient>, vault: &str, title: &str) -> SessionDocument {
    let reply = client
        .execute(
            RemoteOperation::CreateSession {
                vault_id: vault.into(),
                input: CreateSessionRequest {
                    title: title.into(),
                    markdown: "Disposable deployment fixture; keep this note.".into(),
                },
            },
            &id(),
            cancel(),
        )
        .await
        .unwrap();
    let RemoteReply::Session(value) = reply else {
        panic!("session reply")
    };
    value
}
async fn event(receiver: &mut mpsc::Receiver<RemoteEvent>, matches: impl Fn(&RemoteEvent) -> bool) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let value = receiver.recv().await.expect("SSE listener closed");
            if matches(&value) {
                break;
            }
        }
    })
    .await
    .expect("expected Caddy SSE event");
}
async fn changed(receiver: &mut mpsc::Receiver<RemoteEvent>, vault: &str, session: &str) {
    event(receiver, |value| {
        matches!(value, RemoteEvent::Change { event }
        if event.vault_id == vault && matches!(&event.change,
            Change::IndexChanged { ids, .. } if ids.iter().any(|id| id == session)))
    })
    .await;
}
fn pcm(start: u32, frames: u32) -> Vec<u8> {
    (start..start + frames)
        .flat_map(|frame| {
            let mic = ((frame % 101) as f32 - 50.0) / 128.0;
            let system = ((frame % 97) as f32 - 48.0) / 256.0;
            [mic.to_le_bytes(), system.to_le_bytes()].concat()
        })
        .collect()
}
async fn publish(
    client: &Arc<RemoteClient>,
    input: &CaptureIngestRequest,
    parts: &[Vec<u8>],
) -> (CaptureSeal, CaptureIngestStatus) {
    let mut prefix = client.begin_capture(input, cancel()).await.unwrap().prefix;
    let vault = &input.identity.scope.vault_id;
    let capture = &input.identity.capture_id;
    for bytes in parts {
        let mut descriptor = CaptureChunkDescriptor {
            capture_fingerprint: prefix.capture_fingerprint.clone(),
            sequence: prefix.chunk_count,
            first_frame: prefix.frames,
            pcm_offset: prefix.pcm_bytes,
            frames: (bytes.len() / 8) as u32,
            bytes: bytes.len() as u32,
            sha256: hash(bytes),
            previous_digest: prefix.chain_digest.clone(),
            digest: String::new(),
        };
        descriptor.digest = capture_chunk_digest(&descriptor);
        prefix = client
            .append_capture_chunk(vault, capture, &descriptor, bytes.clone(), cancel())
            .await
            .unwrap();
        assert_eq!(
            prefix,
            client
                .append_capture_chunk(vault, capture, &descriptor, bytes.clone(), cancel())
                .await
                .unwrap()
        );
    }
    let seal = CaptureSeal {
        capture_fingerprint: prefix.capture_fingerprint,
        chunk_count: prefix.chunk_count,
        frames: prefix.frames,
        pcm_bytes: prefix.pcm_bytes,
        chain_digest: prefix.chain_digest,
        source_sha256: hash(&parts.concat()),
    };
    client
        .seal_capture(vault, capture, &seal, cancel())
        .await
        .unwrap();
    let published = client
        .publish_capture(vault, capture, &seal, cancel())
        .await
        .unwrap();
    assert_eq!(published.publication.as_ref().unwrap().source, seal);
    (seal, published)
}
async fn recording(
    client: &Arc<RemoteClient>,
    scope: &CaptureScope,
    session: &str,
    expected: &[u8],
) -> String {
    let lease = client
        .open_recording(&scope.vault_id, session, &id(), cancel())
        .await
        .unwrap()
        .unwrap();
    assert!(lease.metadata.size_bytes < 1024 * 1024);
    let mut bytes = Vec::new();
    while bytes.len() < lease.metadata.size_bytes as usize {
        let chunk = client
            .read_recording_chunk(
                &lease,
                bytes.len() as u64,
                (lease.metadata.size_bytes as usize - bytes.len()).min(4093),
                cancel(),
            )
            .await
            .unwrap();
        assert!(!chunk.is_empty());
        bytes.extend(chunk);
    }
    assert_eq!(hash(&bytes), lease.metadata.sha256);
    let reader = hound::WavReader::new(std::io::Cursor::new(&bytes)).unwrap();
    assert_eq!(
        reader.spec(),
        hound::WavSpec {
            channels: 2,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float
        }
    );
    let actual: Vec<u8> = reader
        .into_samples::<f32>()
        .flat_map(|value| value.unwrap().to_le_bytes())
        .collect();
    assert_eq!(
        actual, expected,
        "every PCM frame and both channel positions must match"
    );
    client
        .close_recording(&scope.vault_id, &lease.id, cancel())
        .await
        .unwrap();
    hash(&bytes)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable systemd host, packaged executable and actual Caddy TLS coordinator"]
async fn packaged_caddy_append_and_restart() {
    let root = std::path::PathBuf::from(
        std::env::var("LOOFAH_DEPLOYMENT_CONTROL").expect("coordinator required"),
    );
    assert!(root.is_absolute());
    let (issuer, stop, issuer_task) = issuer().await;
    write(&root, "issuer-ready.json", json!({"issuer":issuer}));
    let ready = marker(&root, "service-ready.json").await;
    let origin = ready["origin"].as_str().unwrap();
    let client = RemoteClient::discover_fixture(origin, Arc::new(Credentials), CA)
        .await
        .unwrap();
    client
        .execute(RemoteOperation::Provision, &id(), cancel())
        .await
        .unwrap();
    let RemoteReply::Me(member) = client
        .execute(RemoteOperation::Me, &id(), cancel())
        .await
        .unwrap()
    else {
        panic!("me reply")
    };
    let scope = CaptureScope {
        server: client.address().to_owned(),
        member_id: member.member.id,
        vault_id: member.personal_vault_id,
    };
    let (sender, mut receiver) = mpsc::channel(128);
    let subscription = client
        .subscribe(scope.vault_id.clone(), move |value| {
            sender.try_send(value).is_ok()
        })
        .unwrap();
    event(&mut receiver, |value| {
        matches!(
            value,
            RemoteEvent::Connection {
                state: ConnectionState::Connected,
                ..
            }
        )
    })
    .await;
    let note = create(&client, &scope.vault_id, "Packaged original recording").await;
    changed(&mut receiver, &scope.vault_id, &note.meta.id).await;
    let initial = CaptureIngestRequest {
        identity: CaptureIdentity {
            scope: scope.clone(),
            capture_id: id(),
            request_id: id(),
            target: CaptureTarget::ExistingSession {
                session_id: note.meta.id.clone(),
                expected_audio_sha256: None,
            },
        },
        format: CaptureFormat::stereo(),
        first_admitted_audio_observed_at_ms: 1_794_096_001_123,
    };
    let base = pcm(0, 16001);
    let (_, original) = publish(&client, &initial, std::slice::from_ref(&base)).await;
    let original_sha = recording(&client, &scope, &note.meta.id, &base).await;
    assert_eq!(
        original.publication.unwrap().canonical_audio_sha256,
        original_sha
    );
    let request = CaptureAppendRequest {
        scope: scope.clone(),
        session_id: note.meta.id.clone(),
        capture_id: id(),
        request_id: id(),
        admission_id: id(),
        processing: None,
    };
    let prepared = CaptureAppendPreparation::prepare(root.join("append"), request.clone())
        .await
        .unwrap();
    let admission = client
        .prepare_capture_append(&prepared, cancel())
        .await
        .unwrap();
    assert_eq!(admission.baseline.recording_sha256, original_sha);
    assert_eq!(admission.baseline.frames, 16001);
    assert_eq!(admission.baseline.format, initial.format);
    let append = CaptureIngestRequest {
        identity: CaptureIdentity {
            scope: scope.clone(),
            capture_id: request.capture_id.clone(),
            request_id: request.request_id.clone(),
            target: CaptureTarget::AppendSession {
                session_id: note.meta.id.clone(),
                admission: admission.clone(),
            },
        },
        format: admission.baseline.format.clone(),
        first_admitted_audio_observed_at_ms: 1_794_096_101_123,
    };
    let suffix = [pcm(16001, 333), pcm(16334, 97)];
    let (seal, published) = publish(&client, &append, &suffix).await;
    let expected = [base, suffix.concat()].concat();
    let canonical_sha = recording(&client, &scope, &note.meta.id, &expected).await;
    assert_eq!(
        published
            .publication
            .as_ref()
            .unwrap()
            .canonical_audio_sha256,
        canonical_sha
    );
    while receiver.try_recv().is_ok() {}
    write(
        &root,
        "restart-request.json",
        json!({"capture_id":request.capture_id}),
    );
    marker(&root, "service-stopped.json").await;
    event(&mut receiver, |value| {
        matches!(
            value,
            RemoteEvent::Connection {
                state: ConnectionState::Reconnecting,
                ..
            }
        )
    })
    .await;
    write(
        &root,
        "restart-continue.json",
        json!({"observed":"SSE reconnecting"}),
    );
    marker(&root, "service-restarted.json").await;
    event(&mut receiver, |value| {
        matches!(
            value,
            RemoteEvent::Connection {
                state: ConnectionState::Connected,
                ..
            }
        )
    })
    .await;
    let reopened = RemoteClient::discover_fixture(origin, Arc::new(Credentials), CA)
        .await
        .unwrap();
    // A new local preparation location forces a real HTTP replay of the original
    // immutable request, rather than merely reading the SDK's cached admission.
    let replay = CaptureAppendPreparation::prepare(root.join("reopened-append"), request.clone())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .prepare_capture_append(&replay, cancel())
            .await
            .unwrap(),
        admission
    );
    assert_eq!(
        reopened.begin_capture(&append, cancel()).await.unwrap(),
        published
    );
    assert_eq!(
        reopened
            .publish_capture(&scope.vault_id, &request.capture_id, &seal, cancel())
            .await
            .unwrap(),
        published
    );
    assert_eq!(
        recording(&reopened, &scope, &note.meta.id, &expected).await,
        canonical_sha
    );
    let RemoteReply::Session(retained) = reopened
        .execute(
            RemoteOperation::Session {
                vault_id: scope.vault_id.clone(),
                session_id: note.meta.id.clone(),
            },
            &id(),
            cancel(),
        )
        .await
        .unwrap()
    else {
        panic!("session reply")
    };
    assert_eq!(retained.note, note.note);
    assert_eq!(retained.meta.title, note.meta.title);
    let after = create(&reopened, &scope.vault_id, "SSE after packaged restart").await;
    changed(&mut receiver, &scope.vault_id, &after.meta.id).await;
    subscription.close().await;
    stop.cancel();
    issuer_task.await.unwrap();
    write(
        &root,
        "driver-result.json",
        json!({"passed":true,"scope":scope,
        "request":request,"admission":admission,"publication":published.publication,
        "original_sha256":original_sha,"canonical_sha256":canonical_sha,
        "pcm_sha256":hash(&expected),"frames":16431,"channels":2,"sample_rate":16000,
        "sse_before_and_after_restart":true,"original_http_replay":true,
        "inference_configured":false,"physical_capture":false,"production_sso":false}),
    );
}

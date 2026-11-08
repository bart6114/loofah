use super::*;
use hypr_loofah_client::{RemoteClient, RemoteOperation, RemoteReply};
use sha2::{Digest, Sha256};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

fn input(fixture: &Fixture, me: &Me) -> CaptureIngestRequest {
    CaptureIngestRequest {
        identity: CaptureIdentity {
            scope: CaptureScope {
                server: format!(
                    "{}/",
                    fixture
                        .service
                        .config
                        .public_url
                        .origin()
                        .ascii_serialization()
                ),
                member_id: me.member.id.clone(),
                vault_id: me.personal_vault_id.clone(),
            },
            capture_id: uuid::Uuid::new_v4().to_string(),
            request_id: uuid::Uuid::new_v4().to_string(),
            target: CaptureTarget::PendingSession {
                create_request_id: uuid::Uuid::new_v4().to_string(),
            },
        },
        format: CaptureFormat::stereo(),
        first_admitted_audio_observed_at_ms: 1_794_096_001_123,
    }
}
fn pcm(frames: u32) -> Vec<u8> {
    (0..frames)
        .flat_map(|_| [0.25f32.to_le_bytes(), (-0.5f32).to_le_bytes()].concat())
        .collect()
}
fn descriptor(
    input: &CaptureIngestRequest,
    sequence: u64,
    frames: u32,
    previous: Option<&CapturePrefixReceipt>,
) -> CaptureChunkDescriptor {
    let fingerprint = capture_fingerprint(&input.identity, &input.format);
    let mut descriptor = CaptureChunkDescriptor {
        capture_fingerprint: fingerprint.clone(),
        sequence,
        first_frame: previous.map_or(0, |prefix| prefix.frames),
        pcm_offset: previous.map_or(0, |prefix| prefix.pcm_bytes),
        frames,
        bytes: pcm(frames).len() as u32,
        sha256: hash(pcm(frames)),
        previous_digest: previous.map_or_else(
            || capture_chain_seed(&fingerprint),
            |prefix| prefix.chain_digest.clone(),
        ),
        digest: String::new(),
    };
    descriptor.digest = capture_chunk_digest(&descriptor);
    descriptor
}
async fn begin(
    fixture: &Fixture,
    subject: &str,
    input: &CaptureIngestRequest,
) -> (StatusCode, Value) {
    fixture
        .call_token(
            &fixture.token(subject),
            "POST",
            &format!("/api/v1/vaults/{}/captures", input.identity.scope.vault_id),
            Some(serde_json::to_value(input).unwrap()),
            Some(&input.identity.request_id),
        )
        .await
}
async fn append(
    fixture: &Fixture,
    subject: &str,
    input: &CaptureIngestRequest,
    descriptor: &CaptureChunkDescriptor,
    bytes: Vec<u8>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/v1/vaults/{}/captures/{}/chunks/{}",
            input.identity.scope.vault_id, input.identity.capture_id, descriptor.sequence
        ))
        .header(
            "authorization",
            format!("Bearer {}", fixture.token(subject)),
        )
        .header("x-loofah-api-major", "1")
        .header("x-request-id", uuid::Uuid::new_v4().to_string())
        .header("content-type", "application/octet-stream")
        .header(
            "x-capture-chunk",
            serde_json::to_string(descriptor).unwrap(),
        )
        .body(Body::from(bytes))
        .unwrap();
    let response = fixture.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    (
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 131_072).await.unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn capture_scope_is_derived_from_current_membership_and_staging_never_creates_canonical_content()
 {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    fixture.login("bob").await;
    let input = input(&fixture, &alice);
    let (status, initial) = begin(&fixture, "alice", &input).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(initial["prefix"]["chunk_count"], 0);
    assert!(initial["session_id"].is_null());
    for field in ["server", "member_id", "vault_id"] {
        let mut changed = input.clone();
        match field {
            "server" => changed.identity.scope.server = "https://other.example/".into(),
            "member_id" => changed.identity.scope.member_id = "other".into(),
            _ => changed.identity.scope.vault_id = uuid::Uuid::new_v4().to_string(),
        }
        assert!(!begin(&fixture, "alice", &changed).await.0.is_success());
    }
    let path = format!(
        "/api/v1/vaults/{}/captures/{}",
        alice.personal_vault_id, input.identity.capture_id
    );
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .call(
                "bob",
                "POST",
                &format!("/api/v1/vaults/{}/captures", alice.personal_vault_id),
                Some(serde_json::to_value(&input).unwrap())
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut changed = input.clone();
    changed.first_admitted_audio_observed_at_ms += 1;
    let (status, error) = begin(&fixture, "alice", &changed).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["mutation_outcome"], "not_committed");
    let context = fixture
        .service
        .vault(&alice.personal_vault_id)
        .await
        .unwrap();
    assert!(context.store.session_list().is_empty());
    assert!(
        !context.store.vault_base().join("sessions").exists()
            || std::fs::read_dir(context.store.vault_base().join("sessions"))
                .unwrap()
                .next()
                .is_none()
    );
}

#[tokio::test]
async fn capture_replay_and_wrong_chunk_or_seal_preserve_the_exact_admitted_prefix() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let input = input(&fixture, &me);
    assert_eq!(begin(&fixture, "alice", &input).await.0, StatusCode::OK);
    let first = descriptor(&input, 0, 160, None);
    let mut wrong = first.clone();
    wrong.first_frame = 1;
    wrong.digest = capture_chunk_digest(&wrong);
    assert_eq!(
        append(&fixture, "alice", &input, &wrong, pcm(160)).await.0,
        StatusCode::CONFLICT
    );
    let (status, prefix) = append(&fixture, "alice", &input, &first, pcm(160)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(prefix["chunk_count"], 1);
    assert_eq!(
        append(&fixture, "alice", &input, &first, pcm(160)).await,
        (status, prefix.clone())
    );
    let mut corrupt = pcm(160);
    corrupt[0] ^= 1;
    assert_eq!(
        append(&fixture, "alice", &input, &first, corrupt).await.0,
        StatusCode::BAD_REQUEST
    );
    let prefix: CapturePrefixReceipt = serde_json::from_value(prefix).unwrap();
    let seal_path = format!(
        "/api/v1/vaults/{}/captures/{}/seal",
        me.personal_vault_id, input.identity.capture_id
    );
    let seal = CaptureSeal {
        capture_fingerprint: prefix.capture_fingerprint.clone(),
        chunk_count: prefix.chunk_count,
        frames: prefix.frames,
        pcm_bytes: prefix.pcm_bytes,
        chain_digest: prefix.chain_digest.clone(),
        source_sha256: "0".repeat(64),
    };
    assert_eq!(
        fixture
            .call(
                "alice",
                "POST",
                &seal_path,
                Some(serde_json::to_value(seal).unwrap())
            )
            .await
            .0,
        StatusCode::CONFLICT
    );
    let second = descriptor(&input, 1, 160, Some(&prefix));
    assert_eq!(
        append(&fixture, "alice", &input, &second, pcm(160)).await.0,
        StatusCode::OK
    );
    let seal = CaptureSeal {
        capture_fingerprint: prefix.capture_fingerprint,
        chunk_count: 2,
        frames: 320,
        pcm_bytes: 2560,
        chain_digest: second.digest,
        source_sha256: hash([pcm(160), pcm(160)].concat()),
    };
    let (status, sealed) = fixture
        .call(
            "alice",
            "POST",
            &seal_path,
            Some(serde_json::to_value(&seal).unwrap()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sealed["seal"], serde_json::to_value(&seal).unwrap());
    assert_eq!(begin(&fixture, "alice", &input).await.1, sealed);
    let path = format!(
        "/api/v1/vaults/{}/captures/{}",
        me.personal_vault_id, input.identity.capture_id
    );
    assert_eq!(fixture.call("alice", "GET", &path, None).await.1, sealed);
}

#[tokio::test]
async fn native_capture_upload_recovers_lost_replies_at_realistic_latency_without_rebinding_or_losing_audio()
 {
    for rtt in [50, 150, 500] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let fixture = Fixture::with_options(None, Some(origin.parse().unwrap())).await;
        let first = Arc::new(AtomicBool::new(true));
        let ready = CancellationToken::new();
        let release = CancellationToken::new();
        let app = fixture.app.clone().layer(axum::middleware::from_fn({
            let ready = ready.clone();
            let release = release.clone();
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let hold = request.method() == axum::http::Method::PUT
                    && request.uri().path().contains("/chunks/")
                    && first.swap(false, Ordering::SeqCst);
                let ready = ready.clone();
                let release = release.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(rtt / 2)).await;
                    let response = next.run(request).await;
                    if hold {
                        assert_eq!(response.status(), StatusCode::OK);
                        ready.cancel();
                        release.cancelled().await;
                    }
                    tokio::time::sleep(Duration::from_millis(rtt / 2)).await;
                    response
                }
            }
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = RemoteClient::discover(&origin, Arc::new(EphemeralCredentials), true)
            .await
            .unwrap();
        let login = client.begin_login().await.unwrap();
        let url = login.authorization_url().to_string();
        let complete = tokio::spawn(login.complete());
        assert!(reqwest::get(url).await.unwrap().status().is_success());
        complete.await.unwrap().unwrap();
        let RemoteReply::Me(me) = client
            .execute(
                RemoteOperation::Provision,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
        else {
            panic!("provision reply")
        };
        let input = input(&fixture, &me);
        client
            .begin_capture(&input, CancellationToken::new())
            .await
            .unwrap();
        let first = descriptor(&input, 0, 160, None);
        let cancel = CancellationToken::new();
        let pending = tokio::spawn({
            let client = client.clone();
            let input = input.clone();
            let first = first.clone();
            let cancel = cancel.clone();
            async move {
                client
                    .append_capture_chunk(
                        &input.identity.scope.vault_id,
                        &input.identity.capture_id,
                        &first,
                        pcm(160),
                        cancel,
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(15), ready.cancelled())
            .await
            .unwrap();
        cancel.cancel();
        assert_eq!(
            pending.await.unwrap().unwrap_err().mutation_outcome,
            Some(MutationOutcome::Unknown)
        );
        release.cancel();
        let status = client
            .capture_status(
                &me.personal_vault_id,
                &input.identity.capture_id,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(status.prefix.chunk_count, 1);
        assert_eq!(status.identity, input.identity);
        let receipt = client
            .append_capture_chunk(
                &me.personal_vault_id,
                &input.identity.capture_id,
                &first,
                pcm(160),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(receipt, status.prefix);
        let second = descriptor(&input, 1, 160, Some(&receipt));
        let receipt = client
            .append_capture_chunk(
                &me.personal_vault_id,
                &input.identity.capture_id,
                &second,
                pcm(160),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let seal = CaptureSeal {
            capture_fingerprint: receipt.capture_fingerprint,
            chunk_count: 2,
            frames: 320,
            pcm_bytes: 2560,
            chain_digest: receipt.chain_digest,
            source_sha256: hash([pcm(160), pcm(160)].concat()),
        };
        let sealed = client
            .seal_capture(
                &me.personal_vault_id,
                &input.identity.capture_id,
                &seal,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(sealed.seal.as_ref(), Some(&seal));
        assert!(sealed.publication.is_none());
        fixture.service.shutdown().await.unwrap();
        server.abort();
    }
}

#[tokio::test]
async fn shared_vault_capture_staging_remains_private_and_rechecks_demotion_and_removal() {
    let fixture = Fixture::new().await;
    fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Capture permissions"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    let member_path = format!("/api/v1/vaults/{vault}/members/{}", bob.member.id);
    let (status, mutation) = fixture
        .call(
            "alice",
            "PUT",
            &member_path,
            Some(json!({"role":"editor", "expected_revision":shared["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let mut input = input(&fixture, &bob);
    input.identity.scope.vault_id = vault.into();
    assert_eq!(begin(&fixture, "bob", &input).await.0, StatusCode::OK);
    let status_path = format!(
        "/api/v1/vaults/{vault}/captures/{}",
        input.identity.capture_id
    );
    assert_eq!(
        fixture.call("alice", "GET", &status_path, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, mutation) = fixture
        .call(
            "alice",
            "PUT",
            &member_path,
            Some(json!({"role":"reader", "expected_revision":mutation["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        fixture.call("bob", "GET", &status_path, None).await.0,
        StatusCode::OK
    );
    let descriptor = descriptor(&input, 0, 160, None);
    assert_eq!(
        append(&fixture, "bob", &input, &descriptor, pcm(160))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let remove = format!(
        "{member_path}?expected_revision={}",
        mutation["revision"].as_str().unwrap()
    );
    assert_eq!(
        fixture.call("alice", "DELETE", &remove, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        fixture.call("bob", "GET", &status_path, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert!(!begin(&fixture, "bob", &input).await.0.is_success());
}

fn hash(bytes: impl AsRef<[u8]>) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn native_publication_recovers_lost_replies_and_verifies_canonical_audio_at_realistic_latency()
 {
    for rtt in [50, 150, 500] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let fixture = Fixture::with_options(None, Some(origin.parse().unwrap())).await;
        let first = Arc::new(AtomicBool::new(true));
        let ready = CancellationToken::new();
        let release = CancellationToken::new();
        let app = fixture.app.clone().layer(axum::middleware::from_fn({
            let ready = ready.clone();
            let release = release.clone();
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let hold = request.method() == axum::http::Method::POST
                    && request.uri().path().ends_with("/publish")
                    && first.swap(false, Ordering::SeqCst);
                let ready = ready.clone();
                let release = release.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(rtt / 2)).await;
                    let response = next.run(request).await;
                    if hold {
                        assert_eq!(response.status(), StatusCode::OK);
                        ready.cancel();
                        release.cancelled().await;
                    }
                    tokio::time::sleep(Duration::from_millis(rtt / 2)).await;
                    response
                }
            }
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = RemoteClient::discover(&origin, Arc::new(EphemeralCredentials), true)
            .await
            .unwrap();
        assert!(
            client
                .capabilities()
                .features
                .iter()
                .any(|feature| feature == "capture_publication")
        );
        let login = client.begin_login().await.unwrap();
        let url = login.authorization_url().to_string();
        let complete = tokio::spawn(login.complete());
        assert!(reqwest::get(url).await.unwrap().status().is_success());
        complete.await.unwrap().unwrap();
        let RemoteReply::Me(me) = client
            .execute(
                RemoteOperation::Provision,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
        else {
            panic!("provision reply")
        };
        let RemoteReply::Session(note) = client
            .execute(
                RemoteOperation::CreateSession {
                    vault_id: me.personal_vault_id.clone(),
                    input: CreateSessionRequest {
                        title: "Capture destination".into(),
                        markdown: "Keep this note and its attachments.".into(),
                    },
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
        else {
            panic!("created note")
        };
        let mut input = input(&fixture, &me);
        input.identity.target = CaptureTarget::ExistingSession {
            session_id: note.meta.id.clone(),
            expected_audio_sha256: None,
        };
        client
            .begin_capture(&input, CancellationToken::new())
            .await
            .unwrap();
        let mut prefix = None;
        for sequence in 0..2 {
            let chunk = descriptor(&input, sequence, 160, prefix.as_ref());
            prefix = Some(
                client
                    .append_capture_chunk(
                        &me.personal_vault_id,
                        &input.identity.capture_id,
                        &chunk,
                        pcm(160),
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap(),
            );
        }
        let prefix = prefix.unwrap();
        let source = [pcm(160), pcm(160)].concat();
        let seal = CaptureSeal {
            capture_fingerprint: prefix.capture_fingerprint,
            chunk_count: prefix.chunk_count,
            frames: prefix.frames,
            pcm_bytes: prefix.pcm_bytes,
            chain_digest: prefix.chain_digest,
            source_sha256: hash(&source),
        };
        client
            .seal_capture(
                &me.personal_vault_id,
                &input.identity.capture_id,
                &seal,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let cancel = CancellationToken::new();
        let pending = tokio::spawn({
            let (client, vault, capture, seal, cancel) = (
                client.clone(),
                me.personal_vault_id.clone(),
                input.identity.capture_id.clone(),
                seal.clone(),
                cancel.clone(),
            );
            async move {
                client
                    .publish_capture(&vault, &capture, &seal, cancel)
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(15), ready.cancelled())
            .await
            .unwrap();
        cancel.cancel();
        assert_eq!(
            pending.await.unwrap().unwrap_err().mutation_outcome,
            Some(MutationOutcome::Unknown)
        );
        release.cancel();
        let status = client
            .capture_status(
                &me.personal_vault_id,
                &input.identity.capture_id,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(status.identity, input.identity);
        assert_eq!(
            status.first_admitted_audio_observed_at_ms,
            input.first_admitted_audio_observed_at_ms
        );
        let publication = status.publication.unwrap();
        assert_eq!(publication.source, seal);
        assert_eq!(publication.session_id, note.meta.id);
        for _ in 0..2 {
            let replay = client
                .publish_capture(
                    &me.personal_vault_id,
                    &input.identity.capture_id,
                    &seal,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            assert_eq!(replay.publication.as_ref(), Some(&publication));
            assert_eq!(replay.identity, input.identity);
        }
        // A separate authenticated artifact read verifies the receipt's canonical observation.
        let lease = client
            .open_recording(
                &me.personal_vault_id,
                &note.meta.id,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lease.metadata.filename, "audio.wav");
        assert_eq!(lease.metadata.sha256, publication.canonical_audio_sha256);
        assert_eq!(lease.metadata.size_bytes, publication.canonical_audio_bytes);
        let audio = client
            .read_recording_chunk(
                &lease,
                0,
                lease.metadata.size_bytes as usize,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(hash(&audio), publication.canonical_audio_sha256);
        assert_eq!(&audio[..4], b"RIFF");
        assert_eq!(&audio[audio.len() - source.len()..], source);
        client
            .close_recording(&me.personal_vault_id, &lease.id, CancellationToken::new())
            .await
            .unwrap();
        let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
        let current = context.read_note(&note.meta.id).await.unwrap();
        assert_eq!(current, note.note);
        let metadata = context.read_metadata(&note.meta.id).await.unwrap();
        assert_eq!(
            metadata.meta.started_at.as_deref(),
            Some("2026-11-08T00:00:01.123Z")
        );
        client.disconnect();
        fixture.service.shutdown().await.unwrap();
        server.abort();
    }
}

#[tokio::test]
async fn publication_refuses_pending_destinations_and_requires_current_vault_access() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    fixture.login("bob").await;
    let input = input(&fixture, &alice);
    assert_eq!(begin(&fixture, "alice", &input).await.0, StatusCode::OK);
    let chunk = descriptor(&input, 0, 160, None);
    let (status, receipt) = append(&fixture, "alice", &input, &chunk, pcm(160)).await;
    assert_eq!(status, StatusCode::OK);
    let prefix: CapturePrefixReceipt = serde_json::from_value(receipt).unwrap();
    let seal = CaptureSeal {
        capture_fingerprint: prefix.capture_fingerprint,
        chunk_count: prefix.chunk_count,
        frames: prefix.frames,
        pcm_bytes: prefix.pcm_bytes,
        chain_digest: prefix.chain_digest,
        source_sha256: hash(pcm(160)),
    };
    let base = format!(
        "/api/v1/vaults/{}/captures/{}",
        alice.personal_vault_id, input.identity.capture_id
    );
    assert_eq!(
        fixture
            .call(
                "alice",
                "POST",
                &format!("{base}/seal"),
                Some(serde_json::to_value(&seal).unwrap())
            )
            .await
            .0,
        StatusCode::OK
    );
    let path = format!("{base}/publish");
    let body = Some(serde_json::to_value(&seal).unwrap());
    let (status, cause) = fixture.call("alice", "POST", &path, body.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(cause["mutation_outcome"], "not_committed");
    assert_eq!(
        fixture.call("bob", "POST", &path, body).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, retained) = fixture.call("alice", "GET", &base, None).await;
    assert_eq!(status, StatusCode::OK);
    let retained: CaptureIngestStatus = serde_json::from_value(retained).unwrap();
    assert_eq!(retained.identity, input.identity);
    assert_eq!(retained.seal.as_ref(), Some(&seal));
    assert!(retained.publication.is_none());
    let context = fixture
        .service
        .vault(&alice.personal_vault_id)
        .await
        .unwrap();
    assert!(context.store.session_list().is_empty());
    fixture.service.shutdown().await.unwrap();
}

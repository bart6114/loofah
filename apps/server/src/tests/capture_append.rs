use super::*;
use std::{io::Cursor, path::PathBuf, sync::atomic::Ordering};

const BASE_FRAMES: u32 = 16_001;
const TRANSCRIPT: &[u8] = br#" {"future":true,"transcripts":[{"id":"t","session_id":"append-note","words":[{"id":"w","text":"Original","channel":0,"start_ms":0,"end_ms":1}],"speaker_hints":[]}]} "#;

async fn recording(fixture: &Fixture, vault: &str, channels: u16) -> PathBuf {
    let context = fixture.service.vault(vault).await.unwrap();
    let meta = serde_json::from_value(json!({
        "id":"append-note", "title":"Keep title", "created_at":"2026-11-08T10:00:00Z",
        "tags":[], "audio":{"source":"recording", "layout":if channels == 2 {"mic_system"} else {"mixed"}}
    })).unwrap();
    context
        .store
        .create_note_session(&meta, "Keep notes")
        .await
        .unwrap();
    let directory = context.store.vault_base().join("sessions/append-note");
    let mut writer = hound::WavWriter::create(
        directory.join("audio.wav"),
        hound::WavSpec {
            channels,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for _ in 0..BASE_FRAMES {
        writer.write_sample(0.125_f32).unwrap();
        if channels == 2 {
            writer.write_sample(-0.25_f32).unwrap();
        }
    }
    writer.finalize().unwrap();
    std::fs::write(directory.join("transcript.json"), TRANSCRIPT).unwrap();
    std::fs::write(directory.join("unknown.bin"), b"keep attachment").unwrap();
    std::fs::write(directory.join(".private"), b"keep hidden file").unwrap();
    directory
}

fn request(fixture: &Fixture, member: &Me, vault: &str) -> CaptureAppendRequest {
    let mut scope = capture::input(fixture, member).identity.scope;
    scope.vault_id = vault.into();
    CaptureAppendRequest {
        scope,
        session_id: "append-note".into(),
        capture_id: uuid::Uuid::new_v4().to_string(),
        request_id: uuid::Uuid::new_v4().to_string(),
        admission_id: uuid::Uuid::new_v4().to_string(),
        processing: None,
    }
}

async fn prepare(
    fixture: &Fixture,
    subject: &str,
    input: &CaptureAppendRequest,
) -> (StatusCode, Value) {
    fixture
        .call_token(
            &fixture.token(subject),
            "POST",
            &format!(
                "/api/v1/vaults/{}/capture-append-admissions",
                input.scope.vault_id
            ),
            Some(serde_json::to_value(input).unwrap()),
            Some(&input.request_id),
        )
        .await
}

fn ingest(admission: CaptureAppendAdmission) -> CaptureIngestRequest {
    CaptureIngestRequest {
        identity: CaptureIdentity {
            scope: admission.request.scope.clone(),
            capture_id: admission.request.capture_id.clone(),
            request_id: admission.request.request_id.clone(),
            target: CaptureTarget::AppendSession {
                session_id: admission.request.session_id.clone(),
                admission: admission.clone(),
            },
        },
        format: admission.baseline.format,
        first_admitted_audio_observed_at_ms: 1_794_096_001_123,
    }
}

async fn canonical(fixture: &Fixture, input: &CaptureAppendRequest) -> Vec<u8> {
    let vault = &input.scope.vault_id;
    let lease = uuid::Uuid::new_v4();
    let open = format!(
        "/api/v1/vaults/{vault}/sessions/{}/audio/leases/{lease}",
        input.session_id
    );
    let path = format!("/api/v1/vaults/{vault}/audio/leases/{lease}");
    let (status, metadata) = fixture.call("alice", "PUT", &open, None).await;
    assert_eq!(status, StatusCode::OK, "{metadata}");
    let response = recording::request(fixture, "alice", "GET", &path, &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    assert_eq!(metadata["metadata"]["sha256"], capture::hash(&bytes));
    assert_eq!(metadata["metadata"]["sizeBytes"], bytes.len());
    assert!(
        fixture
            .call("alice", "DELETE", &path, None)
            .await
            .0
            .is_success()
    );
    bytes
}

async fn reopen(mut fixture: Fixture) -> Fixture {
    fixture.service.shutdown().await.unwrap();
    fixture.service.wait_for_shutdown().await.unwrap();
    let config = fixture.service.config.clone();
    let root = std::mem::replace(&mut fixture._root, tempfile::tempdir().unwrap());
    let issuer = std::mem::take(&mut fixture.issuer);
    let issuer_task = std::mem::replace(&mut fixture.issuer_task, tokio::spawn(async {}));
    drop(fixture);
    let service = Service::open(config).await.unwrap();
    service.injected_changes.store(true, Ordering::Release);
    Fixture {
        _root: root,
        issuer,
        issuer_task,
        app: routes::router(service.clone()),
        service,
    }
}

#[tokio::test]
async fn raw_append_http_publishes_exact_channels_and_replays_after_service_reopen() {
    for channels in [1, 2] {
        let fixture = Fixture::new().await;
        let alice = fixture.login("alice").await;
        let directory = recording(&fixture, &alice.personal_vault_id, channels).await;
        let input = request(&fixture, &alice, &alice.personal_vault_id);
        let (status, capabilities) = fixture
            .call("alice", "GET", "/api/v1/capabilities", None)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !capabilities["features"]
                .as_array()
                .unwrap()
                .contains(&json!("capture_append"))
        );
        let (status, prepared) = prepare(&fixture, "alice", &input).await;
        assert_eq!(status, StatusCode::OK, "{prepared}");
        let admission: CaptureAppendAdmission = serde_json::from_value(prepared.clone()).unwrap();
        assert_eq!(admission.request, input);
        assert_eq!(
            admission.baseline.recording_sha256,
            capture::hash(std::fs::read(directory.join("audio.wav")).unwrap())
        );
        assert_eq!(admission.baseline.frames, u64::from(BASE_FRAMES));
        assert_eq!(
            admission.baseline.format.channels,
            if channels == 2 {
                vec![CaptureChannel::Microphone, CaptureChannel::SystemAudio]
            } else {
                vec![CaptureChannel::Mixed]
            }
        );
        assert_eq!(
            prepare(&fixture, "alice", &input).await,
            (StatusCode::OK, prepared.clone())
        );
        let ingest = ingest(admission);
        let (status, begun) = capture::begin(&fixture, "alice", &ingest).await;
        assert_eq!(status, StatusCode::OK, "{begun}");
        let begun: CaptureIngestStatus = serde_json::from_value(begun).unwrap();
        let mut prefix = begun.prefix;
        let mut suffix = Vec::new();
        for frames in [333, 97] {
            let bytes: Vec<_> = (0..frames)
                .flat_map(|_| {
                    if channels == 2 {
                        [0.5_f32.to_le_bytes(), 0.75_f32.to_le_bytes()].concat()
                    } else {
                        0.5_f32.to_le_bytes().to_vec()
                    }
                })
                .collect();
            let mut descriptor = CaptureChunkDescriptor {
                capture_fingerprint: prefix.capture_fingerprint.clone(),
                sequence: prefix.chunk_count,
                first_frame: prefix.frames,
                pcm_offset: prefix.pcm_bytes,
                frames,
                bytes: bytes.len() as u32,
                sha256: capture::hash(&bytes),
                previous_digest: prefix.chain_digest.clone(),
                digest: String::new(),
            };
            descriptor.digest = capture_chunk_digest(&descriptor);
            let response =
                capture::append(&fixture, "alice", &ingest, &descriptor, bytes.clone()).await;
            assert_eq!(response.0, StatusCode::OK, "{}", response.1);
            // A repeated HTTP chunk is an acknowledgement replay, not another suffix.
            assert_eq!(
                capture::append(&fixture, "alice", &ingest, &descriptor, bytes.clone()).await,
                response
            );
            prefix = serde_json::from_value(response.1).unwrap();
            suffix.extend(bytes);
        }
        let seal = CaptureSeal {
            capture_fingerprint: prefix.capture_fingerprint,
            chunk_count: prefix.chunk_count,
            frames: prefix.frames,
            pcm_bytes: prefix.pcm_bytes,
            chain_digest: prefix.chain_digest,
            source_sha256: capture::hash(&suffix),
        };
        let base = format!(
            "/api/v1/vaults/{}/captures/{}",
            input.scope.vault_id, input.capture_id
        );
        for operation in ["seal", "publish"] {
            let (status, value) = fixture
                .call(
                    "alice",
                    "POST",
                    &format!("{base}/{operation}"),
                    Some(serde_json::to_value(&seal).unwrap()),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{value}");
        }
        let (status, published) = fixture.call("alice", "GET", &base, None).await;
        assert_eq!(status, StatusCode::OK);
        let published: CaptureIngestStatus = serde_json::from_value(published).unwrap();
        let publication = published.publication.as_ref().unwrap();
        assert_eq!(published.identity, ingest.identity);
        assert_eq!(publication.source, seal);
        assert_eq!(publication.session_id, input.session_id);
        let bytes = canonical(&fixture, &input).await;
        assert_eq!(publication.canonical_audio_sha256, capture::hash(&bytes));
        assert_eq!(publication.canonical_audio_bytes, bytes.len() as u64);
        let reader = hound::WavReader::new(Cursor::new(&bytes)).unwrap();
        assert_eq!(reader.spec().channels, channels);
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.duration(), BASE_FRAMES + 430);
        let expected: Vec<_> = (0..BASE_FRAMES)
            .flat_map(|_| {
                if channels == 2 {
                    vec![0.125_f32, -0.25_f32]
                } else {
                    vec![0.125_f32]
                }
            })
            .chain(
                suffix
                    .chunks_exact(4)
                    .map(|sample| f32::from_le_bytes(sample.try_into().unwrap())),
            )
            .map(f32::to_bits)
            .collect();
        assert_eq!(
            reader
                .into_samples::<f32>()
                .map(|sample| sample.unwrap().to_bits())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            std::fs::read(directory.join("transcript.json")).unwrap(),
            TRANSCRIPT
        );
        assert_eq!(
            std::fs::read(directory.join("notes.md")).unwrap(),
            b"Keep notes"
        );
        assert_eq!(
            std::fs::read(directory.join("unknown.bin")).unwrap(),
            b"keep attachment"
        );
        assert_eq!(
            std::fs::read(directory.join(".private")).unwrap(),
            b"keep hidden file"
        );
        let fixture = reopen(fixture).await;
        assert_eq!(
            prepare(&fixture, "alice", &input).await,
            (StatusCode::OK, prepared)
        );
        let (status, replay) = capture::begin(&fixture, "alice", &ingest).await;
        assert_eq!(status, StatusCode::OK, "{replay}");
        assert_eq!(
            serde_json::from_value::<CaptureIngestStatus>(replay).unwrap(),
            published
        );
        let (status, replay) = fixture
            .call(
                "alice",
                "POST",
                &format!("{base}/publish"),
                Some(serde_json::to_value(&seal).unwrap()),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{replay}");
        assert_eq!(
            serde_json::from_value::<CaptureIngestStatus>(replay).unwrap(),
            published
        );
        assert_eq!(canonical(&fixture, &input).await, bytes);
        assert_eq!(
            std::fs::read(directory.join("transcript.json")).unwrap(),
            TRANSCRIPT
        );
        fixture.service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn append_http_stale_transcript_revision_rejects_begin_without_refreshing_admission() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let directory = recording(&fixture, &alice.personal_vault_id, 2).await;
    let input = request(&fixture, &alice, &alice.personal_vault_id);
    let original_audio = std::fs::read(directory.join("audio.wav")).unwrap();
    let (status, prepared) = prepare(&fixture, "alice", &input).await;
    assert_eq!(status, StatusCode::OK, "{prepared}");
    let base = format!(
        "/api/v1/vaults/{}/sessions/{}",
        input.scope.vault_id, input.session_id
    );
    let (status, original) = fixture
        .call("alice", "GET", &format!("{base}/transcript/editor"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{original}");
    let (status, changed) = fixture.call("alice", "POST", &format!("{base}/transcript/speakers"), Some(json!({
        "expected_revision": original["revision"],
        "assignments":[{"transcript_id":"t","channel":0,"speaker_index":null,"speaker_label":"Alice","anchor_word_id":"w"}]
    }))).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["kind"], "saved");
    let edited_transcript = std::fs::read(directory.join("transcript.json")).unwrap();
    assert_ne!(edited_transcript, TRANSCRIPT);
    assert_eq!(
        prepare(&fixture, "alice", &input).await,
        (StatusCode::OK, prepared.clone())
    );
    let ingest = ingest(serde_json::from_value(prepared).unwrap());
    let (status, rejected) = capture::begin(&fixture, "alice", &ingest).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(rejected["code"], "conflict");
    assert_eq!(rejected["mutation_outcome"], "not_committed");
    assert_eq!(
        std::fs::read(directory.join("audio.wav")).unwrap(),
        original_audio
    );
    assert_eq!(
        std::fs::read(directory.join("transcript.json")).unwrap(),
        edited_transcript
    );
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn append_http_admission_rechecks_reader_and_exact_authenticated_request_scope() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (status, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Append permissions"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let vault = shared["id"].as_str().unwrap();
    let directory = recording(&fixture, vault, 2).await;
    let audio = std::fs::read(directory.join("audio.wav")).unwrap();
    let member_path = format!("/api/v1/vaults/{vault}/members/{}", bob.member.id);
    let (status, membership) = fixture
        .call(
            "alice",
            "PUT",
            &member_path,
            Some(json!({"role":"editor", "expected_revision":shared["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let input = request(&fixture, &bob, vault);
    let (status, prepared) = prepare(&fixture, "bob", &input).await;
    assert_eq!(status, StatusCode::OK, "{prepared}");
    let (status, demoted) = fixture
        .call(
            "alice",
            "PUT",
            &member_path,
            Some(json!({"role":"reader", "expected_revision":membership["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{demoted}");
    let (status, denied) = prepare(&fixture, "bob", &input).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    assert_eq!(denied["code"], "forbidden");
    assert_eq!(
        capture::begin(
            &fixture,
            "bob",
            &ingest(serde_json::from_value(prepared).unwrap())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let owner = request(&fixture, &alice, vault);
    let path = format!("/api/v1/vaults/{vault}/capture-append-admissions");
    let (status, denied) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &path,
            Some(serde_json::to_value(&owner).unwrap()),
            Some(&uuid::Uuid::new_v4().to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{denied}");
    assert_eq!(denied["mutation_outcome"], "not_committed");
    for field in ["server", "member", "vault"] {
        let mut wrong = owner.clone();
        match field {
            "server" => wrong.scope.server = "https://other.example/".into(),
            "member" => wrong.scope.member_id = bob.member.id.clone(),
            _ => wrong.scope.vault_id = alice.personal_vault_id.clone(),
        }
        let (status, denied) = fixture
            .call_token(
                &fixture.token("alice"),
                "POST",
                &path,
                Some(serde_json::to_value(&wrong).unwrap()),
                Some(&wrong.request_id),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {denied}");
        assert_eq!(denied["code"], "validation");
        assert_eq!(denied["mutation_outcome"], "not_committed");
    }
    let mut private = owner;
    private.scope.vault_id = bob.personal_vault_id;
    assert_eq!(
        prepare(&fixture, "alice", &private).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(std::fs::read(directory.join("audio.wav")).unwrap(), audio);
    assert_eq!(
        std::fs::read(directory.join("transcript.json")).unwrap(),
        TRANSCRIPT
    );
    fixture.service.shutdown().await.unwrap();
}

use super::*;
use crate::VaultRuntime;
use hypr_loofah_protocol::*;

async fn fixture() -> (tempfile::TempDir, VaultRuntime, CaptureAppendRequest) {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let vault = root_path.join("vault");
    std::fs::create_dir(&vault).unwrap();
    let runtime = VaultRuntime::open(
        "vault".into(),
        &vault,
        &root_path.join("cache"),
        &root_path.join("operations"),
    )
    .await
    .unwrap();
    runtime.context.startup.wait_until_ready().await.unwrap();
    runtime
        .context
        .enable_capture_ingestion("https://capture.example/".into(), Default::default())
        .unwrap();
    let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Keep","created_at":"2026-11-08T10:00:00Z","tags":[],"audio":{"source":"recording","layout":"mic_system"},"future":true})).unwrap();
    runtime
        .context
        .store
        .create_note_session(&meta, "Original notes")
        .await
        .unwrap();
    let mut audio = hound::WavWriter::create(
        vault.join("sessions/note/audio.wav"),
        hypr_audio_utils::pcm_spec(2),
    )
    .unwrap();
    for _ in 0..16001 {
        audio.write_sample(0.25_f32).unwrap();
        audio.write_sample(-0.125_f32).unwrap();
    }
    audio.finalize().unwrap();
    std::fs::write(
        vault.join("sessions/note/transcript.json"),
        br#" {"transcripts":[],"future":{"keep":true}} "#,
    )
    .unwrap();
    let input = CaptureAppendRequest {
        scope: CaptureScope {
            server: "https://capture.example/".into(),
            member_id: "member".into(),
            vault_id: "vault".into(),
        },
        session_id: "note".into(),
        capture_id: uuid::Uuid::new_v4().to_string(),
        request_id: uuid::Uuid::new_v4().to_string(),
        admission_id: uuid::Uuid::new_v4().to_string(),
        processing: None,
    };
    (root, runtime, input)
}

#[tokio::test]
async fn append_preparation_pins_exact_frames_and_cannot_be_retargeted_or_forged() {
    let (_root, runtime, input) = fixture().await;
    let context = runtime.context.clone();
    let prepared = context
        .prepare_capture_append_authorized("member".into(), input.clone(), || async { Ok(()) })
        .await
        .unwrap();
    assert_eq!(prepared.baseline.frames, 16001);
    assert_eq!(prepared.baseline.format, CaptureFormat::stereo());
    assert!(prepared.baseline.transcript_present);
    assert_eq!(
        context
            .prepare_capture_append_authorized("member".into(), input.clone(), || async { Ok(()) })
            .await
            .unwrap(),
        prepared
    );
    let mut forged = prepared.clone();
    forged.baseline.frames -= 1;
    let identity = CaptureIdentity {
        scope: input.scope.clone(),
        capture_id: input.capture_id.clone(),
        request_id: input.request_id.clone(),
        target: CaptureTarget::AppendSession {
            session_id: "note".into(),
            admission: forged,
        },
    };
    assert!(
        context
            .begin_capture_authorized(
                "member".into(),
                CaptureIngestRequest {
                    identity,
                    format: CaptureFormat::stereo(),
                    first_admitted_audio_observed_at_ms: 1
                },
                || async { Ok(()) }
            )
            .await
            .is_err()
    );
    let mut other = input;
    other.session_id = "other".into();
    assert!(
        context
            .prepare_capture_append_authorized("member".into(), other, || async { Ok(()) })
            .await
            .is_err()
    );
    drop(context);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn changed_raw_transcript_after_admission_blocks_capture_without_rebinding() {
    let (root, runtime, input) = fixture().await;
    let context = runtime.context.clone();
    let prepared = context
        .prepare_capture_append_authorized("member".into(), input.clone(), || async { Ok(()) })
        .await
        .unwrap();
    std::fs::write(
        root.path().join("vault/sessions/note/transcript.json"),
        b"{\"transcripts\":[],\"user_edit\":true}",
    )
    .unwrap();
    assert_eq!(
        context
            .prepare_capture_append_authorized("member".into(), input.clone(), || async { Ok(()) })
            .await
            .unwrap(),
        prepared
    );
    let identity = CaptureIdentity {
        scope: input.scope,
        capture_id: input.capture_id,
        request_id: input.request_id,
        target: CaptureTarget::AppendSession {
            session_id: "note".into(),
            admission: prepared,
        },
    };
    assert!(
        context
            .begin_capture_authorized(
                "member".into(),
                CaptureIngestRequest {
                    identity,
                    format: CaptureFormat::stereo(),
                    first_admitted_audio_observed_at_ms: 1
                },
                || async { Ok(()) }
            )
            .await
            .is_err()
    );
    drop(context);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn append_publishes_exact_suffix_and_historical_retry_never_recreates_deleted_canonical() {
    let (root, runtime, input) = fixture().await;
    let context = runtime.context.clone();
    let directory = root.path().join("vault/sessions/note");
    let original_transcript = std::fs::read(directory.join("transcript.json")).unwrap();
    let prepared = context
        .prepare_capture_append_authorized("member".into(), input.clone(), || async { Ok(()) })
        .await
        .unwrap();
    let identity = CaptureIdentity {
        scope: input.scope,
        capture_id: input.capture_id.clone(),
        request_id: input.request_id,
        target: CaptureTarget::AppendSession {
            session_id: "note".into(),
            admission: prepared,
        },
    };
    let status = context
        .begin_capture_authorized(
            "member".into(),
            CaptureIngestRequest {
                identity,
                format: CaptureFormat::stereo(),
                first_admitted_audio_observed_at_ms: 1,
            },
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let bytes = [0.5_f32.to_le_bytes(), 0.75_f32.to_le_bytes()].concat();
    let mut descriptor = CaptureChunkDescriptor {
        capture_fingerprint: status.prefix.capture_fingerprint.clone(),
        sequence: 0,
        first_frame: 0,
        pcm_offset: 0,
        frames: 1,
        bytes: 8,
        sha256: super::super::hash(&bytes),
        previous_digest: status.prefix.chain_digest,
        digest: String::new(),
    };
    descriptor.digest = capture_chunk_digest(&descriptor);
    let prefix = context
        .append_capture_authorized(
            "member".into(),
            input.capture_id.clone(),
            descriptor,
            bytes.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let seal = CaptureSeal {
        capture_fingerprint: prefix.capture_fingerprint,
        chunk_count: prefix.chunk_count,
        frames: prefix.frames,
        pcm_bytes: prefix.pcm_bytes,
        chain_digest: prefix.chain_digest,
        source_sha256: super::super::hash(&bytes),
    };
    context
        .seal_capture_authorized(
            "member".into(),
            input.capture_id.clone(),
            seal.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let published = context
        .publish_capture_authorized(
            "member".into(),
            input.capture_id.clone(),
            seal.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let recording = hound::WavReader::open(directory.join("audio.wav")).unwrap();
    assert_eq!(recording.duration(), 16002);
    let samples: Vec<_> = recording
        .into_samples::<f32>()
        .map(Result::unwrap)
        .collect();
    assert_eq!(&samples[samples.len() - 2..], &[0.5, 0.75]);
    assert_eq!(
        std::fs::read(directory.join("transcript.json")).unwrap(),
        original_transcript
    );
    std::fs::remove_file(directory.join("audio.wav")).unwrap();
    std::fs::write(directory.join("transcript.json"), b"user revision").unwrap();
    let replay = context
        .publish_capture_authorized("member".into(), input.capture_id, seal, || async { Ok(()) })
        .await
        .unwrap();
    assert_eq!(replay.publication, published.publication);
    assert!(!directory.join("audio.wav").exists());
    assert_eq!(
        std::fs::read(directory.join("transcript.json")).unwrap(),
        b"user revision"
    );
    drop(context);
    runtime.shutdown().await.unwrap();
}

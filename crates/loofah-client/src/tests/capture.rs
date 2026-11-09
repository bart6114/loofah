use super::*;

#[tokio::test]
async fn capture_operations_require_capability_before_loading_credentials_or_sending_requests() {
    let fixture = Fixture::new().await;
    let client = fixture.client().await;
    let input = CaptureIngestRequest {
        identity: CaptureIdentity {
            scope: CaptureScope {
                server: client.address().into(),
                member_id: "member".into(),
                vault_id: "vault".into(),
            },
            capture_id: uuid::Uuid::new_v4().to_string(),
            request_id: uuid::Uuid::new_v4().to_string(),
            target: CaptureTarget::PendingSession {
                create_request_id: uuid::Uuid::new_v4().to_string(),
            },
        },
        format: CaptureFormat::stereo(),
        first_admitted_audio_observed_at_ms: 1,
    };
    let descriptor = CaptureChunkDescriptor {
        capture_fingerprint: "0".repeat(64),
        sequence: 0,
        first_frame: 0,
        pcm_offset: 0,
        frames: 1,
        bytes: 8,
        sha256: "0".repeat(64),
        previous_digest: "0".repeat(64),
        digest: "0".repeat(64),
    };
    let seal = CaptureSeal {
        capture_fingerprint: "0".repeat(64),
        chunk_count: 0,
        frames: 0,
        pcm_bytes: 0,
        chain_digest: "0".repeat(64),
        source_sha256: "0".repeat(64),
    };
    let results = [
        client
            .begin_capture(&input, CancellationToken::new())
            .await
            .unwrap_err(),
        client
            .capture_status(
                "vault",
                &input.identity.capture_id,
                CancellationToken::new(),
            )
            .await
            .unwrap_err(),
        client
            .append_capture_chunk(
                "vault",
                &input.identity.capture_id,
                &descriptor,
                vec![0; 8],
                CancellationToken::new(),
            )
            .await
            .unwrap_err(),
        client
            .seal_capture(
                "vault",
                &input.identity.capture_id,
                &seal,
                CancellationToken::new(),
            )
            .await
            .unwrap_err(),
        client
            .publish_capture(
                "vault",
                &input.identity.capture_id,
                &seal,
                CancellationToken::new(),
            )
            .await
            .unwrap_err(),
    ];
    for cause in results {
        assert_eq!(cause.code, ErrorCode::IncompatibleVersion);
        assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
    }
    assert_eq!(fixture.state.issued.load(Ordering::SeqCst), 0);
    assert!(fixture.credentials.values.lock().await.is_empty());
}

#[tokio::test]
async fn capture_ingestion_alone_cannot_publish_or_load_credentials() {
    let fixture = Fixture::new().await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client).unwrap().capabilities.features = vec!["capture_ingestion".into()];
    let cause = client
        .publish_capture(
            "vault",
            &uuid::Uuid::new_v4().to_string(),
            &CaptureSeal {
                capture_fingerprint: "0".repeat(64),
                chunk_count: 1,
                frames: 1,
                pcm_bytes: 4,
                chain_digest: "0".repeat(64),
                source_sha256: "0".repeat(64),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(cause.code, ErrorCode::IncompatibleVersion);
    assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert!(!client.tokens.lock().await.loaded);
    assert!(fixture.credentials.values.lock().await.is_empty());
    assert_eq!(fixture.state.issued.load(Ordering::SeqCst), 0);
}

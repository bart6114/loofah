use super::*;
use serde_json::json;

fn identity() -> CaptureIdentity {
    serde_json::from_value(json!({
        "scope": {"server":"https://capture.example/", "member_id":"member", "vault_id":"vault"},
        "capture_id":"00000000-0000-4000-8000-000000000001",
        "request_id":"00000000-0000-4000-8000-000000000002",
        "target": {"kind":"pending_session", "create_request_id":"00000000-0000-4000-8000-000000000003"}
    })).unwrap()
}

#[test]
fn version_one_capture_and_chunk_hashes_keep_the_existing_spool_wire_format() {
    let identity = identity();
    let format = CaptureFormat::stereo();
    let fingerprint = capture_fingerprint(&identity, &format);
    assert_eq!(
        fingerprint,
        "b14a28f7b2e448ee0e5f819ead0d8c40cd75f5abdf349efe0456a639a2ee3b46"
    );
    let seed = capture_chain_seed(&fingerprint);
    assert_eq!(
        seed,
        "c61a9a8002f8185c33bf23539c9de1cdf86a09d153dde708d5d156619a082d1a"
    );
    let mut descriptor = CaptureChunkDescriptor {
        capture_fingerprint: fingerprint,
        sequence: 0,
        first_frame: 0,
        pcm_offset: 0,
        frames: 2,
        bytes: 16,
        sha256: "374708fff7719dd5979ec875d56cd2286f6d3cf7ec317a3b25632aab28ec37bb".into(),
        previous_digest: seed,
        digest: String::new(),
    };
    let digest = capture_chunk_digest(&descriptor);
    assert_eq!(
        digest,
        "9b0b4f0a8c797f37337fa8ce926d02b5b4425ac9bbf6369b97dc3f416fd8441f"
    );
    descriptor.digest = digest.clone();
    assert_eq!(capture_chunk_digest(&descriptor), digest);
    assert_eq!(
        serde_json::to_value(format).unwrap(),
        json!({
            "encoding":"pcm_f32_le", "sample_rate":16000,
            "channels":["microphone", "system_audio"]
        })
    );
}

#[test]
fn capture_identity_commits_scope_target_and_explicit_channel_order() {
    let identity = identity();
    let format = CaptureFormat::stereo();
    let original = capture_fingerprint(&identity, &format);
    let mut changed = identity.clone();
    changed.scope.member_id = "other".into();
    assert_ne!(capture_fingerprint(&changed, &format), original);
    changed = identity.clone();
    changed.scope.vault_id = "other".into();
    assert_ne!(capture_fingerprint(&changed, &format), original);
    changed = identity.clone();
    changed.scope.server = "https://other.example/".into();
    assert_ne!(capture_fingerprint(&changed, &format), original);
    changed = identity.clone();
    changed.target = CaptureTarget::ExistingSession {
        session_id: "note".into(),
        expected_audio_sha256: None,
    };
    let missing_audio = capture_fingerprint(&changed, &format);
    assert_ne!(missing_audio, original);
    changed.target = CaptureTarget::ExistingSession {
        session_id: "note".into(),
        expected_audio_sha256: Some("0".repeat(64)),
    };
    assert_ne!(capture_fingerprint(&changed, &format), missing_audio);
    let mut reversed = format.clone();
    reversed.channels.reverse();
    assert_ne!(capture_fingerprint(&identity, &reversed), original);
    assert!(validate_capture_format(&reversed).is_err());
}

#[test]
fn capture_formats_keep_supported_mono_layouts_and_reject_ambiguous_or_future_input() {
    for channel in [
        CaptureChannel::Microphone,
        CaptureChannel::SystemAudio,
        CaptureChannel::Mixed,
    ] {
        assert!(
            validate_capture_format(&CaptureFormat {
                encoding: CaptureEncoding::PcmF32Le,
                sample_rate: 16_000,
                channels: vec![channel],
            })
            .is_ok()
        );
    }
    for channels in [
        vec![],
        vec![CaptureChannel::Microphone; 2],
        vec![CaptureChannel::Mixed; 3],
    ] {
        assert!(
            validate_capture_format(&CaptureFormat {
                channels,
                ..CaptureFormat::stereo()
            })
            .is_err()
        );
    }
    assert!(
        validate_capture_format(&CaptureFormat {
            sample_rate: 48_000,
            ..CaptureFormat::stereo()
        })
        .is_err()
    );
    let mut value = serde_json::to_value(identity()).unwrap();
    value["scope"]["path"] = json!("/private/vault");
    assert!(serde_json::from_value::<CaptureIdentity>(value).is_err());
    let mut value = serde_json::to_value(CaptureFormat::stereo()).unwrap();
    value["encoding"] = json!("pcm_i16_le");
    assert!(serde_json::from_value::<CaptureFormat>(value).is_err());
}

use super::*;
use sha2::{Digest, Sha256};

const SUBTITLE: &str = "1\n00:00:00,000 --> 00:00:01,000\nRetained server subtitle\n";
#[tokio::test]
async fn media_capabilities_are_paired_even_without_optional_transcription() {
    let fixture = Fixture::new().await;
    let (status, capabilities) = fixture
        .call("alice", "GET", "/api/v1/capabilities", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let features = capabilities["features"].as_array().unwrap();
    assert!(features.contains(&json!("media_jobs")));
    assert!(features.contains(&json!("media_initial_metadata")));
}
async fn declare_subtitle(fixture: &Fixture, vault: &str, request: &str) -> Value {
    let context = fixture.service.vault(vault).await.unwrap();
    context.store.write_meta(&serde_json::from_value(json!({"id":"note","title":"Media note","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap()).await.unwrap();
    let (status, source) = fixture
        .call(
            "alice",
            "GET",
            &format!("/api/v1/vaults/{vault}/media/source/note"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{source}");
    assert!(source["recording"].is_null());
    let sha: String = Sha256::digest(SUBTITLE.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let declaration = json!({"request_id":request,"operation":{"kind":"import_subtitles","session_id":"note","filename":"captions.srt","expected":source,"source":{"sha256":sha,"size_bytes":SUBTITLE.len()}},"transcription":null,"summary":null});
    let (status, admission) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &format!("/api/v1/vaults/{vault}/media/requests"),
            Some(declaration),
            Some(request),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{admission}");
    admission
}
async fn upload(
    fixture: &Fixture,
    subject: &str,
    vault: &str,
    request: &str,
    revision: &str,
    bytes: &'static str,
) -> (StatusCode, Value) {
    upload_bytes(
        fixture,
        subject,
        vault,
        request,
        revision,
        bytes.as_bytes().to_vec(),
    )
    .await
}
pub(super) async fn upload_bytes(
    fixture: &Fixture,
    subject: &str,
    vault: &str,
    request: &str,
    revision: &str,
    bytes: Vec<u8>,
) -> (StatusCode, Value) {
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v1/vaults/{vault}/media/requests/{request}/source"
                ))
                .header(
                    "authorization",
                    format!("Bearer {}", fixture.token(subject)),
                )
                .header("x-loofah-api-major", "1")
                .header("x-request-id", request)
                .header("x-loofah-media-revision", revision)
                .header("content-type", "application/octet-stream")
                .header("content-length", bytes.len())
                .body(Body::from(bytes))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
        .unwrap();
    (status, body)
}

#[tokio::test]
async fn raw_media_import_works_with_server_transcription_disabled() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let vault = &me.personal_vault_id;
    let request = uuid::Uuid::new_v4().to_string();
    let mut output = std::io::Cursor::new(Vec::new());
    {
        let mut wav = hound::WavWriter::new(
            &mut output,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for n in 0..1600 {
            wav.write_sample::<i16>(((n as f32 / 12.0).sin() * 1000.0) as i16)
                .unwrap();
        }
        wav.finalize().unwrap();
    }
    let bytes = output.into_inner();
    let sha: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let declaration = json!({"request_id":request,"operation":{
        "kind":"import_audio","session_id":null,"expected":null,"filename":"Voice.wav","content_type":"audio/wav",
        "created_at":null,"transcribe":false,"source":{"sha256":sha,"size_bytes":bytes.len()}
    },"transcription":null,"summary":null});
    let (code, admission) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &format!("/api/v1/vaults/{vault}/media/requests"),
            Some(declaration),
            Some(&request),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{admission}");
    let session = admission["status"]["media"]["session_id"].as_str().unwrap();
    let (code, uploaded) = upload_bytes(
        &fixture,
        "alice",
        vault,
        &request,
        admission["revision"].as_str().unwrap(),
        bytes,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{uploaded}");
    let status = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let (_, status) = fixture
                .call(
                    "alice",
                    "GET",
                    &format!("/api/v1/vaults/{vault}/media/requests/{request}"),
                    None,
                )
                .await;
            if status["phase"] != "running" {
                break status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(status["phase"], "succeeded", "{status}");
    assert!(status["media"]["transcript_id"].is_null());
    let context = fixture.service.vault(vault).await.unwrap();
    assert!(
        context
            .store
            .vault_base()
            .join("sessions")
            .join(session)
            .join("audio.mp3")
            .is_file()
    );
    assert!(
        !context
            .store
            .vault_base()
            .join("sessions")
            .join(session)
            .join("transcript.json")
            .exists()
    );
    fixture.service.shutdown().await.unwrap();
}
#[tokio::test]
async fn media_wire_uses_member_private_declaration_and_verified_subtitle_publication() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Media"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
            Some(json!({"role":"editor","expected_revision":shared["revision"]})),
        )
        .await;
    let request = uuid::Uuid::new_v4().to_string();
    let admission = declare_subtitle(&fixture, vault, &request).await;
    let path = format!("/api/v1/vaults/{vault}/media/requests/{request}");
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        upload(
            &fixture,
            "bob",
            vault,
            &request,
            admission["revision"].as_str().unwrap(),
            SUBTITLE
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (code, status) = upload(
        &fixture,
        "alice",
        vault,
        &request,
        admission["revision"].as_str().unwrap(),
        SUBTITLE,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{status}");
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let (code, status) = fixture.call("alice", "GET", &path, None).await;
            assert_eq!(code, StatusCode::OK);
            if status["phase"] != "running" {
                break status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(status["phase"], "succeeded", "{status}");
    let (code, result) = fixture
        .call("alice", "GET", &format!("{path}/result"), None)
        .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(result["user_id"], alice.member.id);
    assert_eq!(result["session_id"], "note");
    assert_eq!(
        fixture
            .call("bob", "GET", &format!("{path}/result"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    fixture.service.shutdown().await.unwrap();
}
#[tokio::test]
async fn media_source_revision_and_payload_mismatch_never_enter_publication() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let vault = &me.personal_vault_id;
    let request = uuid::Uuid::new_v4().to_string();
    let admission = declare_subtitle(&fixture, vault, &request).await;
    assert_eq!(
        upload(
            &fixture,
            "alice",
            vault,
            &request,
            &uuid::Uuid::new_v4().to_string(),
            SUBTITLE
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let mut changed = SUBTITLE.as_bytes().to_vec();
    changed[0] = b'2';
    assert_eq!(
        upload_bytes(
            &fixture,
            "alice",
            vault,
            &request,
            admission["revision"].as_str().unwrap(),
            changed
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let context = fixture.service.vault(vault).await.unwrap();
    assert!(
        !context
            .store
            .vault_base()
            .join("sessions/note/transcript.json")
            .exists()
    );
    let (code, status) = fixture
        .call(
            "alice",
            "GET",
            &format!("/api/v1/vaults/{vault}/media/requests/{request}"),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(status["revision"], admission["revision"]);
    assert_eq!(status["phase"], "receiving");
    fixture.service.shutdown().await.unwrap();
}

use super::*;

#[tokio::test]
#[ignore = "requires explicitly provisioned LOOFAH_WHISPER_MODEL; real checked media replacement"]
async fn real_configured_media_replacement_uses_shared_cpu_and_original_member_source() {
    let path = std::fs::canonicalize(std::env::var("LOOFAH_WHISPER_MODEL").unwrap()).unwrap();
    let model = std::fs::read(&path).unwrap();
    assert_eq!(
        digest(&model),
        "c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9"
    );
    let fixture = Fixture::with_transcription(None, None, Some(model_config(path, &model))).await;
    let mut resources = fixture.service.transcription_resource_observer().unwrap();
    tokio::time::timeout(Duration::from_secs(180), async {
        while resources.snapshot().phase
            != hypr_transcribe_whisper_local::resources::ResourcePhase::Ready
        {
            assert!(!matches!(
                resources.snapshot().phase,
                hypr_transcribe_whisper_local::resources::ResourcePhase::Failed(_)
            ));
            resources.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_real_media(fixture).await;
    assert!(matches!(
        resources.snapshot().phase,
        hypr_transcribe_whisper_local::resources::ResourcePhase::Stopped
    ));
}

async fn assert_real_media(fixture: Fixture) {
    let me = fixture.login("alice").await;
    let vault = &me.personal_vault_id;
    let context = fixture.service.vault(vault).await.unwrap();
    let audio = speech_wav();
    let directory = seed(&context, &audio).await;
    context.store.write_transcript("speech", serde_json::from_value(json!({
        "id":"previous-transcript","session_id":"speech","words":[{"text":"Previous transcript","start_ms":0,"end_ms":10,"channel":0}]
    })).unwrap()).await.unwrap();
    let (_, source) = fixture
        .call(
            "alice",
            "GET",
            &format!("/api/v1/vaults/{vault}/media/source/speech"),
            None,
        )
        .await;
    assert_eq!(source["recording"]["transcript_present"], true);
    let request = uuid::Uuid::new_v4().to_string();
    let declaration = json!({"request_id":request,"operation":{"kind":"transcribe","expected":source["recording"]},"transcription":{"model":"operator-base","languages":["en"]},"summary":null});
    let route = format!("/api/v1/vaults/{vault}/media/requests");
    let (code, admission) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &route,
            Some(declaration.clone()),
            Some(&request),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{admission}");
    let status = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let (code, status) = fixture
                .call("alice", "GET", &format!("{route}/{request}"), None)
                .await;
            assert_eq!(code, StatusCode::OK, "{status}");
            if status["phase"] != "running" {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(status["phase"], "succeeded", "{status}");
    let (_, result) = fixture
        .call("alice", "GET", &format!("{route}/{request}/result"), None)
        .await;
    let result: TranscriptWithData = serde_json::from_value(result).unwrap();
    assert_eq!(result.user_id, me.member.id);
    assert_ne!(result.id, "previous-transcript");
    assert!(result.words.iter().any(|word| word.channel == 0.0));
    assert!(result.words.iter().any(|word| word.channel == 1.0));
    assert_eq!(std::fs::read(directory.join("audio.wav")).unwrap(), audio);
    assert_eq!(
        std::fs::read(directory.join("unknown-user-file.txt")).unwrap(),
        b"Private user attachment"
    );
    let canonical =
        hypr_vault_read::transcript::read_transcript_json(context.store.vault_base(), "speech")
            .unwrap();
    assert_eq!(canonical.transcripts[0].id, result.id);
    let (code, replay) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &route,
            Some(declaration),
            Some(&request),
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(replay["status"]["media"]["transcript_id"], result.id);
    let imported_request = uuid::Uuid::new_v4().to_string();
    let declaration = json!({"request_id":imported_request,"operation":{
        "kind":"import_audio","session_id":null,"expected":null,"filename":"Voice.wav","content_type":"audio/wav",
        "created_at":null,"transcribe":true,"source":{"sha256":digest(&audio),"size_bytes":audio.len()}
    },"transcription":{"model":"operator-base","languages":["en"]},"summary":null});
    let (code, imported) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &route,
            Some(declaration),
            Some(&imported_request),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{imported}");
    let imported_session = imported["status"]["media"]["session_id"].as_str().unwrap();
    let (code, uploaded) = super::super::media_jobs::upload_bytes(
        &fixture,
        "alice",
        vault,
        &imported_request,
        imported["revision"].as_str().unwrap(),
        audio.clone(),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{uploaded}");
    let status = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let (code, status) = fixture
                .call("alice", "GET", &format!("{route}/{imported_request}"), None)
                .await;
            assert_eq!(code, StatusCode::OK, "{status}");
            if status["phase"] != "running" {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(status["phase"], "succeeded", "{status}");
    let (_, imported_result) = fixture
        .call(
            "alice",
            "GET",
            &format!("{route}/{imported_request}/result"),
            None,
        )
        .await;
    assert_eq!(imported_result["user_id"], me.member.id);
    assert_eq!(imported_result["session_id"], imported_session);
    assert!(!imported_result["words"].as_array().unwrap().is_empty());
    assert!(
        context
            .store
            .vault_base()
            .join("sessions")
            .join(imported_session)
            .join("audio.mp3")
            .is_file()
    );
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires explicit LOOFAH_ONNX_MODELS directory; real portable CPU batch and diarization"]
async fn real_configured_onnx_media_preserves_shared_checked_publication() {
    let root = std::path::PathBuf::from(std::env::var("LOOFAH_ONNX_MODELS").unwrap());
    let scratch = tempfile::tempdir().unwrap();
    let config = serde_json::from_value(json!({
        "model":"operator-base", "model_path":root.join("batch"),
        "onnx":{"streaming_model_path":root.join("streaming"), "diarizer_model_path":root.join("diarizer"), "snapshot_root":scratch.path()}
    })).unwrap();
    let fixture = Fixture::with_transcription(None, None, Some(config)).await;
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let state = fixture.service.transcription_policy().availability;
            if state == hypr_loofah_protocol::TranscriptionAvailability::Ready {
                break;
            }
            assert_eq!(
                state,
                hypr_loofah_protocol::TranscriptionAvailability::Loading
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_real_media(fixture).await;
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
}

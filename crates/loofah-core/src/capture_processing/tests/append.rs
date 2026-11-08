use super::*;

async fn append_capture(
    context: &Arc<VaultContext>,
    root: &std::path::Path,
    summary: bool,
) -> CaptureIngestStatus {
    let meta = serde_json::from_value(serde_json::json!({"id":"append-note","title":"Original title","created_at":"2026-11-08T10:00:00Z","tags":[],"audio":{"source":"recording","layout":"mic_system"}})).unwrap();
    context
        .store
        .create_note_session(&meta, "Original notes")
        .await
        .unwrap();
    let directory = root.join("vault/sessions/append-note");
    let mut wave =
        hound::WavWriter::create(directory.join("audio.wav"), hypr_audio_utils::pcm_spec(2))
            .unwrap();
    for _ in 0..16001 {
        wave.write_sample(0.25_f32).unwrap();
        wave.write_sample(-0.25_f32).unwrap();
    }
    wave.finalize().unwrap();
    std::fs::write(directory.join("transcript.json"), serde_json::to_vec(&serde_json::json!({"future":{"preserve":true},"transcripts":[{"id":uuid::Uuid::new_v4().to_string(),"session_id":"append-note","started_at":1000,"future":{"unknown":42},"words":[{"id":"old-word","text":"Original","start_ms":0,"end_ms":20,"channel":0,"future":"preserve"}]}]})).unwrap()).unwrap();
    let input = CaptureAppendRequest {
        scope: CaptureScope {
            server: "https://capture.example/".into(),
            member_id: MEMBER.into(),
            vault_id: "vault".into(),
        },
        session_id: "append-note".into(),
        capture_id: uuid::Uuid::new_v4().to_string(),
        request_id: uuid::Uuid::new_v4().to_string(),
        admission_id: uuid::Uuid::new_v4().to_string(),
        processing: Some(CaptureProcessingOptions {
            model: None,
            languages: vec!["en".into()],
            keywords: vec!["Loofah".into()],
            summary: summary.then_some(CaptureProcessingSummary {
                target: DocumentTarget::Summary,
                title_policy: TitlePolicy::Preserve,
            }),
        }),
    };
    let admission = context
        .prepare_capture_append_authorized(MEMBER.into(), input.clone(), || async { Ok(()) })
        .await
        .unwrap();
    let identity = CaptureIdentity {
        scope: input.scope,
        capture_id: input.capture_id,
        request_id: input.request_id,
        target: CaptureTarget::AppendSession {
            session_id: input.session_id,
            admission,
        },
    };
    context
        .begin_capture_authorized(
            MEMBER.into(),
            CaptureIngestRequest {
                identity,
                format: CaptureFormat::stereo(),
                first_admitted_audio_observed_at_ms: 9000,
            },
            || async { Ok(()) },
        )
        .await
        .unwrap()
}
async fn seal(context: &Arc<VaultContext>, source: &CaptureIngestStatus) -> CaptureSeal {
    let writer = context
        .capture_writer(MEMBER.into(), source.identity.capture_id.clone())
        .await
        .unwrap();
    writer
        .append_chunk(0, 1600, vec![0; 1600 * 2 * 4])
        .await
        .unwrap();
    writer.seal().await.unwrap()
}
async fn publish_when_ready(
    context: &Arc<VaultContext>,
    source: &CaptureIngestStatus,
    seal: CaptureSeal,
) -> CaptureIngestStatus {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match context
                .publish_capture_authorized(
                    MEMBER.into(),
                    source.identity.capture_id.clone(),
                    seal.clone(),
                    || async { Ok(()) },
                )
                .await
            {
                Ok(status) => return status,
                Err(cause)
                    if matches!(
                        cause.code,
                        ErrorCode::Unavailable | ErrorCode::ResourceLimit
                    ) =>
                {
                    tokio::time::sleep(Duration::from_millis(20)).await
                }
                Err(cause) => panic!("append publication failed: {cause:?}"),
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn suffix_only_inference_preserves_original_unknown_fields_and_summarizes_exact_merged_commit()
 {
    let (temp, runtime, batch, summary, _) = fixture().await;
    let root = temp.path().canonicalize().unwrap();
    let source = append_capture(&runtime.context, &root, true).await;
    let directory = root.join("vault/sessions/append-note");
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("transcript.json")).unwrap()).unwrap();
    submit(&runtime.context, &source, true).await;
    let seal = seal(&runtime.context, &source).await;
    publish_when_ready(&runtime.context, &source, seal.clone()).await;
    let completed = terminal(&runtime.context, &source).await;
    assert_eq!(
        completed.state,
        CaptureProcessingState::Succeeded,
        "{completed:?}"
    );
    assert_eq!(batch.calls.load(Ordering::Acquire), 1);
    assert_eq!(batch.observed_frames.load(Ordering::Acquire), 1600);
    assert_eq!(summary.calls.load(Ordering::Acquire), 1);
    let merged: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("transcript.json")).unwrap()).unwrap();
    assert_eq!(merged["future"], original["future"]);
    assert_eq!(merged["transcripts"][0], original["transcripts"][0]);
    assert_eq!(merged["transcripts"][1]["words"][0]["start_ms"], 1000.0625);
    assert_eq!(merged["transcripts"][1]["started_at"], 1000.0);
    assert_eq!(
        hound::WavReader::open(directory.join("audio.wav"))
            .unwrap()
            .duration(),
        17601
    );
    runtime
        .context
        .publish_capture_authorized(
            MEMBER.into(),
            source.identity.capture_id.clone(),
            seal,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    assert_eq!(batch.calls.load(Ordering::Acquire), 1);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn lost_suffix_output_stays_uncertain_on_restart_and_never_repeats_inference() {
    let (temp, runtime, batch, _, _) = fixture().await;
    let root = temp.path().canonicalize().unwrap();
    let source = append_capture(&runtime.context, &root, false).await;
    submit(&runtime.context, &source, false).await;
    seal(&runtime.context, &source).await;
    let original = runtime
        .context
        .capture_append_source(&source.identity, false)
        .await
        .unwrap();
    let result = original.directory().join("core-suffix-result.json");
    tokio::time::timeout(Duration::from_secs(20), async {
        while !result.is_file() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(batch.calls.load(Ordering::Acquire), 1);
    std::fs::remove_file(result).unwrap();
    drop(original);
    runtime.shutdown().await.unwrap();
    let mut runtime = open_runtime(&root).await;
    let auth = Arc::new(Auth(false.into()));
    runtime
        .prepare_capture_processing(policy("a"), auth.clone())
        .await
        .unwrap();
    runtime
        .enable_transcription(Arc::new(batch.clone()), auth, Default::default())
        .await
        .unwrap();
    runtime.start_capture_processing();
    let state = terminal(&runtime.context, &source).await;
    assert_eq!(state.state, CaptureProcessingState::Uncertain);
    assert_eq!(batch.calls.load(Ordering::Acquire), 1);
    assert_eq!(
        hound::WavReader::open(root.join("vault/sessions/append-note/audio.wav"))
            .unwrap()
            .duration(),
        16001
    );
    runtime.shutdown().await.unwrap();
}

use super::*;

async fn live_status(
    context: &Arc<hypr_loofah_core::VaultContext>,
    member: &str,
    capture: &str,
    generation: &str,
) -> CaptureLiveSnapshot {
    context
        .capture_live_status_authorized(
            member.into(),
            capture.into(),
            generation.into(),
            || async { Ok(()) },
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires existing provisioned LOOFAH_WHISPER_MODEL; configured service live CPU"]
async fn real_configured_cpu_live_captions_precede_seal_and_promote_matching_capture() {
    let path = std::fs::canonicalize(std::env::var("LOOFAH_WHISPER_MODEL").unwrap()).unwrap();
    let model = std::fs::read(&path).unwrap();
    assert_eq!(
        digest(&model),
        "c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9"
    );
    let fixture =
        Fixture::with_transcription(None, None, Some(model_config(path.clone(), &model))).await;
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
    let me = fixture.login("alice").await;
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    let metadata=serde_json::from_value(json!({"id":"live","title":"Live capture","created_at":"2026-11-08T00:00:00Z","tags":[],"audio":{"source":"recording","layout":"mic_system"}})).unwrap();
    context
        .store
        .create_note_session(&metadata, "Keep this exact saved note.")
        .await
        .unwrap();
    let directory = context.store.vault_base().join("sessions/live");
    std::fs::write(
        directory.join("unknown-user-file.txt"),
        b"private attachment",
    )
    .unwrap();
    let metadata_path = directory.join("_meta.json");
    let mut raw: Value = serde_json::from_slice(&std::fs::read(&metadata_path).unwrap()).unwrap();
    raw["future"] = json!({"unchanged":true});
    raw["audio"]["future"] = json!("nested");
    std::fs::write(&metadata_path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    context.store.refresh_session("live").await.unwrap();
    let note = std::fs::read(directory.join("notes.md")).unwrap();
    let capture = uuid::Uuid::new_v4().to_string();
    let identity = CaptureIdentity {
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
        capture_id: capture.clone(),
        request_id: uuid::Uuid::new_v4().to_string(),
        target: CaptureTarget::ExistingSession {
            session_id: "live".into(),
            expected_audio_sha256: None,
        },
    };
    let input = CaptureIngestRequest {
        identity,
        format: CaptureFormat::stereo(),
        first_admitted_audio_observed_at_ms: 1_794_139_200_123,
    };
    let begun = context
        .begin_capture_authorized(me.member.id.clone(), input.clone(), || async { Ok(()) })
        .await
        .unwrap();
    let generation = uuid::Uuid::new_v4().to_string();
    let catalog = fixture.service.catalog.clone();
    let member = me.member.id.clone();
    let vault = me.personal_vault_id.clone();
    let live_started = std::time::Instant::now();
    context
        .start_capture_live_authorized(
            me.member.id.clone(),
            capture.clone(),
            generation.clone(),
            CaptureLiveRequest {
                capture_fingerprint: begun.prefix.capture_fingerprint.clone(),
                expected_transcript: AbsentTranscriptExpectation::Absent,
                model: "operator-base".into(),
                languages: vec!["en".into()],
                keywords: vec![],
            },
            move || async move {
                catalog
                    .access_member(&member, &vault, true)
                    .await
                    .map_err(|cause| cause.0)
            },
        )
        .await
        .unwrap();
    let wav = speech_wav();
    let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).unwrap();
    assert_eq!(reader.spec().sample_rate, 16000);
    assert_eq!(reader.spec().channels, 2);
    let pcm: Vec<u8> = reader
        .samples::<f32>()
        .flat_map(|sample| sample.unwrap().to_le_bytes())
        .collect();
    let fingerprint = begun.prefix.capture_fingerprint;
    let mut previous = hypr_loofah_protocol::capture_chain_seed(&fingerprint);
    let mut chunks = 0_u64;
    for (sequence, bytes) in pcm.chunks(16000 * 2 * 4).enumerate() {
        let mut descriptor = CaptureChunkDescriptor {
            capture_fingerprint: fingerprint.clone(),
            sequence: sequence as u64,
            first_frame: sequence as u64 * 16000,
            pcm_offset: sequence as u64 * 16000 * 8,
            frames: (bytes.len() / 8) as u32,
            bytes: bytes.len() as u32,
            sha256: digest(bytes),
            previous_digest: previous,
            digest: String::new(),
        };
        descriptor.digest = hypr_loofah_protocol::capture_chunk_digest(&descriptor);
        previous = descriptor.digest.clone();
        context
            .append_capture_authorized(
                me.member.id.clone(),
                capture.clone(),
                descriptor,
                bytes.to_vec(),
                || async { Ok(()) },
            )
            .await
            .unwrap();
        chunks += 1;
    }
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let page = context
                .capture_live_captions_authorized(
                    me.member.id.clone(),
                    capture.clone(),
                    generation.clone(),
                    None,
                    128,
                    || async { Ok(()) },
                )
                .await
                .unwrap();
            assert!(!page.snapshot.state.is_terminal(), "{:?}", page.snapshot);
            if page
                .captions
                .iter()
                .any(|caption| !caption.new_words.is_empty())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    let before_seal = live_status(&context, &me.member.id, &capture, &generation).await;
    println!(
        "CPU_LIVE_FIRST_CAPTION_WALL_MS={}",
        live_started.elapsed().as_millis()
    );
    assert_eq!(before_seal.completed_through_frame, None);
    assert!(!directory.join("transcript.json").exists());
    let source = context
        .capture_status_authorized(me.member.id.clone(), capture.clone(), || async { Ok(()) })
        .await
        .unwrap();
    assert!(source.seal.is_none());
    assert!(source.publication.is_none());
    let seal = CaptureSeal {
        capture_fingerprint: fingerprint,
        chunk_count: chunks,
        frames: (pcm.len() / 8) as u64,
        pcm_bytes: pcm.len() as u64,
        chain_digest: previous,
        source_sha256: digest(&pcm),
    };
    context
        .seal_capture_authorized(
            me.member.id.clone(),
            capture.clone(),
            seal.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let published = context
        .publish_capture_authorized(me.member.id.clone(), capture.clone(), seal, || async {
            Ok(())
        })
        .await
        .unwrap();
    let done = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let snapshot = live_status(&context, &me.member.id, &capture, &generation).await;
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(done.state, CaptureLiveState::Completed, "{done:?}");
    assert_eq!(done.completed_through_frame, Some((pcm.len() / 8) as u64));
    let journal_path = fixture
        .service
        .config
        .storage_root
        .join("runtime")
        .join(&me.personal_vault_id)
        .join("creates/capture-live")
        .join(&generation)
        .join("record.json");
    let journal: Value = serde_json::from_slice(&std::fs::read(journal_path).unwrap()).unwrap();
    let caption_bytes = journal["caption_bytes"].as_u64().unwrap();
    let duration = pcm.len() as f64 / (16000.0 * 8.0);
    println!("CPU_LIVE_SOURCE_SECONDS={duration}");
    println!("CPU_LIVE_CAPTION_COUNT={}", done.next_caption_sequence);
    println!("CPU_LIVE_CAPTION_BYTES={caption_bytes}");
    println!(
        "CPU_LIVE_TWO_HOUR_PROJECTED_CAPTIONS={}",
        (done.next_caption_sequence as f64 * 7200.0 / duration).ceil()
    );
    println!(
        "CPU_LIVE_TWO_HOUR_PROJECTED_CAPTION_BYTES={}",
        (caption_bytes as f64 * 7200.0 / duration).ceil()
    );
    let retained = context
        .capture_live_result_authorized(
            me.member.id.clone(),
            capture.clone(),
            generation.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.transcript.user_id, me.member.id);
    assert_eq!(
        retained.transcript.started_at,
        input.first_admitted_audio_observed_at_ms as f64
    );
    assert!(
        retained
            .transcript
            .words
            .iter()
            .any(|word| word.channel == 0.0)
    );
    assert!(
        retained
            .transcript
            .words
            .iter()
            .any(|word| word.channel == 1.0)
    );
    let canonical =
        hypr_vault_read::transcript::read_transcript_json(context.store.vault_base(), "live")
            .unwrap();
    assert_eq!(canonical.transcripts, [retained.transcript]);
    let canonical_audio = std::fs::read(directory.join("audio.wav")).unwrap();
    assert_eq!(
        digest(&canonical_audio),
        published.publication.unwrap().canonical_audio_sha256
    );
    let metadata: Value = serde_json::from_slice(&std::fs::read(metadata_path).unwrap()).unwrap();
    assert_eq!(metadata["future"], raw["future"]);
    assert_eq!(metadata["audio"]["future"], raw["audio"]["future"]);
    assert_eq!(std::fs::read(directory.join("notes.md")).unwrap(), note);
    assert_eq!(
        std::fs::read(directory.join("unknown-user-file.txt")).unwrap(),
        b"private attachment"
    );
    println!("CPU_LIVE_MODEL_SHA256={}", digest(&model));
    println!("CPU_LIVE_SOURCE_SHA256={}", digest(&pcm));
    println!("CPU_LIVE_CANONICAL_SHA256={}", digest(&canonical_audio));
    println!(
        "CPU_LIVE_TRANSCRIPT_SHA256={}",
        digest(&std::fs::read(directory.join("transcript.json")).unwrap())
    );
    fixture.service.shutdown().await.unwrap();
    assert_eq!(
        resources.snapshot().phase,
        hypr_transcribe_whisper_local::resources::ResourcePhase::Stopped
    );
    assert_eq!(digest(&std::fs::read(path).unwrap()), digest(&model));
}

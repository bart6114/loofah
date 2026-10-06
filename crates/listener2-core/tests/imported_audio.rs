use std::sync::Arc;

use hypr_fs_format::{AudioLayout, AudioSource, SessionAudio};
use listener2_core::{BatchEvent, BatchParams, BatchProvider, BatchRuntime, run_batch};

struct Runtime;
impl BatchRuntime for Runtime {
    fn emit(&self, _event: BatchEvent) {}
}

#[tokio::test]
#[ignore = "requires installed local models and scripts/generate-import-audio-fixture.py output"]
async fn synthetic_stereo_is_transcribed_once_with_speakers() {
    let directory =
        std::env::var("LOOFAH_SYNTHETIC_AUDIO_DIR").expect("synthetic fixture directory");
    assert!(
        hypr_transcribe_soniqo::diarize::is_ready(),
        "speaker model must be installed"
    );
    for filename in ["synthetic-mono.wav", "synthetic-room.wav"] {
        let path = std::path::Path::new(&directory).join(filename);
        let original = std::fs::read(&path).unwrap();
        let output = run_batch(
            Arc::new(Runtime),
            BatchParams {
                audio: SessionAudio {
                    source: AudioSource::Import,
                    layout: AudioLayout::Mixed,
                },
                session_id: "synthetic-import-qa".into(),
                file_path: path.to_string_lossy().into_owned(),
                provider: BatchProvider::Soniqo,
                model: Some("soniqo-parakeet-batch".into()),
                base_url: hypr_transcribe_soniqo::LOCAL_BASE_URL.into(),
                api_key: String::new(),
                languages: vec![],
                keywords: vec![],
                num_speakers: None,
                min_speakers: None,
                max_speakers: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(output.response.results.channels.len(), 1);
        let alternative = &output.response.results.channels[0].alternatives[0];
        let text = alternative.transcript.to_lowercase();
        assert_eq!(
            text.matches("blue lantern").count(),
            2,
            "intentional repetition must survive: {text}"
        );
        for phrase in [
            "yellow pencils",
            "garden gate",
            "purple bicycle",
            "practice conversation",
        ] {
            assert_eq!(text.matches(phrase).count(), 1, "{phrase}: {text}");
        }
        assert!(alternative.words.iter().all(|word| word.channel == 2));
        let speakers: std::collections::BTreeSet<_> =
            alternative.words.iter().filter_map(|w| w.speaker).collect();
        assert!(
            speakers.len() >= 2,
            "two synthetic voices should have speaker changes"
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
        println!(
            "{filename}: {} words, {} speaker clusters; all utterance counts correct",
            alternative.words.len(),
            speakers.len()
        );
    }
}

#[tokio::test]
#[ignore = "requires LOOFAH_WHISPER_MODEL; 85-minute import with real Whisper inference"]
async fn long_whisper_import_preserves_speech_at_both_ends() {
    use std::io::{Seek, SeekFrom, Write};

    let mut file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
    let samples = 85 * 60 * 16_000_u32;
    let data_size = samples * 4;
    let mono = match std::env::var("LOOFAH_WHISPER_LONG_SPEECH") {
        Ok(path) => {
            let mut reader = hound::WavReader::open(path).unwrap();
            assert_eq!(reader.spec().channels, 1);
            assert_eq!(reader.spec().sample_rate, 16_000);
            reader
                .samples::<i16>()
                .flat_map(|sample| sample.unwrap().to_le_bytes())
                .collect::<Vec<_>>()
        }
        Err(_) => hypr_data::english_1::AUDIO.to_vec(),
    };
    let speech = mono
        .chunks_exact(2)
        .flat_map(|sample| [sample[0], sample[1], sample[0], sample[1]])
        .collect::<Vec<_>>();
    // A sparse stereo PCM WAV forces PreparedAudio's mono-float conversion
    // above both old upload limits without filling the fixture with speech.
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + data_size).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16_u32.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&2_u16.to_le_bytes()).unwrap();
    file.write_all(&16_000_u32.to_le_bytes()).unwrap();
    file.write_all(&64_000_u32.to_le_bytes()).unwrap();
    file.write_all(&4_u16.to_le_bytes()).unwrap();
    file.write_all(&16_u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&data_size.to_le_bytes()).unwrap();
    file.write_all(&speech).unwrap();
    file.as_file().set_len(44 + u64::from(data_size)).unwrap();
    file.seek(SeekFrom::End(-(speech.len() as i64))).unwrap();
    file.write_all(&speech).unwrap();
    file.flush().unwrap();

    let app = hypr_transcribe_whisper_local::TranscribeService::builder()
        .model_path(std::env::var("LOOFAH_WHISPER_MODEL").unwrap().into())
        .build()
        .into_router(|error: String| async move {
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, error)
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let start = std::time::Instant::now();
    let output = run_batch(
        Arc::new(Runtime),
        BatchParams {
            audio: SessionAudio {
                source: AudioSource::Import,
                layout: AudioLayout::Mixed,
            },
            session_id: "long-whisper-import".into(),
            file_path: file.path().to_string_lossy().into_owned(),
            provider: BatchProvider::WhisperLocal,
            model: Some("whisper-large-v3".into()),
            base_url: format!("http://{address}/v1"),
            api_key: String::new(),
            languages: vec![
                std::env::var("LOOFAH_WHISPER_LONG_LANGUAGE")
                    .unwrap_or_else(|_| "en".into())
                    .parse()
                    .unwrap(),
            ],
            keywords: vec![],
            num_speakers: None,
            min_speakers: None,
            max_speakers: None,
        },
    )
    .await
    .unwrap();
    server.abort();
    assert_eq!(output.mode, listener2_core::BatchRunMode::Streamed);
    assert_eq!(output.response.results.channels.len(), 1);
    let words = &output.response.results.channels[0].alternatives[0].words;
    assert!(words.iter().any(|word| word.start < 60.0));
    assert!(words.iter().any(|word| word.start > 84.0 * 60.0));
    assert!(words.iter().all(|word| word.start.is_finite()
        && word.end >= word.start
        && word.end <= 85.0 * 60.0 + 0.1));
    assert_eq!(output.response.metadata["session_audio"]["layout"], "mixed");
    println!(
        "{}",
        output.response.results.channels[0].alternatives[0].transcript
    );
    println!(
        "85-minute stereo import: {} bytes, {} words, elapsed {:?}",
        data_size,
        words.len(),
        start.elapsed()
    );
}

#[tokio::test]
#[ignore = "requires LOOFAH_WHISPER_MODEL and LOOFAH_WHISPER_AUDIO; real large recording"]
async fn actual_large_whisper_import() {
    struct Metrics {
        start: std::time::Instant,
        progress: std::sync::Mutex<f64>,
        first_progress: std::sync::atomic::AtomicBool,
        first_words: std::sync::atomic::AtomicBool,
    }
    impl BatchRuntime for Metrics {
        fn emit(&self, event: BatchEvent) {
            use std::sync::atomic::Ordering;
            if let BatchEvent::BatchResponseStreamed { event, .. } = event {
                let percentage = event.percentage();
                let mut previous = self.progress.lock().unwrap();
                assert!(percentage >= *previous, "Progress moved backwards");
                *previous = percentage;
                if !self.first_progress.swap(true, Ordering::Relaxed) {
                    println!("first_progress_ms={}", self.start.elapsed().as_millis());
                }
                if matches!(
                    event,
                    owhisper_interface::batch_stream::BatchStreamEvent::Segment { .. }
                ) && !self.first_words.swap(true, Ordering::Relaxed)
                {
                    println!("first_words_ms={}", self.start.elapsed().as_millis());
                }
            }
        }
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter("listener2_core=info,transcribe_whisper_local=info,audio_utils=info")
        .with_test_writer()
        .try_init();
    let path = std::env::var("LOOFAH_WHISPER_AUDIO").unwrap();
    let app = hypr_transcribe_whisper_local::TranscribeService::builder()
        .model_path(std::env::var("LOOFAH_WHISPER_MODEL").unwrap().into())
        .build()
        .into_router(|error: String| async move {
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, error)
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let start = std::time::Instant::now();
    let runtime = Arc::new(Metrics {
        start,
        progress: Default::default(),
        first_progress: Default::default(),
        first_words: Default::default(),
    });
    let output = run_batch(
        runtime,
        BatchParams {
            audio: SessionAudio {
                source: AudioSource::Import,
                layout: AudioLayout::Mixed,
            },
            session_id: "real-large-whisper-qa".into(),
            file_path: path,
            provider: BatchProvider::WhisperLocal,
            model: Some("whisper-large-v3-turbo".into()),
            base_url: format!("http://{address}/v1"),
            api_key: String::new(),
            languages: vec!["nl".parse().unwrap()],
            keywords: vec![],
            num_speakers: None,
            min_speakers: None,
            max_speakers: None,
        },
    )
    .await
    .unwrap();
    server.abort();
    let alternative = &output.response.results.channels[0].alternatives[0];
    let words = &alternative.words;
    assert!(!words.is_empty());
    assert!(words.iter().any(|word| word.start < 60.0));
    let duration = output.response.metadata["duration"].as_f64().unwrap();
    assert!(words.iter().any(|word| word.start > duration - 120.0));
    assert!(words.iter().all(|word| word.start.is_finite()
        && word.end >= word.start
        && word.end <= duration + 0.1
        && word.channel == 2));
    let speakers: std::collections::BTreeSet<_> = words.iter().filter_map(|w| w.speaker).collect();
    if hypr_transcribe_soniqo::diarize::is_ready() {
        assert!(!speakers.is_empty());
    }
    println!(
        "duration={duration} words={} speakers={} total_ms={}",
        words.len(),
        speakers.len(),
        start.elapsed().as_millis()
    );
    if let Ok(path) = std::env::var("LOOFAH_WHISPER_QA_OUTPUT") {
        std::fs::write(path, serde_json::to_vec(&output.response).unwrap()).unwrap();
    }
}

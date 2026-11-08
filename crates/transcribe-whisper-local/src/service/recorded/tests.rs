use super::*;
use std::sync::{OnceLock, atomic::Ordering};

use tracing_subscriber::{Layer, layer::SubscriberExt};

const AUDIO_SECONDS: usize = 16;

fn loaded() -> &'static hypr_whisper_local::LoadedWhisper {
    static MODEL: OnceLock<hypr_whisper_local::LoadedWhisper> = OnceLock::new();
    MODEL.get_or_init(|| {
        let model = hypr_whisper_local::LoadedWhisper::builder()
            .model_path(std::env::var("LOOFAH_WHISPER_MODEL").expect("LOOFAH_WHISPER_MODEL"))
            .use_gpu(false)
            .build()
            .unwrap();
        assert!(matches!(
            model.session(vec![]).unwrap().backend_name(),
            "CPU" | "BLAS"
        ));
        model
    })
}

fn params() -> ListenParams {
    ListenParams {
        languages: vec!["en".parse().unwrap()],
        keywords: vec!["Jersey".into(), "Loofah".into()],
        ..Default::default()
    }
}

fn audio() -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let data_bytes = (16000 * AUDIO_SECONDS * 4) as u32;
    let mut wav = Vec::with_capacity(data_bytes as usize + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16000_u32.to_le_bytes());
    wav.extend_from_slice(&64000_u32.to_le_bytes());
    wav.extend_from_slice(&4_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    for bytes in hypr_data::english_2::AUDIO[..16000 * 2 * AUDIO_SECONDS].chunks_exact(2) {
        wav.extend_from_slice(&[bytes[0], bytes[1], 0, 0]);
    }
    std::fs::write(file.path(), wav).unwrap();
    file
}

#[test]
#[ignore = "requires LOOFAH_WHISPER_MODEL; real CPU batch kernel"]
fn cpu_kernel_preserves_channels_timestamps_progress_and_rejects_failed_emitters() {
    let model_path = std::env::var("LOOFAH_WHISPER_MODEL").unwrap();
    let file = audio();
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut events = Vec::new();
    let response = transcribe_batch_file(
        loaded(),
        Path::new(&model_path),
        file.path(),
        &params(),
        cancelled.clone(),
        |event| {
            events.push(event);
            Ok(())
        },
    )
    .unwrap();
    assert!(!cancelled.load(Ordering::Acquire));
    assert_eq!(response.results.channels.len(), 2);
    assert_eq!(response.metadata["channels"], 2);
    assert_eq!(response.metadata["duration"], AUDIO_SECONDS as f64);
    let alternative = &response.results.channels[0].alternatives[0];
    let text = alternative.transcript.to_lowercase();
    assert!(text.contains("hello") && text.contains("jersey"), "{text}");
    assert!(!alternative.words.is_empty());
    assert!(alternative.words.iter().all(|word| word.channel == 0
        && word.start.is_finite()
        && word.end.is_finite()
        && word.start >= 0.0
        && word.end >= word.start
        && word.end <= AUDIO_SECONDS as f64));
    assert!(
        response.results.channels[1].alternatives[0]
            .words
            .is_empty()
    );
    let mut previous = 0.0;
    let mut segments = 0;
    for event in events {
        match event {
            BatchSseMessage::Progress { progress } => {
                assert!(progress.percentage >= previous && progress.percentage <= 0.99);
                previous = progress.percentage;
            }
            BatchSseMessage::Segment { .. } => segments += 1,
            _ => panic!("the kernel emits only progress and segments"),
        }
    }
    assert!(segments > 0 && previous > 0.0);
    let mut emitted = 0;
    let error = transcribe_batch_file(
        loaded(),
        Path::new(&model_path),
        file.path(),
        &params(),
        cancelled.clone(),
        |_| {
            emitted += 1;
            Err(crate::Error::protocol("test receiver closed"))
        },
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "test receiver closed");
    assert_eq!(emitted, 1);
    assert!(cancelled.load(Ordering::Acquire));
    let error = transcribe_batch_file(
        loaded(),
        Path::new(&model_path),
        Path::new("missing-audio"),
        &params(),
        cancelled,
        |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "batch transcription cancelled");
}

#[test]
#[ignore = "requires LOOFAH_WHISPER_MODEL and retained generated speech WAV; real CPU gap regression"]
fn cpu_kernel_recognizes_discontinuous_retained_speech() {
    let model_path = std::env::var("LOOFAH_WHISPER_MODEL").unwrap();
    let source = std::env::var("LOOFAH_WHISPER_RETAINED_AUDIO").unwrap();
    let output = std::env::var("LOOFAH_WHISPER_REPAIR_OUTPUT").unwrap();
    let response = transcribe_batch_file(
        loaded(),
        Path::new(&model_path),
        Path::new(&source),
        &ListenParams {
            languages: vec!["en".parse().unwrap()],
            ..Default::default()
        },
        Arc::new(AtomicBool::new(false)),
        |_| Ok(()),
    )
    .unwrap();
    std::fs::write(&output, serde_json::to_vec(&response).unwrap()).unwrap();
    assert_eq!(response.results.channels.len(), 2);
    let duration = response.metadata["duration"].as_f64().unwrap();
    assert!(duration >= 330.0);
    let words = &response.results.channels[0].alternatives[0].words;
    for window in (0..duration as u64).step_by(300) {
        if window + 30 > duration as u64 {
            break;
        }
        let text = words
            .iter()
            .filter(|word| word.start >= window as f64 && word.start < (window + 45) as f64)
            .map(|word| word.word.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        assert!(
            text.contains("hello") && text.contains("jersey"),
            "Retained English2 speech missing at window {window}: {text}"
        );
    }
}

#[tokio::test]
#[ignore = "requires LOOFAH_WHISPER_MODEL and LOOFAH_WHISPER_CPU=1; real JSON/SSE parity"]
async fn cpu_http_json_and_sse_keep_existing_wire_shapes_and_terminal_order() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    assert_eq!(std::env::var("LOOFAH_WHISPER_CPU").as_deref(), Ok("1"));
    let file = audio();
    let router = super::super::TranscribeService::builder()
        .model_path(std::env::var("LOOFAH_WHISPER_MODEL").unwrap().into())
        .build()
        .into_router(|error: String| async move { (StatusCode::INTERNAL_SERVER_ERROR, error) });
    for accept in ["application/json", "text/event-stream"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/listen?language=en&keywords=Jersey&keywords=Loofah")
                    .header("content-type", "audio/wav")
                    .header("accept", accept)
                    .body(Body::from(std::fs::read(file.path()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        let response: batch::Response = if accept == "application/json" {
            serde_json::from_slice(&body).unwrap()
        } else {
            let mut final_response = None;
            let mut progress = 0.0;
            let mut segments = 0;
            for line in std::str::from_utf8(&body)
                .unwrap()
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
            {
                assert!(final_response.is_none(), "event after terminal response");
                match serde_json::from_str::<BatchSseMessage>(line).unwrap() {
                    BatchSseMessage::Progress { progress: next } => {
                        assert!(next.percentage >= progress);
                        progress = next.percentage;
                    }
                    BatchSseMessage::Segment { .. } => segments += 1,
                    BatchSseMessage::Result { response } => final_response = Some(response),
                    BatchSseMessage::Error { detail, .. } => panic!("{detail}"),
                }
            }
            assert!(segments > 0 && progress > 0.0);
            final_response.expect("missing terminal response")
        };
        assert_eq!(response.metadata["channels"], 2);
        assert_eq!(response.metadata["duration"], AUDIO_SECONDS as f64);
        let text = response.results.channels[0].alternatives[0]
            .transcript
            .to_lowercase();
        assert!(text.contains("hello") && text.contains("jersey"), "{text}");
        assert!(
            response.results.channels[1].alternatives[0]
                .words
                .is_empty()
        );
    }
}

struct CancelAtMessage {
    message: &'static str,
    cancelled: Arc<AtomicBool>,
    observed: Arc<AtomicBool>,
}

impl<S: tracing::Subscriber> Layer<S> for CancelAtMessage {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        struct Visitor<'a>(&'a CancelAtMessage);
        impl tracing::field::Visit for Visitor<'_> {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" && format!("{value:?}") == self.0.message {
                    self.0.observed.store(true, Ordering::Release);
                    self.0.cancelled.store(true, Ordering::Release);
                }
            }
        }
        event.record(&mut Visitor(self));
    }
}

#[test]
#[ignore = "requires LOOFAH_WHISPER_MODEL; cancels real decode, VAD and native inference"]
fn cpu_kernel_cancellation_reaches_preparation_vad_and_native_inference() {
    let model_path = std::env::var("LOOFAH_WHISPER_MODEL").unwrap();
    let file = audio();
    let model = loaded();
    for (message, expected) in [
        ("pcm_decode_buffer_budget", "Audio preparation cancelled"),
        ("whisper_batch_audio_prepared", "Transcription cancelled"),
        ("transcribe_started", "cancelled"),
    ] {
        let cancelled = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(AtomicBool::new(false));
        let subscriber = tracing_subscriber::registry().with(CancelAtMessage {
            message,
            cancelled: cancelled.clone(),
            observed: observed.clone(),
        });
        let result = tracing::subscriber::with_default(subscriber, || {
            transcribe_batch_file(
                model,
                Path::new(&model_path),
                file.path(),
                &params(),
                cancelled,
                |_| Ok(()),
            )
        });
        assert!(
            observed.load(Ordering::Acquire),
            "missing pipeline checkpoint: {message}"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .to_lowercase()
                .contains(&expected.to_lowercase()),
            "cancellation missed {message}"
        );
    }
}

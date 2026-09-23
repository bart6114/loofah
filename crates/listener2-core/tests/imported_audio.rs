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

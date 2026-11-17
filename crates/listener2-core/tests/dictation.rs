use std::sync::Arc;

use listener2_core::{BatchEvent, BatchParams, BatchProvider, BatchRuntime, run_dictation_batch};

struct Runtime;
impl BatchRuntime for Runtime {
    fn emit(&self, _: BatchEvent) {}
}

#[tokio::test]
#[ignore = "requires Parakeet, the speaker model and LOOFAH_DICTATION_TEST_WAV containing the word test"]
async fn dictation_transcribes_without_speaker_labels() {
    assert!(hypr_transcribe_soniqo::diarize::is_ready());
    let warm_start = std::time::Instant::now();
    hypr_transcribe_soniqo::warm_model(hypr_transcribe_soniqo::SoniqoModel::ParakeetBatch).unwrap();
    println!("Warm-up: {:?}", warm_start.elapsed());
    for attempt in 1..=2 {
        let start = std::time::Instant::now();
        let output = run_dictation_batch(
            Arc::new(Runtime),
            BatchParams {
                audio: Default::default(),
                session_id: format!("dictation-{attempt}"),
                provider: BatchProvider::Soniqo,
                file_path: std::env::var("LOOFAH_DICTATION_TEST_WAV").unwrap(),
                model: Some("soniqo-parakeet-batch".into()),
                base_url: hypr_transcribe_soniqo::LOCAL_BASE_URL.into(),
                api_key: String::new(),
                languages: vec![hypr_language::ISO639::En.into()],
                keywords: vec![],
                num_speakers: Some(1),
                min_speakers: None,
                max_speakers: None,
            },
        )
        .await
        .unwrap();
        let result = &output.response.results.channels[0].alternatives[0];
        assert!(result.transcript.to_lowercase().contains("test"));
        assert!(!result.words.is_empty());
        assert!(result.words.iter().all(|word| word.speaker.is_none()));
        println!(
            "Attempt {attempt}: {:?} — {}",
            start.elapsed(),
            result.transcript
        );
    }
}

use std::{
    path::Path,
    time::{Duration, Instant},
};
use transcribe_onnx::{
    BatchSession, LiveSession,
    models::{self, Model},
};

fn ensure_downloaded(model: Model) {
    if model.verify().is_ok() {
        return;
    }
    models::start(model).unwrap();
    let started = Instant::now();
    loop {
        let state = models::state(model).unwrap();
        match state.status.as_str() {
            "ready" => break,
            "error" => panic!("{}: {:?}", model.id(), state.error),
            _ => {
                assert!(
                    started.elapsed() < Duration::from_secs(1800),
                    "Model download timed out"
                );
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
}

#[test]
#[ignore = "Downloads the pinned speech models; run on each native release architecture"]
fn pinned_models_transcribe_real_audio_on_cpu() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/src/english_2/audio.wav");
    ensure_downloaded(Model::Streaming);
    let [mut live, mut quiet] = LiveSession::load_pair().unwrap();
    let reader = hound::WavReader::open(&fixture).unwrap();
    let samples: Vec<f32> = reader
        .into_samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect();
    let mut finals = Vec::new();
    let started = Instant::now();
    for chunk in samples.chunks(1600) {
        assert!(quiet.append(&vec![0.0; chunk.len()]).unwrap().is_empty());
        for partial in live.append(chunk).unwrap() {
            if partial.is_final {
                finals.push(partial.text);
            }
        }
    }
    for partial in live.finalize().unwrap() {
        if partial.is_final {
            finals.push(partial.text);
        }
    }
    let live_text = finals.join(" ");
    eprintln!("live elapsed={:?}, text={live_text}", started.elapsed());
    assert!(
        live_text.to_lowercase().contains("hello"),
        "Live output: {live_text}"
    );
    assert!(
        !live_text.contains('▁'),
        "Tokenizer markers must not reach transcripts"
    );
    assert!(quiet.finalize().unwrap().is_empty());
    drop(live);
    drop(quiet);

    ensure_downloaded(Model::Batch);
    let mut batch = BatchSession::load().unwrap();
    let started = Instant::now();
    let (duration, segments) = batch.transcribe_file(&fixture).unwrap();
    let text = segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!("batch elapsed={:?}, text={text}", started.elapsed());
    assert!((duration - 30.0).abs() < 0.01);
    assert!(
        text.to_lowercase().contains("hello"),
        "Batch output: {text}"
    );
    assert!(
        text.to_lowercase().contains("jersey"),
        "Batch must cover the conversation: {text}"
    );
    assert!(
        text.split_whitespace().count() >= 55,
        "Batch omitted most of the fixture: {text}"
    );
    assert!(
        segments
            .iter()
            .all(|s| s.start >= 0.0 && s.end >= s.start && s.end <= duration + 1.0)
    );
}

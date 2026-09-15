use hypr_whisper::Language;
use std::time::Instant;
use whisper_local::LoadedWhisper;

#[test]
#[ignore = "requires WHISPER_BENCH_MODEL and a release build"]
fn engine_benchmark() {
    let path = std::env::var("WHISPER_BENCH_MODEL").unwrap();
    let audio: Vec<f32> = hypr_data::english_1::AUDIO
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();
    let audio = &audio[..std::env::var("WHISPER_BENCH_SECONDS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .map_or(audio.len(), |s| (s * 16000).min(audio.len()))];
    let start = Instant::now();
    let loaded = LoadedWhisper::builder().model_path(path).build().unwrap();
    println!("load_ms={}", start.elapsed().as_millis());
    for run in 0..4 {
        let mut model = loaded.session(vec![Language::En]).unwrap();
        let start = Instant::now();
        let mut count = 0;
        for chunk in audio.chunks(16000 * 5) {
            count += model.transcribe(chunk).unwrap().len();
        }
        println!(
            "run={run} audio_samples={} elapsed_ms={} segments={count}",
            audio.len(),
            start.elapsed().as_millis()
        );
        assert!(count > 0);
    }
}

#[test]
#[ignore = "requires WHISPER_BENCH_MODEL and a release build"]
fn detection_benchmark() {
    let loaded = LoadedWhisper::builder()
        .model_path(std::env::var("WHISPER_BENCH_MODEL").unwrap())
        .build()
        .unwrap();
    let audio: Vec<f32> = hypr_data::english_1::AUDIO
        .chunks_exact(2)
        .take(10 * 16000)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();
    for run in 0..4 {
        let mut model = loaded
            .session(vec![Language::En, Language::Nl, Language::Fr])
            .unwrap();
        let start = Instant::now();
        let scores = model.detect_language(&audio).unwrap();
        println!(
            "detection_run={run} elapsed_ms={} scores={:?}",
            start.elapsed().as_millis(),
            scores
        );
    }
}

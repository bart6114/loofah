use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use transcribe_onnx::{
    diarize::Diarizer,
    models::{self, Model},
};

#[test]
#[ignore = "downloads speaker models and requires LOOFAH_DIARIZATION_FIXTURE (16 kHz mono WAV)"]
fn real_speakers_have_stable_timestamped_labels() {
    let path = std::env::var("LOOFAH_DIARIZATION_FIXTURE").expect("Set LOOFAH_DIARIZATION_FIXTURE");
    models::start(Model::Diarizer).unwrap();
    let start = Instant::now();
    loop {
        let state = models::state(Model::Diarizer).unwrap();
        assert_ne!(state.status, "error", "{:?}", state.error);
        if state.status == "ready" {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(600),
            "Model download timed out"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    let mut reader = hound::WavReader::open(path).unwrap();
    assert_eq!(reader.spec().sample_rate, 16000);
    assert_eq!(reader.spec().channels, 1);
    let samples = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect::<Vec<_>>();
    let mut engine = Diarizer::load().unwrap();
    let start = Instant::now();
    let segments = engine.process(&samples, 16000).unwrap();
    let speakers = segments
        .iter()
        .map(|s| s.speaker_index)
        .collect::<BTreeSet<_>>();
    println!(
        "{speakers:?}, {:?} for {:.1}s audio: {segments:?}",
        start.elapsed(),
        samples.len() as f64 / 16000.0
    );
    let expected: usize = std::env::var("LOOFAH_EXPECTED_SPEAKERS")
        .ok()
        .map(|n| n.parse().unwrap())
        .unwrap_or(6);
    assert_eq!(speakers.len(), expected);
    if expected == 6 {
        for (speaker, time) in [4000, 12000, 20000, 26000, 33000, 40000]
            .into_iter()
            .enumerate()
        {
            assert!(
                segments.iter().any(|s| s.speaker_index == speaker as i32
                    && s.start_ms <= time
                    && s.end_ms >= time),
                "Speaker {speaker} must own the turn at {time} ms"
            );
        }
        for (seconds, count) in [(16, 2), (29, 4)] {
            let segments = engine.process(&samples[..16000 * seconds], 16000).unwrap();
            assert_eq!(
                segments
                    .iter()
                    .map(|s| s.speaker_index)
                    .collect::<BTreeSet<_>>()
                    .len(),
                count
            );
        }
    }
    assert!(segments.iter().all(|s| s.start_ms >= 0
        && s.end_ms > s.start_ms
        && s.end_ms <= samples.len() as i64 * 1000 / 16000));
    assert!(
        engine
            .process(&vec![0.0; 16000 * 10], 16000)
            .unwrap()
            .is_empty()
    );
}

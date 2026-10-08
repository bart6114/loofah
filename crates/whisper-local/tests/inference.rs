use hypr_whisper::Language;
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};
use whisper_local::LoadedWhisper;

fn transcribe(use_gpu: bool, require_gpu: bool) {
    let loaded = LoadedWhisper::builder()
        .model_path(std::env::var("LOOFAH_WHISPER_MODEL").expect("LOOFAH_WHISPER_MODEL"))
        .use_gpu(use_gpu)
        .build()
        .unwrap();
    let mut model = loaded.session(vec![Language::En]).unwrap();
    model.set_native_timestamps(true);
    if require_gpu {
        assert!(
            model.backend_name().to_lowercase().contains("vulkan"),
            "GPU required, selected {}",
            model.backend_name()
        );
    } else if !use_gpu {
        assert!(
            matches!(model.backend_name(), "CPU" | "BLAS"),
            "Unexpected CPU backend: {}",
            model.backend_name()
        );
    } else if cfg!(all(feature = "metal", target_os = "macos")) {
        assert!(model.backend_name().starts_with("MTL"));
    }
    let audio = hypr_data::english_2::AUDIO
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0)
        .collect::<Vec<_>>();
    let started = Instant::now();
    let segments = model.transcribe(&audio).unwrap();
    let text = segments
        .iter()
        .map(|segment| segment.text())
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!(
        "backend={} elapsed_ms={} text={text}",
        model.backend_name(),
        started.elapsed().as_millis()
    );
    assert!(text.to_lowercase().contains("hello"), "{text}");
    assert!(text.to_lowercase().contains("jersey"), "{text}");
    assert!(segments.iter().all(|segment| segment.start().is_finite()
        && segment.end().is_finite()
        && segment.start() >= 0.0
        && segment.end() >= segment.start()));
    model.set_cancellation(Arc::new(AtomicBool::new(true)));
    assert!(matches!(
        model.transcribe(&audio),
        Err(whisper_local::Error::Cancelled)
    ));
}

#[test]
#[ignore = "requires LOOFAH_WHISPER_MODEL"]
fn automatic_backend_transcribes() {
    transcribe(true, false);
}

#[test]
#[ignore = "requires LOOFAH_WHISPER_MODEL"]
fn forced_cpu_transcribes_and_cancels() {
    transcribe(false, false);
}

#[test]
#[ignore = "requires LOOFAH_WHISPER_MODEL and a physical Vulkan GPU"]
fn actual_gpu_transcribes() {
    transcribe(true, true);
}

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

#[test]
#[ignore = "requires LOOFAH_WHISPER_MODEL"]
fn cpu_language_detection_and_inference_abort_inside_native_graphs() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    unsafe extern "C" fn abort_after_work_started(data: *mut std::ffi::c_void) -> bool {
        let checks = unsafe { &*(data as *const AtomicUsize) };
        checks.fetch_add(1, Ordering::AcqRel) >= 3
    }

    let context = WhisperContext::new_with_params(
        std::env::var("LOOFAH_WHISPER_MODEL").expect("LOOFAH_WHISPER_MODEL"),
        WhisperContextParameters {
            use_gpu: false,
            ..Default::default()
        },
    )
    .unwrap();
    let mut state = context.create_state().unwrap();
    let audio = hypr_data::english_2::AUDIO[..16000 * 2 * 10]
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0)
        .collect::<Vec<_>>();
    state.pcm_to_mel(&audio, 2).unwrap();
    let checks = AtomicUsize::new(0);
    let started = Instant::now();
    let result = unsafe {
        state.lang_detect_with_abort(
            0,
            2,
            Some(abort_after_work_started),
            &checks as *const AtomicUsize as *mut std::ffi::c_void,
        )
    };
    assert!(
        result.is_err(),
        "language detection ignored native cancellation"
    );
    assert!(checks.load(Ordering::Acquire) >= 4);
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
    let (language, _) = state.lang_detect(0, 2).unwrap();
    assert_eq!(whisper_rs::get_lang_str(language), Some("en"));

    checks.store(0, Ordering::Release);
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("en"));
    params.set_n_threads(2);
    unsafe {
        params.set_abort_callback(Some(abort_after_work_started));
        params.set_abort_callback_user_data(&checks as *const AtomicUsize as *mut std::ffi::c_void);
    }
    assert!(
        state.full(params, &audio).is_err(),
        "inference ignored native cancellation"
    );
    assert!(checks.load(Ordering::Acquire) >= 4);
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("en"));
    params.set_n_threads(2);
    state.full(params, &audio).unwrap();
    let text = (0..state.full_n_segments())
        .map(|i| {
            state
                .get_segment(i)
                .unwrap()
                .to_str_lossy()
                .unwrap()
                .into_owned()
        })
        .collect::<String>();
    assert!(text.to_lowercase().contains("hello"), "{text}");
}

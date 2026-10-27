use super::*;
use std::time::Duration;

#[test]
#[ignore = "requires the downloaded Nemotron model and native CoreML inference"]
fn nemotron_guidance_accepts_supported_language_pairs() {
    let model = SoniqoModel::NemotronStreaming;
    assert!(is_model_downloaded(model).unwrap());
    for language in model.supported_language_codes().unwrap() {
        if language == "en" {
            continue;
        }
        let config = model.language_config(&["en".parse().unwrap(), language.parse().unwrap()]);
        LiveTranscriptionSession::start_with_config(model, &config)
            .unwrap_or_else(|error| panic!("guide for {language}: {error}"))
            .stop()
            .unwrap();
    }
}

#[test]
#[ignore = "requires the downloaded Nemotron model and native CoreML inference"]
fn nemotron_file_inference_allows_live_channels_to_progress() {
    let model = SoniqoModel::NemotronStreaming;
    assert!(is_model_downloaded(model).unwrap());
    let mut session = LiveTranscriptionSession::start_with_language(model, Some("en-US")).unwrap();
    let file =
        hypr_audio_utils::PcmFile::prepare(hypr_data::english_1::AUDIO_PATH, || false).unwrap();
    let pcm = &file.descriptor;
    let samples = pcm
        .reader()
        .unwrap()
        .channel(0, 0..pcm.frames.min(16_000 * 15))
        .unwrap();
    let batch = std::thread::spawn(move || {
        transcribe_file(model, hypr_data::english_1::AUDIO_PATH, Some("en-US")).unwrap()
    });
    std::thread::sleep(Duration::from_millis(100));
    let mut max_append = Duration::ZERO;
    for chunk in samples.chunks(4_000) {
        let started = Instant::now();
        session.append(TranscriptSource::Microphone, chunk).unwrap();
        max_append = max_append.max(started.elapsed());
    }
    session.finalize(TranscriptSource::Microphone).unwrap();
    session.stop().unwrap();
    assert!(!batch.join().unwrap().text.trim().is_empty());
    eprintln!("Maximum live append during batch inference: {max_append:?}");
    assert!(
        max_append < Duration::from_secs(5),
        "file inference blocked live transcription: {max_append:?}"
    );
}

#[test]
#[ignore = "requires downloaded Parakeet models and native CoreML inference"]
fn parakeet_models_still_transcribe_after_runtime_upgrade() {
    let path = std::env::var("LOOFAH_NEMOTRON_QA_AUDIO")
        .unwrap_or_else(|_| hypr_data::english_1::AUDIO_PATH.to_string());
    for model in [SoniqoModel::ParakeetStreaming, SoniqoModel::ParakeetBatch] {
        assert!(is_model_downloaded(model).unwrap());
        let transcript = transcribe_file(model, &path, Some("en")).unwrap();
        assert!(
            !transcript.text.trim().is_empty(),
            "{model} returned no text"
        );
        eprintln!("{model}: {}", transcript.text);
    }
    let file = hypr_audio_utils::PcmFile::prepare(&path, || false).unwrap();
    let pcm = &file.descriptor;
    let samples = pcm.reader().unwrap().channel(0, 0..pcm.frames).unwrap();
    let mut session = LiveTranscriptionSession::start(SoniqoModel::ParakeetStreaming).unwrap();
    let mut text = String::new();
    for chunk in samples.chunks(4_000) {
        for partial in session.append(TranscriptSource::Microphone, chunk).unwrap() {
            text.push_str(&partial.text);
        }
    }
    for partial in session.finalize(TranscriptSource::Microphone).unwrap() {
        text.push_str(&partial.text);
    }
    session.stop().unwrap();
    assert!(!text.trim().is_empty());
}

#[test]
#[ignore = "downloads the Nemotron model and runs native CoreML inference"]
fn nemotron_download_file_and_dual_stream() {
    let model = SoniqoModel::NemotronStreaming;
    start_model_download(model).unwrap();
    let started = Instant::now();
    loop {
        let state = model_download_state(model).unwrap();
        assert_ne!(state.status, "error", "{:?}", state.error);
        if state.status == "ready" {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(600),
            "download timed out"
        );
        std::thread::sleep(Duration::from_millis(250));
    }

    let path = std::env::var("LOOFAH_NEMOTRON_QA_AUDIO")
        .unwrap_or_else(|_| hypr_data::english_1::AUDIO_PATH.to_string());
    let hint = std::env::var("LOOFAH_NEMOTRON_QA_LANGUAGE").ok();
    let config = match std::env::var("LOOFAH_NEMOTRON_QA_LANGUAGES") {
        Ok(languages) => model.language_config(
            &languages
                .split(',')
                .map(|language| language.parse().unwrap())
                .collect::<Vec<_>>(),
        ),
        Err(_) => LanguageConfig::single(hint.as_deref()),
    };
    let batch = transcribe_file_with_config(model, &path, &config).unwrap();
    eprintln!(
        "Nemotron batch ({:.2}s): {}",
        batch.duration_seconds, batch.text
    );
    assert!(!batch.text.trim().is_empty());
    // Reloading must use the complete local cache rather than downloading again.
    reset_model(model).unwrap();
    assert!(is_model_downloaded(model).unwrap());

    let file = hypr_audio_utils::PcmFile::prepare(&path, || false).unwrap();
    let pcm = &file.descriptor;
    let samples = pcm.reader().unwrap().channel(0, 0..pcm.frames).unwrap();
    let mut session = LiveTranscriptionSession::start_with_config(model, &config).unwrap();
    let mut finals = [Vec::<LivePartial>::new(), Vec::new()];
    let mut partial_count = 0;
    let started = Instant::now();
    let repetitions = std::env::var("LOOFAH_NEMOTRON_QA_REPEATS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    for repetition in 0..repetitions {
        for chunk in samples.chunks(4_000) {
            for source in [TranscriptSource::Microphone, TranscriptSource::System] {
                for partial in session.append(source, chunk).unwrap() {
                    partial_count += 1;
                    if partial.is_final && !partial.text.trim().is_empty() {
                        finals[source.channel_index() as usize].push(partial);
                    }
                }
            }
        }
        eprintln!(
            "Completed audio pass {}/{} ({:.1} audio minutes), RSS: {} KiB",
            repetition + 1,
            repetitions,
            pcm.duration() * (repetition + 1) as f64 / 60.0,
            String::from_utf8_lossy(
                &std::process::Command::new("ps")
                    .args(["-o", "rss=", "-p", &std::process::id().to_string()])
                    .output()
                    .unwrap()
                    .stdout
            )
            .trim()
        );
        for source in [TranscriptSource::Microphone, TranscriptSource::System] {
            for partial in session.finalize(source).unwrap() {
                if partial.is_final && !partial.text.trim().is_empty() {
                    finals[source.channel_index() as usize].push(partial);
                }
            }
            assert!(session.finalize(source).unwrap().is_empty());
        }
    }
    session.stop().unwrap();
    eprintln!(
        "Nemotron dual-stream RTF: {:.3}; updates: {partial_count}",
        started.elapsed().as_secs_f64() / (pcm.duration() * repetitions as f64)
    );
    for (channel, segments) in finals.iter().enumerate() {
        assert!(
            !segments.is_empty(),
            "channel {channel} has no final transcript"
        );
        for pair in segments.windows(2) {
            assert!(
                pair[0].start_seconds.unwrap() + pair[0].duration_seconds.unwrap()
                    <= pair[1].start_seconds.unwrap() + 0.001
            );
        }
        eprintln!(
            "Channel {channel} ({} segments): {}",
            segments.len(),
            segments
                .iter()
                .take(10)
                .map(|partial| partial.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    assert_eq!(finals[0].len(), finals[1].len());
    assert_eq!(
        finals[0].iter().map(|p| &p.text).collect::<Vec<_>>(),
        finals[1].iter().map(|p| &p.text).collect::<Vec<_>>()
    );
}

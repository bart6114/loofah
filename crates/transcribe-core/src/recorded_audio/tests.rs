use super::*;
use hypr_fs_format::AudioSource;
use std::sync::Arc;

fn fixture(frames: &[[f32; 2]]) -> tempfile::NamedTempFile {
    let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
    let mut writer = hound::WavWriter::create(
        file.path(),
        hound::WavSpec {
            channels: 2,
            sample_rate: 16000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for frame in frames {
        for sample in frame {
            writer.write_sample(*sample).unwrap();
        }
    }
    writer.finalize().unwrap();
    file
}

#[test]
fn mixed_stereo_is_one_stream_without_changing_playback() {
    let file = fixture(&[[0.2, 0.1], [0.0, 0.4], [-0.2, 0.0]]);
    let original = std::fs::read(file.path()).unwrap();
    let prepared = PreparedAudio::from_path(
        file.path(),
        SessionAudio::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let source = hypr_audio_utils::source_from_path(&prepared.path).unwrap();
    assert_eq!(u16::from(source.channels()), 1);
    assert_eq!(source.collect::<Vec<_>>(), vec![0.15, 0.2, -0.1]);
    assert_eq!(std::fs::read(file.path()).unwrap(), original);
    let temporary = prepared.path.clone();
    let owner = Arc::new(prepared);
    let worker = owner.clone();
    drop(owner);
    assert!(temporary.exists());
    drop(worker);
    assert!(!temporary.exists());
}

#[test]
fn silent_and_inverted_channels_preserve_signal() {
    for frames in [
        vec![[0.2, 0.0], [-0.4, 0.0]],
        vec![[0.0, 0.2], [0.0, -0.4]],
        vec![[0.2, -0.2], [-0.4, 0.4]],
    ] {
        let file = fixture(&frames);
        let prepared = PreparedAudio::from_path(
            file.path(),
            SessionAudio::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            hypr_audio_utils::source_from_path(&prepared.path)
                .unwrap()
                .collect::<Vec<_>>(),
            vec![0.2, -0.4]
        );
    }
}

#[test]
fn native_tracks_stay_separate_but_legacy_duplicates_become_mixed() {
    let audio = SessionAudio {
        source: AudioSource::Recording,
        layout: AudioLayout::MicSystem,
    };
    for (frames, mixed) in [
        (vec![[0.2, 0.0], [0.0, 0.4]], false),
        (vec![[0.2, 0.2], [0.4, 0.4]], true),
    ] {
        let file = fixture(&frames);
        let prepared =
            PreparedAudio::from_path(file.path(), audio, &AtomicBool::new(false)).unwrap();
        assert_eq!(prepared.audio.layout == AudioLayout::Mixed, mixed);
        assert_eq!(
            u16::from(
                hypr_audio_utils::source_from_path(&prepared.path)
                    .unwrap()
                    .channels()
            ),
            if mixed { 1 } else { 2 }
        );
    }
}

#[test]
fn cancellation_does_not_change_the_source() {
    let file = fixture(&vec![[0.2, 0.1]; 16000]);
    let original = std::fs::read(file.path()).unwrap();
    assert!(
        PreparedAudio::from_path(file.path(), SessionAudio::default(), &AtomicBool::new(true))
            .is_err()
    );
    assert_eq!(std::fs::read(file.path()).unwrap(), original);
}

fn variable_fixture(channels: u16, samples: &[f32]) -> tempfile::NamedTempFile {
    let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
    let mut writer = hound::WavWriter::create(
        file.path(),
        hound::WavSpec {
            channels,
            sample_rate: 16000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for sample in samples {
        writer.write_sample(*sample).unwrap();
    }
    writer.finalize().unwrap();
    file
}

#[test]
fn mono_recordings_use_mixed_domain_without_changing_origin_or_source_samples() {
    let file = variable_fixture(1, &[0.25, -0.5, 0.125]);
    let original = std::fs::read(file.path()).unwrap();
    let prepared = PreparedAudio::from_path(
        file.path(),
        SessionAudio {
            source: AudioSource::Recording,
            layout: AudioLayout::MicSystem,
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        prepared.audio,
        SessionAudio {
            source: AudioSource::Recording,
            layout: AudioLayout::Mixed
        }
    );
    assert_eq!(prepared.pcm.channels, 1);
    assert_eq!(prepared.pcm.sample_rate, 16000);
    assert_eq!(
        hypr_audio_utils::source_from_path(&prepared.path)
            .unwrap()
            .collect::<Vec<_>>(),
        vec![0.25, -0.5, 0.125]
    );
    assert_eq!(std::fs::read(file.path()).unwrap(), original);
}

#[test]
fn multichannel_imports_mix_active_channels_but_native_multichannel_is_refused() {
    let file = variable_fixture(3, &[0.6, 0.3, 0.0, 0.0, 0.6, 0.0]);
    let original = std::fs::read(file.path()).unwrap();
    let prepared = PreparedAudio::from_path(
        file.path(),
        SessionAudio {
            source: AudioSource::Import,
            layout: AudioLayout::Mixed,
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(prepared.audio.source, AudioSource::Import);
    assert_eq!(prepared.audio.layout, AudioLayout::Mixed);
    assert_eq!(prepared.pcm.channels, 1);
    assert_eq!(
        hypr_audio_utils::source_from_path(&prepared.path)
            .unwrap()
            .collect::<Vec<_>>(),
        vec![(0.6_f32 + 0.3) / 2.0, 0.3]
    );
    assert!(
        PreparedAudio::from_path(
            file.path(),
            SessionAudio {
                source: AudioSource::Recording,
                layout: AudioLayout::MicSystem,
            },
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!(std::fs::read(file.path()).unwrap(), original);
}

#[test]
fn frame_reader_rejects_nonfinite_and_incomplete_interleaved_frames() {
    for samples in [vec![0.2], vec![0.2, f32::NAN], vec![f32::INFINITY, 0.0]] {
        assert!(read_frame(&mut samples.into_iter(), &mut [0.0; 2]).is_err());
    }
    assert!(!read_frame(&mut std::iter::empty(), &mut [0.0; 2]).unwrap());
    let mut frame = [0.0; 2];
    assert!(read_frame(&mut [0.25, -0.5].into_iter(), &mut frame).unwrap());
    assert_eq!(frame, [0.25, -0.5]);
}

#[test]
fn response_stamp_preserves_metadata_and_projects_repaired_audio_to_existing_mixed_domain() {
    let file = fixture(&[[0.2, 0.2], [0.4, 0.4]]);
    let prepared = PreparedAudio::from_path(
        file.path(),
        SessionAudio {
            source: AudioSource::Recording,
            layout: AudioLayout::MicSystem,
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut response:batch::Response=serde_json::from_value(serde_json::json!({
        "metadata":{"future":{"keep":true}},
        "results":{"channels":[{"alternatives":[{"transcript":"Voice","confidence":1.0,
            "words":[{"word":"Voice","start":0.0,"end":0.5,"confidence":1.0,"channel":0,"speaker":1,"punctuated_word":null}]
        }]}]}
    })).unwrap();
    prepared.stamp_response(&mut response);
    assert_eq!(
        response.metadata["future"],
        serde_json::json!({"keep":true})
    );
    assert_eq!(
        response.metadata["session_audio"],
        serde_json::json!({"source":"recording","layout":"mixed"})
    );
    assert_eq!(
        response.results.channels[0].alternatives[0].words[0].channel,
        2
    );
    let (words, hints) = crate::transcript::words_and_hints_from_batch_response(&response);
    assert_eq!(words[0].channel, 2.0);
    assert_eq!(
        words[0].metadata.as_ref().unwrap()["capture_source"],
        "recording"
    );
    assert_eq!(hints[0].speaker_index, 1);
}

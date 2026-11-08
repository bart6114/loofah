use super::*;
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use hypr_fs_format::{AudioLayout, AudioSource, SessionAudio};
use hypr_transcribe_whisper_local::resources::{
    Limits, ModelSpec, ResourceObserver, ResourcePhase,
};
use owhisper_interface::batch_stream::BatchStreamEvent;

const PRINCIPAL: &str = "00000000-0000-0000-0000-000000000000";
const MODEL_SHA256: &str = "c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9";

struct SourceOwner {
    directory: Option<tempfile::TempDir>,
    file: Option<File>,
    released: Arc<AtomicBool>,
    model: ResourceObserver,
}
impl Drop for SourceOwner {
    fn drop(&mut self) {
        assert_eq!(
            self.model.snapshot().batch_groups,
            0,
            "source released before native batch joined"
        );
        drop(self.file.take());
        drop(self.directory.take());
        self.released.store(true, Ordering::Release);
    }
}

struct Runtime {
    _source: SourceOwner,
    cancelled: Arc<AtomicBool>,
    saw_segment: Arc<AtomicBool>,
    active_at_cancel: Arc<AtomicBool>,
    cancel_after_segment: bool,
    model: ResourceObserver,
}
impl BatchRuntime for Runtime {
    fn emit(&self, event: BatchEvent) {
        if let BatchEvent::BatchResponseStreamed {
            event: BatchStreamEvent::Segment { .. },
            ..
        } = event
        {
            self.saw_segment.store(true, Ordering::Release);
            if self.cancel_after_segment && !self.cancelled.swap(true, Ordering::AcqRel) {
                self.active_at_cancel
                    .store(self.model.snapshot().batch_groups == 1, Ordering::Release);
            }
        }
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

fn configured_path(name: &str) -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os(name).unwrap_or_else(|| panic!("set {name} to an explicit fixture path")),
    );
    assert!(path.is_absolute(), "{name} must be absolute");
    path.canonicalize().unwrap()
}

fn dual_source(input: &Path, seconds: usize) -> (tempfile::TempDir, PathBuf) {
    let decoded = hypr_audio_utils::PcmFile::prepare(input, || false).unwrap();
    let pcm = &decoded.descriptor;
    assert!(
        pcm.frames >= 16_000 * 16,
        "fixture needs at least 16 seconds of speech"
    );
    let speech = pcm.reader().unwrap().channel(0, 0..16_000 * 16).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("retained-source.wav");
    let mut writer = hound::WavWriter::create(&path, hypr_audio_utils::pcm_spec(2)).unwrap();
    for frame in 0..16_000 * seconds {
        writer.write_sample(speech[frame % speech.len()]).unwrap();
        writer
            .write_sample(if frame < 16_000 {
                0.0
            } else {
                speech[(frame - 16_000) % speech.len()]
            })
            .unwrap();
    }
    writer.finalize().unwrap();
    (directory, path)
}

fn params(path: &Path) -> BatchParams {
    BatchParams {
        audio: SessionAudio {
            source: AudioSource::Recording,
            layout: AudioLayout::MicSystem,
        },
        session_id: "owned-native-fixture".into(),
        provider: super::super::BatchProvider::WhisperLocal,
        file_path: path.to_string_lossy().into_owned(),
        model: Some("QuantizedBase".into()),
        base_url: String::new(),
        api_key: String::new(),
        languages: vec!["en".parse().unwrap()],
        keywords: vec!["Loofah".into()],
        num_speakers: None,
        min_speakers: None,
        max_speakers: None,
    }
}

async fn phase(observer: &mut ResourceObserver, expected: ResourcePhase) {
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let actual = observer.snapshot().phase;
            if actual == expected {
                break;
            }
            assert!(
                !matches!(actual, ResourcePhase::Failed(_)),
                "native resources failed: {actual:?}"
            );
            assert!(
                observer.changed().await.is_some(),
                "native owner disappeared"
            );
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit LOOFAH_WHISPER_MODEL and LOOFAH_WHISPER_AUDIO; real owned Native batch (LOOFAH_WHISPER_CPU=1 allowed)"]
async fn configured_owned_native_whisper_maps_dual_channels_and_drains_cancelled_sources_and_model()
{
    let model_path = configured_path("LOOFAH_WHISPER_MODEL");
    let audio_path = configured_path("LOOFAH_WHISPER_AUDIO");
    assert_eq!(
        std::fs::metadata(&model_path).unwrap().len(),
        81_768_585,
        "use the verified base-q8_0 fixture"
    );
    let model_owner = Arc::new(());
    let weak_model = Arc::downgrade(&model_owner);
    let resources = Arc::new(
        WhisperResources::start_device_owned(
            ModelSpec {
                absolute_path: model_path,
                sha256: MODEL_SHA256.into(),
                size_bytes: 81_768_585,
            },
            Limits::default(),
            Box::new(model_owner),
        )
        .unwrap(),
    );
    let mut observer = resources.observer();
    phase(&mut observer, ResourcePhase::Ready).await;
    assert!(weak_model.upgrade().is_some());

    for cancel in [false, true] {
        let seconds = if cancel { 120 } else { 16 };
        let source_path = audio_path.clone();
        let (directory, path) =
            tokio::task::spawn_blocking(move || dual_source(&source_path, seconds))
                .await
                .unwrap();
        let released = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let saw_segment = Arc::new(AtomicBool::new(false));
        let active_at_cancel = Arc::new(AtomicBool::new(false));
        let runtime = Arc::new(Runtime {
            _source: SourceOwner {
                file: Some(File::open(&path).unwrap()),
                directory: Some(directory),
                released: released.clone(),
                model: observer.clone(),
            },
            cancelled: cancelled.clone(),
            saw_segment: saw_segment.clone(),
            active_at_cancel: active_at_cancel.clone(),
            cancel_after_segment: cancel,
            model: observer.clone(),
        });
        let run = params(&path);
        let result = tokio::time::timeout(
            Duration::from_secs(180),
            run_owned_whisper_batch(runtime, run, resources.clone()),
        )
        .await
        .unwrap();
        assert!(released.load(Ordering::Acquire));
        assert!(
            !path.exists(),
            "source directory remained after owned completion"
        );
        assert_eq!(observer.snapshot().batch_groups, 0);
        assert_eq!(observer.snapshot().phase, ResourcePhase::Ready);
        assert!(
            weak_model.upgrade().is_some(),
            "model released before host shutdown"
        );
        if cancel {
            assert!(
                saw_segment.load(Ordering::Acquire),
                "cancellation did not follow real native inference"
            );
            assert!(
                active_at_cancel.load(Ordering::Acquire),
                "inference already completed before cancellation"
            );
            assert!(cancelled.load(Ordering::Acquire));
            assert!(result.is_err());
            assert!(
                resources.try_batch().is_ok(),
                "capacity was not released after actual join"
            );
        } else {
            let output = result.unwrap();
            assert_eq!(output.response.results.channels.len(), 2);
            assert_eq!(
                output.response.metadata["session_audio"],
                serde_json::to_value(params(&path).audio).unwrap()
            );
            let mut id = 0;
            let mut next_id = || {
                id += 1;
                format!("word-{id}")
            };
            let transcript = crate::transcript::transcript_from_batch_response(
                &output.response,
                crate::transcript::BatchTranscriptMeta {
                    session_id: "owned-native-fixture",
                    user_id: PRINCIPAL,
                    provider: "whispercpp",
                    created_at: "2026-11-08T00:00:00.000Z".into(),
                    started_at_ms: 1_700_000_000_000.0,
                    memo_md: "Preserved note".into(),
                },
                &mut next_id,
            )
            .expect("configured fixture must contain speech");
            assert_eq!(transcript.user_id, PRINCIPAL);
            assert_eq!(transcript.memo_md, "Preserved note");
            assert_eq!(transcript.started_at, 1_700_000_000_000.0);
            for channel in [0.0, 1.0] {
                assert!(transcript.words.iter().any(|word| word.channel == channel));
            }
            assert!(transcript.words.iter().all(|word| {
                word.id.is_some()
                    && word.start_ms.is_finite()
                    && word.end_ms.is_finite()
                    && word.start_ms >= 0.0
                    && word.end_ms >= word.start_ms
                    && word.end_ms <= seconds as f64 * 1000.0
                    && word
                        .metadata
                        .as_ref()
                        .is_some_and(|metadata| metadata["capture_source"] == "recording")
            }));
            let mut ids = transcript
                .words
                .iter()
                .map(|word| word.id.as_ref().unwrap())
                .collect::<Vec<_>>();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), transcript.words.len());
        }
    }
    let mut stopped = resources.shutdown();
    phase(&mut stopped, ResourcePhase::Stopped).await;
    assert!(
        weak_model.upgrade().is_none(),
        "model admission survived completed native teardown"
    );
}

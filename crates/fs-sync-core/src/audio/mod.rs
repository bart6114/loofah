use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::error::AudioImportError;
use crate::runtime::{AudioImportEvent, AudioImportRuntime};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

const AUDIO_FORMATS: [&str; 3] = ["audio.mp3", "audio.wav", "audio.ogg"];
const AUDIO_ARTIFACTS: [&str; 9] = [
    "audio.mp3",
    "audio.wav",
    "audio.ogg",
    "audio.mp3.tmp",
    "audio.wav.tmp",
    "audio_mic.wav",
    "audio_spk.wav",
    PEAKS_FILE,
    PEAKS_TMP_FILE,
];

const PEAKS_FILE: &str = "audio.peaks.json";
const PEAKS_TMP_FILE: &str = "audio.peaks.json.tmp";
const PEAKS_VERSION: u32 = 1;
/// Enough resolution for the widest plausible player (the renderer draws one bar per ~5px),
/// while keeping the cache file and the IPC payload a few tens of KB.
const MAX_PEAK_BUCKETS: usize = 1500;
/// Frames per accumulation window before downsampling (20ms at the source rate). Bounds the
/// in-memory window vectors to ~50 entries per second of audio regardless of sample rate.
const PEAK_WINDOWS_PER_SECOND: usize = 50;

#[derive(Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioSourceMetadata {
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioFileMetadata {
    pub filename: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Per-channel waveform peaks precomputed for the audio player. Rendering from these skips
/// pulling the whole recording into the webview and decoding it there (seconds for an
/// hour-long file) -- WaveSurfer accepts pre-decoded peaks plus a duration and renders
/// immediately, while playback itself still streams through the media element.
#[derive(Clone, Debug, PartialEq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioPeaks {
    pub duration_sec: f64,
    pub channels: Vec<Vec<f32>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeaksCacheFile {
    version: u32,
    source_size_bytes: u64,
    source_modified_ms: u64,
    duration_sec: f64,
    channels: Vec<Vec<f32>>,
}

pub fn exists(session_dir: &Path) -> std::io::Result<bool> {
    AUDIO_FORMATS
        .iter()
        .map(|format| session_dir.join(format))
        .try_fold(false, |acc, path| {
            std::fs::exists(&path).map(|exists| acc || exists)
        })
}

pub fn delete(session_dir: &Path) -> std::io::Result<bool> {
    delete_with(session_dir, |path| std::fs::remove_file(path))
}

pub fn path(session_dir: &Path) -> Option<PathBuf> {
    AUDIO_FORMATS
        .iter()
        .map(|format| session_dir.join(format))
        .find(|path| path.exists())
}

pub fn metadata(session_dir: &Path) -> std::io::Result<Option<AudioFileMetadata>> {
    let Some(path) = path(session_dir) else {
        return Ok(None);
    };
    let Some(filename) = path.file_name().and_then(|filename| filename.to_str()) else {
        return Ok(None);
    };

    let content_type = match filename {
        "audio.mp3" => "audio/mpeg",
        "audio.wav" => "audio/wav",
        "audio.ogg" => "audio/ogg",
        _ => return Ok(None),
    };

    let mut file = std::fs::File::open(&path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size_bytes = 0_u64;

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
        size_bytes = size_bytes
            .checked_add(bytes_read as u64)
            .ok_or_else(|| std::io::Error::other("audio_file_too_large"))?;
    }

    let sha256 = hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(&mut output, "{byte:02x}").unwrap();
            output
        });

    Ok(Some(AudioFileMetadata {
        filename: filename.to_string(),
        content_type: content_type.to_string(),
        size_bytes,
        sha256,
    }))
}

/// Returns waveform peaks for the session's recording, computing and caching them on first
/// request. The cache lives beside the recording (`audio.peaks.json`) and is keyed on the
/// source file's size + mtime, so a replaced recording recomputes. `None` means "no peaks
/// available" (no recording, or one we can't decode) -- the player falls back to decoding
/// in the webview.
pub fn peaks(session_dir: &Path) -> std::io::Result<Option<AudioPeaks>> {
    let Some(audio_path) = path(session_dir) else {
        return Ok(None);
    };
    let audio_metadata = std::fs::metadata(&audio_path)?;
    let source_size_bytes = audio_metadata.len();
    let source_modified_ms = audio_metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);

    let cache_path = session_dir.join(PEAKS_FILE);
    if let Some(cached) = read_peaks_cache(&cache_path, source_size_bytes, source_modified_ms) {
        return Ok(Some(cached));
    }

    let Ok(source) = hypr_audio_utils::source_from_path(&audio_path) else {
        return Ok(None);
    };
    let Some(peaks) = compute_peaks(source) else {
        return Ok(None);
    };

    // Best-effort cache: failing to persist must not fail the request.
    let cache = PeaksCacheFile {
        version: PEAKS_VERSION,
        source_size_bytes,
        source_modified_ms,
        duration_sec: peaks.duration_sec,
        channels: peaks.channels.clone(),
    };
    if let Ok(json) = serde_json::to_vec(&cache) {
        let tmp_path = session_dir.join(PEAKS_TMP_FILE);
        let _ =
            std::fs::write(&tmp_path, &json).and_then(|_| std::fs::rename(&tmp_path, &cache_path));
    }

    Ok(Some(peaks))
}

fn read_peaks_cache(
    cache_path: &Path,
    source_size_bytes: u64,
    source_modified_ms: u64,
) -> Option<AudioPeaks> {
    let bytes = std::fs::read(cache_path).ok()?;
    let cache: PeaksCacheFile = serde_json::from_slice(&bytes).ok()?;
    (cache.version == PEAKS_VERSION
        && cache.source_size_bytes == source_size_bytes
        && cache.source_modified_ms == source_modified_ms)
        .then_some(AudioPeaks {
            duration_sec: cache.duration_sec,
            channels: cache.channels,
        })
}

fn compute_peaks<S: hypr_audio_utils::Source>(source: S) -> Option<AudioPeaks> {
    let sample_rate = u32::from(source.sample_rate()) as usize;
    let channel_count = usize::from(u16::from(source.channels()));
    let window_frames = (sample_rate / PEAK_WINDOWS_PER_SECOND).max(1);

    let mut windows: Vec<Vec<f32>> = vec![Vec::new(); channel_count];
    let mut current = vec![0.0_f32; channel_count];
    let mut channel_index = 0usize;
    let mut frames_in_window = 0usize;
    let mut total_frames = 0u64;

    for sample in source {
        let peak = &mut current[channel_index];
        *peak = peak.max(sample.abs());

        channel_index += 1;
        if channel_index < channel_count {
            continue;
        }
        channel_index = 0;
        total_frames += 1;
        frames_in_window += 1;
        if frames_in_window == window_frames {
            for (channel, peak) in windows.iter_mut().zip(current.iter_mut()) {
                channel.push(*peak);
                *peak = 0.0;
            }
            frames_in_window = 0;
        }
    }
    if frames_in_window > 0 {
        for (channel, peak) in windows.iter_mut().zip(current.iter()) {
            channel.push(*peak);
        }
    }

    if total_frames == 0 {
        return None;
    }

    Some(AudioPeaks {
        duration_sec: total_frames as f64 / sample_rate as f64,
        channels: windows
            .iter()
            .map(|channel| downsample_max(channel, MAX_PEAK_BUCKETS))
            .collect(),
    })
}

fn downsample_max(windows: &[f32], buckets: usize) -> Vec<f32> {
    if windows.len() <= buckets {
        return windows.iter().copied().map(round_peak).collect();
    }
    (0..buckets)
        .map(|bucket| {
            let start = bucket * windows.len() / buckets;
            let end = ((bucket + 1) * windows.len() / buckets).max(start + 1);
            round_peak(
                windows[start..end]
                    .iter()
                    .fold(0.0_f32, |acc, v| acc.max(*v)),
            )
        })
        .collect()
}

/// Three decimal places is below what the ~24px-tall waveform can display, and it keeps the
/// serialized cache compact.
fn round_peak(value: f32) -> f32 {
    (value * 1000.0).round() / 1000.0
}

fn delete_with(
    session_dir: &Path,
    mut remove_file: impl FnMut(&Path) -> std::io::Result<()>,
) -> std::io::Result<bool> {
    let primary_path = path(session_dir);

    for artifact in AUDIO_ARTIFACTS {
        let artifact_path = session_dir.join(artifact);
        if primary_path.as_ref() == Some(&artifact_path) {
            continue;
        }
        if std::fs::exists(&artifact_path)? {
            remove_file(&artifact_path)?;
        }
    }

    let Some(primary_path) = primary_path else {
        return Ok(false);
    };
    remove_file(&primary_path)?;
    Ok(true)
}

/// Deletes expired audio artifacts from orphaned session directories and returns
/// the affected session ids. A directory holding a parseable `_meta.json` is a
/// session identified by `_meta.json.id`; one holding an unreadable meta is left
/// untouched; session content is never recursed into. Meta-less uuid-named
/// directories are legacy recorder-fallback orphans, identified by basename.
pub fn source_metadata(source_path: &Path) -> std::io::Result<AudioSourceMetadata> {
    use hypr_audio_utils::Source;

    let metadata = std::fs::metadata(source_path)?;
    let created_at = metadata.created().ok().map(system_time_to_iso);
    let modified_at = metadata.modified().ok().map(system_time_to_iso);
    let duration_ms = hypr_audio_utils::source_from_path(source_path)
        .ok()
        .and_then(|source| source.total_duration())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());

    Ok(AudioSourceMetadata {
        created_at,
        modified_at,
        duration_ms,
    })
}

pub fn import_to_session(
    runtime: &dyn AudioImportRuntime,
    session_id: &str,
    session_dir: &Path,
    source_path: &Path,
) -> Result<PathBuf, AudioImportError> {
    runtime.emit(AudioImportEvent::Started {
        session_id: session_id.to_string(),
    });

    std::fs::create_dir_all(session_dir)?;

    let target_path = session_dir.join("audio.mp3");
    let tmp_path = session_dir.join("audio.mp3.tmp");

    let on_progress = {
        let session_id = session_id.to_string();
        let mut last_emitted: f64 = 0.0;
        let mut last_time = std::time::Instant::now();
        move |percentage: f64| {
            let now = std::time::Instant::now();
            if (percentage - last_emitted) >= 0.01
                || now.duration_since(last_time).as_millis() >= 100
            {
                runtime.emit(AudioImportEvent::Progress {
                    session_id: session_id.clone(),
                    percentage,
                });
                last_emitted = percentage;
                last_time = now;
            }
        }
    };

    let result = hypr_audio_norm::normalize_file(
        source_path,
        &tmp_path,
        &target_path,
        None,
        Some(on_progress),
    )
    .map(|_| ());
    match result {
        Ok(()) => {
            let final_path = target_path;
            runtime.emit(AudioImportEvent::Completed {
                session_id: session_id.to_string(),
            });
            Ok(final_path.to_path_buf())
        }
        Err(error) => {
            if tmp_path.exists() {
                let _ = std::fs::remove_file(&tmp_path);
            }
            runtime.emit(AudioImportEvent::Failed {
                session_id: session_id.to_string(),
                error: error.to_string(),
            });
            Err(error.into())
        }
    }
}

pub fn import_audio(
    source_path: &Path,
    tmp_path: &Path,
    target_path: &Path,
) -> Result<PathBuf, hypr_audio_norm::Error> {
    hypr_audio_norm::normalize_file(source_path, tmp_path, target_path, None, None::<fn(f64)>)
}

fn system_time_to_iso(time: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use hypr_audio_utils::Source;

    const MIN_MP3_BYTES: u64 = 1024;
    fn write_audio(path: &Path) {
        std::fs::write(path, b"audio").unwrap();
    }

    #[test]
    fn test_delete_removes_audio_artifacts() {
        let temp = TempDir::new().unwrap();
        let session_dir = temp.path();
        for artifact in AUDIO_ARTIFACTS {
            write_audio(&session_dir.join(artifact));
        }
        let note_path = session_dir.join("note.md");
        std::fs::write(&note_path, b"keep").unwrap();

        assert!(delete(session_dir).unwrap());

        for artifact in AUDIO_ARTIFACTS {
            assert!(!session_dir.join(artifact).exists());
        }
        assert!(note_path.exists());
    }

    #[test]
    fn metadata_hashes_the_selected_final_audio_file() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("audio.mp3"), b"abc").unwrap();

        let metadata = metadata(temp.path()).unwrap().unwrap();

        assert_eq!(
            metadata,
            AudioFileMetadata {
                filename: "audio.mp3".to_string(),
                content_type: "audio/mpeg".to_string(),
                size_bytes: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                    .to_string(),
            }
        );
    }

    #[test]
    fn metadata_falls_back_to_supported_final_audio_formats() {
        let cases = [("audio.wav", "audio/wav"), ("audio.ogg", "audio/ogg")];

        for (filename, content_type) in cases {
            let temp = TempDir::new().unwrap();
            std::fs::write(temp.path().join(filename), b"audio").unwrap();

            let metadata = metadata(temp.path()).unwrap().unwrap();

            assert_eq!(metadata.filename, filename);
            assert_eq!(metadata.content_type, content_type);
        }
    }

    #[test]
    fn metadata_ignores_audio_artifacts() {
        let temp = TempDir::new().unwrap();
        for artifact in [
            "audio.mp3.tmp",
            "audio.wav.tmp",
            "audio_mic.wav",
            "audio_spk.wav",
        ] {
            write_audio(&temp.path().join(artifact));
        }

        assert_eq!(metadata(temp.path()).unwrap(), None);
    }

    #[test]
    fn delete_without_final_audio_returns_false() {
        let temp = TempDir::new().unwrap();
        write_audio(&temp.path().join("audio.mp3.tmp"));

        assert!(!delete(temp.path()).unwrap());
        assert!(!temp.path().join("audio.mp3.tmp").exists());
    }

    #[test]
    fn delete_preserves_primary_audio_when_auxiliary_deletion_fails() {
        let temp = TempDir::new().unwrap();
        let primary_path = temp.path().join("audio.mp3");
        let auxiliary_path = temp.path().join("audio.mp3.tmp");
        write_audio(&primary_path);
        write_audio(&auxiliary_path);

        let result = delete_with(temp.path(), |path| {
            if path == auxiliary_path {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "blocked auxiliary deletion",
                ));
            }
            std::fs::remove_file(path)
        });

        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(primary_path.exists());
    }

    #[test]
    fn peaks_returns_none_without_a_recording() {
        let temp = TempDir::new().unwrap();
        assert_eq!(peaks(temp.path()).unwrap(), None);
    }

    #[test]
    fn peaks_computes_caches_and_invalidates_when_the_recording_changes() {
        let temp = TempDir::new().unwrap();
        let session_dir = temp.path();
        let audio_path = session_dir.join("audio.wav");
        std::fs::copy(hypr_data::english_1::AUDIO_PATH, &audio_path).unwrap();

        let computed = peaks(session_dir).unwrap().unwrap();
        assert!(computed.duration_sec > 0.0);
        assert!(!computed.channels.is_empty());
        for channel in &computed.channels {
            assert!(!channel.is_empty() && channel.len() <= MAX_PEAK_BUCKETS);
        }
        assert!(
            computed.channels[0].iter().any(|peak| *peak > 0.0),
            "speech audio must produce non-silent peaks"
        );
        assert!(session_dir.join(PEAKS_FILE).exists());

        // A matching cache is served back without re-decoding: tampering with the cached
        // duration proves the next call reads the file rather than recomputing.
        let cache_bytes = std::fs::read(session_dir.join(PEAKS_FILE)).unwrap();
        let mut cache: serde_json::Value = serde_json::from_slice(&cache_bytes).unwrap();
        cache["durationSec"] = serde_json::json!(12345.0);
        std::fs::write(
            session_dir.join(PEAKS_FILE),
            serde_json::to_vec(&cache).unwrap(),
        )
        .unwrap();
        let cached = peaks(session_dir).unwrap().unwrap();
        assert_eq!(cached.duration_sec, 12345.0);

        // Changing the recording (size differs) must bypass the stale cache and recompute.
        let mut audio_bytes = std::fs::read(&audio_path).unwrap();
        audio_bytes.push(0);
        std::fs::write(&audio_path, &audio_bytes).unwrap();
        let recomputed = peaks(session_dir).unwrap().unwrap();
        assert_eq!(recomputed.duration_sec, computed.duration_sec);
    }

    macro_rules! test_import_audio {
        ($($name:ident: $path:expr),* $(,)?) => {
            $(
                #[test]
                fn $name() {
                    let source_path = std::path::Path::new($path);
                    let temp = TempDir::new().unwrap();
                    let tmp_path = temp.path().join("tmp.mp3");
                    let target_path = temp.path().join("target.mp3");

                    let result = import_audio(source_path, &tmp_path, &target_path);
                    assert!(result.is_ok(), "import failed: {:?}", result.err());
                    assert!(target_path.exists());

                    let size = std::fs::metadata(&target_path).unwrap().len();
                    assert!(
                        size > MIN_MP3_BYTES,
                        "Output too small ({size} bytes), likely empty audio"
                    );
                }
            )*
        };
    }

    test_import_audio! {
        test_import_wav: hypr_data::english_1::AUDIO_PATH,
        test_import_mp3: hypr_data::english_1::AUDIO_MP3_PATH,
        test_import_mp4: hypr_data::english_1::AUDIO_MP4_PATH,
        test_import_m4a: hypr_data::english_1::AUDIO_M4A_PATH,
        test_import_ogg: hypr_data::english_1::AUDIO_OGG_PATH,
        test_import_flac: hypr_data::english_1::AUDIO_FLAC_PATH,
        test_import_aac: hypr_data::english_1::AUDIO_AAC_PATH,
        test_import_aiff: hypr_data::english_1::AUDIO_AIFF_PATH,
        test_import_caf: hypr_data::english_1::AUDIO_CAF_PATH,
    }

    #[test]
    fn test_import_stereo_mp3() {
        let source_path = std::path::Path::new(hypr_data::english_10::AUDIO_MP3_PATH);
        let temp = TempDir::new().unwrap();
        let tmp_path = temp.path().join("tmp.mp3");
        let target_path = temp.path().join("target.mp3");

        let result = import_audio(source_path, &tmp_path, &target_path);
        assert!(result.is_ok(), "import failed: {:?}", result.err());
        assert!(target_path.exists());

        let size = std::fs::metadata(&target_path).unwrap().len();
        assert!(
            size > MIN_MP3_BYTES,
            "Output too small ({size} bytes), likely empty audio"
        );

        let decoder = hypr_audio_utils::source_from_path(&target_path).unwrap();
        let channels: u16 = decoder.channels().into();
        assert_eq!(channels, 2, "stereo input should produce stereo output");
    }

    #[test]
    fn test_import_problem_m4a() {
        let source = match std::env::var("PROBLEM_M4A") {
            Ok(p) => PathBuf::from(p),
            Err(_) => return,
        };
        let temp = TempDir::new().unwrap();
        let result = import_audio(
            &source,
            &temp.path().join("tmp.mp3"),
            &temp.path().join("out.mp3"),
        );
        assert!(result.is_ok(), "import failed: {:?}", result.err());
    }

    #[test]
    fn test_import_problem2_m4a() {
        let source = match std::env::var("PROBLEM2_M4A") {
            Ok(p) => PathBuf::from(p),
            Err(_) => return,
        };
        let temp = TempDir::new().unwrap();
        let result = import_audio(
            &source,
            &temp.path().join("tmp.mp3"),
            &temp.path().join("out.mp3"),
        );
        assert!(result.is_ok(), "import failed: {:?}", result.err());
    }
}

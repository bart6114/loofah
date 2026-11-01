use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

use serde::{Deserialize, Serialize};

static CAPTURE: OnceLock<Mutex<Option<Capture>>> = OnceLock::new();
static SAMPLES: AtomicU64 = AtomicU64::new(0);
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static FAILED: AtomicBool = AtomicBool::new(false);
static STOPPING_SESSION: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static AUDIO_SENDER: arc_swap::ArcSwapOption<mpsc::SyncSender<Vec<f32>>> =
    arc_swap::ArcSwapOption::const_empty();
pub static FOREGROUND: AtomicBool = AtomicBool::new(true);

struct Capture {
    session_id: String,
    sender: mpsc::SyncSender<Vec<f32>>,
    thread: std::thread::JoinHandle<Result<PathBuf, String>>,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    session_id: String,
}

#[derive(Clone, Serialize)]
pub struct RecordingStatus {
    pub session_id: Option<String>,
    pub stopping: bool,
    pub elapsed_seconds: f64,
    pub interrupted: bool,
    pub error: Option<String>,
}

fn capture() -> &'static Mutex<Option<Capture>> {
    CAPTURE.get_or_init(|| Mutex::new(None))
}

fn stopping_session() -> &'static Mutex<Option<String>> {
    STOPPING_SESSION.get_or_init(|| Mutex::new(None))
}

pub fn status() -> RecordingStatus {
    let stopping = stopping_session().lock().unwrap().clone();
    RecordingStatus {
        session_id: capture()
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| c.session_id.clone())
            .or_else(|| stopping.clone()),
        stopping: stopping.is_some(),
        elapsed_seconds: SAMPLES.load(Ordering::Relaxed) as f64 / 16000.0,
        interrupted: INTERRUPTED.load(Ordering::Relaxed),
        error: FAILED.load(Ordering::Relaxed).then(|| "Audio capture was interrupted by a storage or buffer error. Stop to preserve the recording.".into()),
    }
}

pub fn set_stopping(session_id: Option<&str>) {
    *stopping_session().lock().unwrap() = session_id.map(str::to_owned);
}

pub fn is_stopping() -> bool {
    stopping_session().lock().unwrap().is_some()
}

pub fn is_active() -> bool {
    capture().lock().unwrap().is_some()
}

pub fn begin(vault: &Path, state_dir: &Path, session_id: &str) -> Result<(), String> {
    let relative =
        hypr_vault_read::paths::validated_session_dir(session_id).map_err(|e| e.to_string())?;
    let directory = vault.join(relative);
    if !directory.join("_meta.json").is_file() {
        return Err("Session no longer exists".into());
    }
    let mut active = capture().lock().unwrap();
    if active.is_some() {
        return Err("A recording is already running".into());
    }
    if ["audio.wav", "audio.mp3", "audio.ogg", "audio.wav.tmp"]
        .iter()
        .any(|n| directory.join(n).exists())
    {
        return Err("Create a new session to record; existing audio is preserved".into());
    }
    let pending = directory.join("audio.wav.tmp");
    let final_path = directory.join("audio.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let journal = state_dir.join("recording.json");
    if journal.exists() {
        return Err("An earlier recording needs recovery before starting another".into());
    }
    super::persist::write_json(
        &journal,
        &Journal {
            session_id: session_id.into(),
        },
    )?;
    let mut writer = match hound::WavWriter::create(&pending, spec) {
        Ok(writer) => writer,
        Err(error) => {
            let _ = std::fs::remove_file(&journal);
            return Err(error.to_string());
        }
    };
    let (sender, receiver) = mpsc::sync_channel::<Vec<f32>>(64);
    SAMPLES.store(0, Ordering::Relaxed);
    FAILED.store(false, Ordering::Relaxed);
    INTERRUPTED.store(false, Ordering::Relaxed);
    let thread = std::thread::spawn(move || {
        let result = (|| {
            let mut since_flush = 0;
            while let Ok(samples) = receiver.recv() {
                for &sample in &samples {
                    writer
                        .write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                        .map_err(|e| e.to_string())?;
                }
                SAMPLES.fetch_add(samples.len() as u64, Ordering::Relaxed);
                since_flush += samples.len();
                if since_flush >= 16000 {
                    writer.flush().map_err(|e| e.to_string())?;
                    std::fs::File::open(&pending)
                        .and_then(|f| f.sync_all())
                        .map_err(|e| e.to_string())?;
                    since_flush = 0;
                }
            }
            writer.finalize().map_err(|e| e.to_string())?;
            std::fs::File::open(&pending)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
            std::fs::rename(&pending, &final_path).map_err(|e| e.to_string())?;
            std::fs::remove_file(journal).map_err(|e| e.to_string())?;
            Ok(final_path)
        })();
        if result.is_err() {
            FAILED.store(true, Ordering::Relaxed);
        }
        result
    });
    AUDIO_SENDER.store(Some(std::sync::Arc::new(sender.clone())));
    *active = Some(Capture {
        session_id: session_id.into(),
        sender,
        thread,
    });
    Ok(())
}

pub async fn finish() -> Result<(String, PathBuf), String> {
    let active = capture()
        .lock()
        .unwrap()
        .take()
        .ok_or("No recording is running")?;
    AUDIO_SENDER.store(None);
    drop(active.sender);
    let path = tokio::task::spawn_blocking(move || {
        active
            .thread
            .join()
            .map_err(|_| "Audio writer stopped unexpectedly".to_string())?
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok((active.session_id, path))
}

/// Called on the native audio queue; never waits for disk or the UI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loofah_mobile_audio(samples: *const f32, count: usize) -> bool {
    if samples.is_null() || count == 0 || count > 32000 || FAILED.load(Ordering::Relaxed) {
        return false;
    }
    let guard = AUDIO_SENDER.load();
    if let Some(sender) = guard.as_ref() {
        let data = unsafe { std::slice::from_raw_parts(samples, count) };
        if sender.try_send(data.to_vec()).is_err() {
            FAILED.store(true, Ordering::Relaxed);
            return false;
        }
        return true;
    }
    false
}

pub fn native_event(event: &str) {
    match event {
        "background" => FOREGROUND.store(false, Ordering::Relaxed),
        "foreground" => FOREGROUND.store(true, Ordering::Relaxed),
        "recording_interrupted" => INTERRUPTED.store(true, Ordering::Relaxed),
        "recording_resumed" => INTERRUPTED.store(false, Ordering::Relaxed),
        "recording_error" => {
            FAILED.store(true, Ordering::Relaxed);
            INTERRUPTED.store(true, Ordering::Relaxed);
        }
        _ => {}
    }
}

pub fn recover(vault: &Path, state_dir: &Path) -> Result<Option<String>, String> {
    let journal = state_dir.join("recording.json");
    if !journal.exists() {
        return Ok(None);
    }
    let entry: Journal =
        serde_json::from_slice(&std::fs::read(&journal).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let directory = vault.join(
        hypr_vault_read::paths::validated_session_dir(&entry.session_id)
            .map_err(|e| e.to_string())?,
    );
    let pending = directory.join("audio.wav.tmp");
    let final_path = directory.join("audio.wav");
    if pending.exists() {
        if final_path.exists() {
            return Err(
                "Both recovered and finalized audio exist; recovery requires inspection".into(),
            );
        }
        repair_pcm_wav(&pending)?;
        std::fs::rename(pending, &final_path).map_err(|e| e.to_string())?;
    }
    let recovered = final_path.is_file();
    std::fs::remove_file(journal).map_err(|e| e.to_string())?;
    Ok(recovered.then_some(entry.session_id))
}

fn repair_pcm_wav(path: &Path) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if !(44..=u32::MAX as u64).contains(&len) {
        return Err("Invalid interrupted recording length".into());
    }
    let mut header = [0u8; 12];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return Err("Invalid recording header".into());
    }
    let data_offset = loop {
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk).map_err(|e| e.to_string())?;
        let offset = file.stream_position().map_err(|e| e.to_string())?;
        if &chunk[..4] == b"data" {
            break offset;
        }
        let size = u32::from_le_bytes(chunk[4..].try_into().unwrap()) as u64;
        if offset + size + (size % 2) >= len {
            return Err("No audio data chunk".into());
        }
        file.seek(SeekFrom::Current((size + size % 2) as i64))
            .map_err(|e| e.to_string())?;
    };
    let bytes = (len - data_offset) / 2 * 2;
    file.set_len(data_offset + bytes)
        .map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(4)).map_err(|e| e.to_string())?;
    file.write_all(&((data_offset + bytes - 8) as u32).to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(data_offset - 4))
        .map_err(|e| e.to_string())?;
    file.write_all(&(bytes as u32).to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopping_retains_the_session_for_activity_after_capture_finishes() {
        set_stopping(Some("saved-session"));
        let during_stop = status();
        assert_eq!(during_stop.session_id.as_deref(), Some("saved-session"));
        assert!(during_stop.stopping);
        assert!(is_stopping());
        set_stopping(None);
        assert!(!status().stopping);
    }

    fn recovery_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        let state = temp.path().join("device");
        let session = vault.join("sessions/test-recording");
        std::fs::create_dir_all(&session).unwrap();
        super::super::persist::write_json(
            &state.join("recording.json"),
            &Journal {
                session_id: "test-recording".into(),
            },
        )
        .unwrap();
        (temp, vault, state, session)
    }

    fn write_audio(path: &Path) {
        let mut writer = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        writer.write_sample(123_i16).unwrap();
        writer.finalize().unwrap();
    }

    #[test]
    fn recovery_finalizes_pending_audio_and_is_idempotent() {
        let (_temp, vault, state, session) = recovery_fixture();
        write_audio(&session.join("audio.wav.tmp"));
        assert_eq!(
            recover(&vault, &state).unwrap().as_deref(),
            Some("test-recording")
        );
        let mut audio = hound::WavReader::open(session.join("audio.wav")).unwrap();
        assert_eq!(audio.samples::<i16>().next().unwrap().unwrap(), 123);
        assert!(!session.join("audio.wav.tmp").exists());
        assert!(!state.join("recording.json").exists());
        assert_eq!(recover(&vault, &state).unwrap(), None);
    }

    #[test]
    fn recovery_handles_crash_between_rename_and_journal_removal() {
        let (_temp, vault, state, session) = recovery_fixture();
        write_audio(&session.join("audio.wav"));
        let before = std::fs::read(session.join("audio.wav")).unwrap();
        assert_eq!(
            recover(&vault, &state).unwrap().as_deref(),
            Some("test-recording")
        );
        assert_eq!(std::fs::read(session.join("audio.wav")).unwrap(), before);
        assert!(!state.join("recording.json").exists());
    }

    #[test]
    fn recovery_before_audio_creation_does_not_queue_nonexistent_recording() {
        let (_temp, vault, state, _session) = recovery_fixture();
        assert_eq!(recover(&vault, &state).unwrap(), None);
        assert!(!state.join("recording.json").exists());
    }

    #[test]
    fn recovery_never_overwrites_existing_audio() {
        let (_temp, vault, state, session) = recovery_fixture();
        write_audio(&session.join("audio.wav.tmp"));
        std::fs::write(session.join("audio.wav"), b"existing recording").unwrap();
        let pending = std::fs::read(session.join("audio.wav.tmp")).unwrap();
        assert!(recover(&vault, &state).is_err());
        assert_eq!(
            std::fs::read(session.join("audio.wav")).unwrap(),
            b"existing recording"
        );
        assert_eq!(
            std::fs::read(session.join("audio.wav.tmp")).unwrap(),
            pending
        );
        assert!(state.join("recording.json").exists());
    }

    #[test]
    fn corrupt_audio_and_journal_are_preserved_for_recovery() {
        let (_temp, vault, state, session) = recovery_fixture();
        std::fs::write(session.join("audio.wav.tmp"), b"incomplete header").unwrap();
        assert!(recover(&vault, &state).is_err());
        assert_eq!(
            std::fs::read(session.join("audio.wav.tmp")).unwrap(),
            b"incomplete header"
        );
        assert!(state.join("recording.json").exists());
        std::fs::write(state.join("recording.json"), b"{broken").unwrap();
        assert!(recover(&vault, &state).is_err());
        assert_eq!(
            std::fs::read(state.join("recording.json")).unwrap(),
            b"{broken"
        );
    }

    #[test]
    fn recovery_discards_only_incomplete_last_sample() {
        use std::io::Write;
        let (_temp, vault, state, session) = recovery_fixture();
        let path = session.join("audio.wav.tmp");
        write_audio(&path);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[0xff])
            .unwrap();
        recover(&vault, &state).unwrap();
        let mut reader = hound::WavReader::open(session.join("audio.wav")).unwrap();
        assert_eq!(
            reader
                .samples::<i16>()
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            vec![123]
        );
    }

    #[test]
    fn recovers_samples_written_after_last_header_checkpoint() {
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("audio.wav.tmp");
        let mut writer = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        writer.write_sample(123_i16).unwrap();
        writer.finalize().unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&456_i16.to_le_bytes())
            .unwrap();
        repair_pcm_wav(&path).unwrap();
        let mut reader = hound::WavReader::open(path).unwrap();
        assert_eq!(
            reader
                .samples::<i16>()
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            vec![123, 456]
        );
    }
}

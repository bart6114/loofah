use super::*;
use hypr_loofah_protocol::{AudioPeaks, RecordingWaveform};
use tokio_util::sync::CancellationToken;

const TIME_LIMIT: Duration = Duration::from_secs(30);
const DURATION_LIMIT: u64 = 12 * 60 * 60;

impl MediaReader {
    pub async fn waveform(
        &self,
        cancel: CancellationToken,
    ) -> Result<RecordingWaveform, ApplicationError> {
        self.entry.touch()?;
        if !matches!(self.entry.source, MediaSource::Recording) {
            return Err(error(
                ErrorCode::Validation,
                "Waveforms are only available for session recordings",
            ));
        }
        check_cancel(&cancel)?;
        let reader = self.clone();
        // Keep the cache cell and admission alive until cancelled blocking work actually exits.
        tokio::spawn(async move {
            let peaks = reader
                .recording
                .peaks
                .get_or_try_init(|| async {
                    let permit = reader.waveforms.clone().try_acquire_owned().map_err(|_| {
                        let mut cause = error(
                            ErrorCode::ResourceLimit,
                            "Waveform workers are busy; retry shortly",
                        );
                        cause.retryable = true;
                        cause
                    })?;
                    let worker = reader.clone();
                    let cancel = cancel.clone();
                    tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        worker.compute_waveform(cancel).map(Arc::new)
                    })
                    .await
                    .map_err(|_| error(ErrorCode::Internal, "Waveform worker stopped"))?
                })
                .await?;
            let peaks = peaks.clone();
            tokio::task::spawn_blocking(move || {
                check_cancel(&cancel)?;
                reader.entry.touch()?;
                reader.verify(&reader.recording.file.lock().unwrap())?;
                Ok(RecordingWaveform {
                    recording_sha256: reader.metadata().sha256.clone(),
                    peaks: peaks.as_ref().clone(),
                })
            })
            .await
            .map_err(|_| error(ErrorCode::Internal, "Waveform confirmation stopped"))?
        })
        .await
        .map_err(|_| error(ErrorCode::Internal, "Waveform request stopped"))?
    }

    fn compute_waveform(&self, cancel: CancellationToken) -> Result<AudioPeaks, ApplicationError> {
        let deadline = Instant::now() + TIME_LIMIT;
        let fault = Arc::new(Mutex::new(None));
        let input = VerifiedInput {
            reader: self.clone(),
            position: 0,
            start: 0,
            buffer: Vec::new(),
            cancel: cancel.clone(),
            deadline,
            fault: fault.clone(),
        };
        let peaks = hypr_fs_sync_core::audio::peaks_from_reader_with_control(
            input,
            self.metadata().size_bytes,
            |frames, sample_rate, channels| {
                check_work(self, &cancel, deadline)?;
                if channels > 8
                    || sample_rate > 384_000
                    || frames > DURATION_LIMIT * u64::from(sample_rate)
                {
                    return Err(processing_limit());
                }
                Ok(())
            },
        );
        if let Some(cause) = fault.lock().unwrap().take() {
            return Err(cause);
        }
        let peaks = peaks?.ok_or_else(|| {
            error(
                ErrorCode::Validation,
                "Cannot decode this recording's waveform",
            )
        })?;
        if !peaks.duration_sec.is_finite()
            || peaks
                .channels
                .iter()
                .flatten()
                .any(|value| !value.is_finite())
        {
            return Err(error(
                ErrorCode::Validation,
                "Recording waveform contains invalid samples",
            ));
        }
        check_work(self, &cancel, deadline)?;
        self.verify(&self.recording.file.lock().unwrap())?;
        Ok(peaks)
    }
}

struct VerifiedInput {
    reader: MediaReader,
    position: u64,
    start: u64,
    buffer: Vec<u8>,
    cancel: CancellationToken,
    deadline: Instant,
    fault: Arc<Mutex<Option<ApplicationError>>>,
}

impl VerifiedInput {
    fn remember(&self, cause: ApplicationError) -> std::io::Error {
        let message = cause.message.clone();
        let mut fault = self.fault.lock().unwrap();
        if fault.is_none() {
            *fault = Some(cause);
        }
        std::io::Error::other(message)
    }
    fn check(&self) -> std::io::Result<()> {
        check_work(&self.reader, &self.cancel, self.deadline).map_err(|cause| self.remember(cause))
    }
}

impl Read for VerifiedInput {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        self.check()?;
        if output.is_empty() || self.position >= self.reader.metadata().size_bytes {
            return Ok(0);
        }
        if self.position < self.start || self.position >= self.start + self.buffer.len() as u64 {
            self.start = self.position / HASH_CHUNK_SIZE as u64 * HASH_CHUNK_SIZE as u64;
            self.buffer = self
                .reader
                .read_chunk_blocking(self.start, HASH_CHUNK_SIZE)
                .map_err(|cause| self.remember(cause))?;
        }
        let offset = (self.position - self.start) as usize;
        let count = output.len().min(self.buffer.len() - offset);
        output[..count].copy_from_slice(&self.buffer[offset..offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for VerifiedInput {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        self.check()?;
        let next = match from {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => {
                i128::from(self.reader.metadata().size_bytes) + i128::from(offset)
            }
        };
        self.position = u64::try_from(next).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid recording seek")
        })?;
        Ok(self.position)
    }
}

fn check_work(
    reader: &MediaReader,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<(), ApplicationError> {
    check_cancel(cancel)?;
    if Instant::now() >= deadline {
        return Err(processing_limit());
    }
    reader.entry.touch()
}
fn check_cancel(cancel: &CancellationToken) -> Result<(), ApplicationError> {
    if cancel.is_cancelled() {
        return Err(error(ErrorCode::Cancelled, "Waveform request cancelled"));
    }
    Ok(())
}
fn processing_limit() -> ApplicationError {
    error(
        ErrorCode::ResourceLimit,
        "Waveform exceeds the current processing limit",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(channels: u16) -> Vec<u8> {
        let frames = 80_000_u32;
        let size = frames * u32::from(channels) * 2;
        let rate = 16_000_u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&size.to_le_bytes());
        for index in 0..frames {
            for _ in 0..channels {
                bytes.extend_from_slice(&(((index % 1000) as i16 - 500) * 50).to_le_bytes());
            }
        }
        bytes
    }

    async fn prepared(channels: u16) -> (tempfile::TempDir, MediaLeases, MediaReader, Vec<u8>) {
        let (root, context) = super::super::tests::fixture().await;
        let bytes = wav(channels);
        std::fs::write(root.path().join("sessions/note/audio.wav"), &bytes).unwrap();
        let leases = MediaLeases::default();
        let id = uuid::Uuid::new_v4().to_string();
        leases.open(context, "alice", &id, "note").await.unwrap();
        let reader = leases.reader("alice", "vault", &id).unwrap();
        (root, leases, reader, bytes)
    }

    #[tokio::test]
    async fn waveform_uses_verified_pinned_bytes_and_its_own_cache_without_touching_vault_files() {
        let (root, leases, reader, bytes) = prepared(2).await;
        let directory = root.path().join("sessions/note");
        std::fs::write(directory.join("audio.peaks.json"), "Untrusted old cache").unwrap();
        std::fs::write(directory.join("unknown.txt"), "User attachment").unwrap();
        let expected = hypr_fs_sync_core::audio::peaks_from_reader(
            std::io::Cursor::new(bytes.clone()),
            bytes.len() as u64,
        )
        .unwrap();
        std::fs::rename(directory.join("audio.wav"), directory.join("retired.wav")).unwrap();
        std::fs::write(directory.join("audio.wav"), b"Replacement recording").unwrap();
        let value = reader.waveform(CancellationToken::new()).await.unwrap();
        assert_eq!(value.peaks, expected);
        assert_eq!(value.peaks.duration_sec, 5.0);
        assert_eq!(value.recording_sha256, reader.metadata().sha256);
        leases.0.waveforms.close();
        assert_eq!(
            reader.waveform(CancellationToken::new()).await.unwrap(),
            value
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("audio.peaks.json")).unwrap(),
            "Untrusted old cache"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("unknown.txt")).unwrap(),
            "User attachment"
        );
        assert_eq!(std::fs::read(directory.join("retired.wav")).unwrap(), bytes);
        assert_eq!(
            std::fs::read(directory.join("audio.wav")).unwrap(),
            b"Replacement recording"
        );
    }

    #[tokio::test]
    async fn waveform_never_caches_a_partial_decode_after_a_timestamp_preserving_in_place_change() {
        let (root, _leases, reader, mut bytes) = prepared(1).await;
        let path = root.path().join("sessions/note/audio.wav");
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        bytes[100_000] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let cause = reader.waveform(CancellationToken::new()).await.unwrap_err();
        assert_eq!(cause.code, ErrorCode::Unavailable);
        assert!(reader.recording.peaks.get().is_none());
        assert!(reader.entry.closed.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn waveform_refuses_unsupported_channel_cost_and_releases_worker_admission() {
        let (_root, leases, reader, _) = prepared(9).await;
        let cause = reader.waveform(CancellationToken::new()).await.unwrap_err();
        assert_eq!(cause.code, ErrorCode::ResourceLimit);
        assert!(!cause.retryable);
        assert_eq!(leases.0.waveforms.available_permits(), 2);
        assert!(reader.recording.peaks.get().is_none());
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            reader.waveform(cancel).await.unwrap_err().code,
            ErrorCode::Cancelled
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_waveform_retains_admission_and_file_ownership_until_blocking_input_finishes()
    {
        let (_root, leases, reader, _) = prepared(1).await;
        let recording = reader.recording.clone();
        let (held, received) = std::sync::mpsc::channel();
        let release = Arc::new(std::sync::Barrier::new(2));
        let blocking = release.clone();
        let holder = std::thread::spawn(move || {
            let _held = recording.file.lock().unwrap();
            held.send(()).unwrap();
            blocking.wait();
        });
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        let cancel = CancellationToken::new();
        let worker = reader.clone();
        let stop = cancel.clone();
        let task = tokio::spawn(async move { worker.waveform(stop).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while reader.recording.reads_started.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let entry = Arc::downgrade(&reader.entry);
        leases.close_all();
        drop(reader);
        assert_eq!(leases.0.waveforms.available_permits(), 1);
        assert!(entry.upgrade().is_some());
        release.wait();
        holder.join().unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while entry.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(leases.0.waveforms.available_permits(), 2);
    }
}

use std::io::{self, Cursor, Read};

use hypr_capture_store::capture_spool::{CaptureChunk, CaptureSnapshot, CaptureWriter};
use hypr_loofah_protocol::{ApplicationError, CaptureChunkDescriptor, CaptureSeal, ErrorCode};
use sha2::{Digest, Sha256};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use super::{hash, rejected};

pub(super) struct VerifiedWave {
    pub snapshot: CaptureSnapshot,
    pub artifact_sha256: String,
    pub artifact_size_bytes: u64,
    header: Vec<u8>,
}

pub(super) async fn verify(
    writer: &CaptureWriter,
    expected: &CaptureSeal,
) -> Result<VerifiedWave, ApplicationError> {
    let snapshot = writer.snapshot().await?;
    if snapshot.seal.as_ref() != Some(expected)
        || snapshot.frames == 0
        || expected.capture_fingerprint != snapshot.capture_fingerprint
    {
        return Err(rejected(
            ErrorCode::Conflict,
            "A nonempty sealed capture is required",
        ));
    }
    let header = header(&snapshot)?;
    let mut source = Sha256::new();
    let mut encoded = Sha256::new();
    encoded.update(&header);
    let mut prefix = Prefix::new(&snapshot);
    for sequence in 0..snapshot.chunk_count {
        let chunk = writer.read_chunk(sequence).await?;
        prefix.admit(&chunk, &snapshot)?;
        source.update(&chunk.bytes);
        encoded.update(&chunk.bytes);
    }
    prefix.finish(expected)?;
    if digest(source) != expected.source_sha256 {
        return Err(rejected(
            ErrorCode::Conflict,
            "Capture source checksum does not match its seal",
        ));
    }
    Ok(VerifiedWave {
        artifact_size_bytes: snapshot.pcm_bytes + header.len() as u64,
        artifact_sha256: digest(encoded),
        snapshot,
        header,
    })
}

impl VerifiedWave {
    pub fn reader(
        &self,
        context: std::sync::Arc<crate::VaultContext>,
        writer: CaptureWriter,
    ) -> (WaveReader, JoinHandle<Result<(), ApplicationError>>) {
        let (send, receive) = mpsc::channel(1);
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let snapshot = self.snapshot.clone();
        let producer = tokio::spawn(async move {
            let _context = context;
            let result = async {
                let mut prefix = Prefix::new(&snapshot);
                for sequence in 0..snapshot.chunk_count {
                    if stopping.is_cancelled() {
                        return Ok(());
                    }
                    // Await real storage I/O even after the reader is dropped; its lease stays owned.
                    let chunk = writer.read_chunk(sequence).await?;
                    prefix.admit(&chunk, &snapshot)?;
                    tokio::select! {
                        _ = stopping.cancelled() => return Ok(()),
                        sent = send.send(Ok(chunk.bytes)) => if sent.is_err() { return Ok(()); },
                    }
                }
                let seal = snapshot
                    .seal
                    .as_ref()
                    .ok_or_else(|| rejected(ErrorCode::Conflict, "Capture seal is missing"))?;
                prefix.finish(seal)
            }
            .await;
            if let Err(cause) = &result {
                let _ = send
                    .send(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        cause.message.clone(),
                    )))
                    .await;
            }
            result
        });
        (
            WaveReader {
                current: Cursor::new(self.header.clone()),
                receive,
                stop,
                finished: false,
            },
            producer,
        )
    }
}

pub(super) struct WaveReader {
    current: Cursor<Vec<u8>>,
    receive: mpsc::Receiver<io::Result<Vec<u8>>>,
    stop: CancellationToken,
    finished: bool,
}
impl Read for WaveReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.current.read(output)?;
            if count != 0 || self.finished {
                return Ok(count);
            }
            match self.receive.blocking_recv() {
                Some(Ok(bytes)) => self.current = Cursor::new(bytes),
                Some(Err(cause)) => {
                    self.finished = true;
                    return Err(cause);
                }
                None => {
                    self.finished = true;
                    return Ok(0);
                }
            }
        }
    }
}
impl Drop for WaveReader {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

struct Prefix {
    sequence: u64,
    frames: u64,
    bytes: u64,
    chain: String,
}
impl Prefix {
    fn new(snapshot: &CaptureSnapshot) -> Self {
        Self {
            sequence: 0,
            frames: 0,
            bytes: 0,
            chain: hypr_loofah_protocol::capture_chain_seed(&snapshot.capture_fingerprint),
        }
    }
    fn admit(
        &mut self,
        chunk: &CaptureChunk,
        snapshot: &CaptureSnapshot,
    ) -> Result<(), ApplicationError> {
        let descriptor: &CaptureChunkDescriptor = &chunk.descriptor;
        if descriptor.sequence != self.sequence
            || descriptor.first_frame != self.frames
            || descriptor.pcm_offset != self.bytes
            || descriptor.previous_digest != self.chain
            || descriptor.capture_fingerprint != snapshot.capture_fingerprint
            || descriptor.bytes as usize != chunk.bytes.len()
            || u64::from(descriptor.frames)
                .checked_mul(snapshot.format.channels.len() as u64)
                .and_then(|samples| samples.checked_mul(4))
                != Some(chunk.bytes.len() as u64)
            || descriptor.frames == 0
            || descriptor.sha256 != hash(&chunk.bytes)
            || descriptor.digest != hypr_loofah_protocol::capture_chunk_digest(descriptor)
        {
            return Err(rejected(
                ErrorCode::Conflict,
                "Capture bytes or frame boundaries changed",
            ));
        }
        self.sequence = self.sequence.checked_add(1).ok_or_else(limit)?;
        self.frames = self
            .frames
            .checked_add(u64::from(descriptor.frames))
            .ok_or_else(limit)?;
        self.bytes = self
            .bytes
            .checked_add(chunk.bytes.len() as u64)
            .ok_or_else(limit)?;
        self.chain = descriptor.digest.clone();
        Ok(())
    }
    fn finish(&self, seal: &CaptureSeal) -> Result<(), ApplicationError> {
        if self.sequence != seal.chunk_count
            || self.frames != seal.frames
            || self.bytes != seal.pcm_bytes
            || self.chain != seal.chain_digest
        {
            return Err(rejected(
                ErrorCode::Conflict,
                "Capture prefix changed after sealing",
            ));
        }
        Ok(())
    }
}

fn header(snapshot: &CaptureSnapshot) -> Result<Vec<u8>, ApplicationError> {
    hypr_loofah_protocol::validate_capture_format(&snapshot.format)
        .map_err(|message| rejected(ErrorCode::Validation, message))?;
    if snapshot.capture_fingerprint
        != hypr_loofah_protocol::capture_fingerprint(&snapshot.identity, &snapshot.format)
        || snapshot
            .frames
            .checked_mul(snapshot.format.channels.len() as u64)
            .and_then(|samples| samples.checked_mul(4))
            != Some(snapshot.pcm_bytes)
    {
        return Err(rejected(
            ErrorCode::Conflict,
            "Capture format or identity changed",
        ));
    }
    let spec = hound::WavSpec {
        channels: snapshot.format.channels.len() as u16,
        sample_rate: snapshot.format.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut cursor = Cursor::new(Vec::new());
    hound::WavWriter::new(&mut cursor, spec)
        .and_then(hound::WavWriter::finalize)
        .map_err(|_| {
            rejected(
                ErrorCode::Internal,
                "Capture WAV header could not be encoded",
            )
        })?;
    let mut header = cursor.into_inner();
    if header.len() < 44
        || header.len() > 128
        || &header[..4] != b"RIFF"
        || &header[8..12] != b"WAVE"
        || &header[header.len() - 8..header.len() - 4] != b"data"
    {
        return Err(rejected(
            ErrorCode::Internal,
            "Capture WAV encoder header is unsupported",
        ));
    }
    let file_size = snapshot
        .pcm_bytes
        .checked_add(header.len() as u64)
        .ok_or_else(limit)?;
    if file_size > u64::from(u32::MAX) {
        return Err(limit());
    }
    // Reuse the pinned encoder's format header and fill the two known RIFF lengths.
    // The sealed frame count avoids buffering a recording or emitting a seekable temporary WAV.
    header[4..8].copy_from_slice(&((file_size - 8) as u32).to_le_bytes());
    let data_length = header.len() - 4;
    header[data_length..].copy_from_slice(&(snapshot.pcm_bytes as u32).to_le_bytes());
    Ok(header)
}
fn digest(value: Sha256) -> String {
    value
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn limit() -> ApplicationError {
    rejected(
        ErrorCode::ResourceLimit,
        "Capture exceeds the supported WAV length",
    )
}

#[cfg(test)]
#[path = "wav_tests.rs"]
mod tests;

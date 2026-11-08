use crate::capture_spool::{CaptureChunk, CaptureSnapshot};
use hypr_loofah_protocol::{
    ApplicationError, CaptureChunkDescriptor, CaptureSeal, ErrorCode, MutationOutcome,
};
use sha2::{Digest, Sha256};
use std::io::Cursor;

pub struct WavePrefix {
    sequence: u64,
    frames: u64,
    bytes: u64,
    chain: String,
}
impl WavePrefix {
    pub fn new(snapshot: &CaptureSnapshot) -> Self {
        Self {
            sequence: 0,
            frames: 0,
            bytes: 0,
            chain: hypr_loofah_protocol::capture_chain_seed(&snapshot.capture_fingerprint),
        }
    }
    pub fn admit(
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
    pub fn finish_snapshot(&self, snapshot: &CaptureSnapshot) -> Result<(), ApplicationError> {
        if self.sequence != snapshot.chunk_count
            || self.frames != snapshot.frames
            || self.bytes != snapshot.pcm_bytes
            || self.chain != snapshot.chain_digest
        {
            return Err(rejected(
                ErrorCode::Conflict,
                "Capture prefix changed during recovery",
            ));
        }
        Ok(())
    }
    pub fn finish(&self, seal: &CaptureSeal) -> Result<(), ApplicationError> {
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

pub fn header(snapshot: &CaptureSnapshot) -> Result<Vec<u8>, ApplicationError> {
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
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn rejected(code: ErrorCode, message: &str) -> ApplicationError {
    ApplicationError {
        mutation_outcome: Some(MutationOutcome::NotCommitted),
        ..crate::failure(code, message)
    }
}
fn limit() -> ApplicationError {
    rejected(
        ErrorCode::ResourceLimit,
        "Capture exceeds the supported WAV length",
    )
}

mod export;
pub use export::{WaveExportReceipt, export_wave};

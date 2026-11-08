use std::io::{self, Cursor, Read};

use hypr_capture_store::{
    capture_spool::{CaptureSnapshot, CaptureWriter},
    wave::{WavePrefix as Prefix, header},
};
use hypr_loofah_protocol::{ApplicationError, CaptureSeal, ErrorCode};
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

fn digest(value: Sha256) -> String {
    value
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
#[path = "wav_tests.rs"]
mod tests;

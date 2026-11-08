use std::{fs::File, path::Path, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::{Blob, Error, Result, blob::FRAME_BYTES};
use hypr_loofah_protocol::{LocalCliOperationKind, LocalCliProgress};

pub(crate) const MAJOR: u32 = 1;
const CONTROL_BYTES: usize = 16 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BlobDescriptor {
    pub bytes: u64,
    pub sha256: String,
}
impl BlobDescriptor {
    fn validate(&self) -> Result<()> {
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::protocol("Invalid transfer checksum"));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Control {
    Hello {
        major: u32,
        vault_key: String,
    },
    Welcome {
        major: u32,
        vault_key: String,
        epoch: String,
        operations: Vec<LocalCliOperationKind>,
    },
    Request {
        request_id: String,
        epoch: String,
        payload: BlobDescriptor,
        blob: Option<BlobDescriptor>,
    },
    Accepted {
        request_id: String,
        epoch: String,
    },
    Keepalive {
        request_id: String,
        epoch: String,
    },
    Progress {
        request_id: String,
        epoch: String,
        progress: LocalCliProgress,
    },
    Response {
        request_id: String,
        epoch: String,
        payload: BlobDescriptor,
        blob: Option<BlobDescriptor>,
    },
    Rejected {
        message: String,
    },
}

pub(crate) async fn send_control<W: AsyncWrite + Unpin>(io: &mut W, value: &Control) -> Result<()> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| Error::protocol("Invalid local control message"))?;
    if bytes.is_empty() || bytes.len() > CONTROL_BYTES {
        return Err(Error::protocol("Local control message too large"));
    }
    send_frame(io, &bytes).await
}

pub(crate) async fn read_control<R: AsyncRead + Unpin>(io: &mut R) -> Result<Control> {
    let bytes = read_frame(io, CONTROL_BYTES).await?;
    decode_control(&bytes)
}

fn decode_control(bytes: &[u8]) -> Result<Control> {
    #[derive(Deserialize)]
    struct Header {
        kind: String,
    }
    let header: Header = decode(bytes)?;
    if header.kind != "progress" {
        return decode(bytes);
    }
    // Serde's internally tagged enum buffers its content and cannot deserialize
    // u128. Read progress directly from JSON to preserve the full elapsed range.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Progress {
        kind: ProgressKind,
        request_id: String,
        epoch: String,
        progress: LocalCliProgress,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum ProgressKind {
        Progress,
    }
    let Progress {
        kind: ProgressKind::Progress,
        request_id,
        epoch,
        progress,
    } = decode(bytes)?;
    Ok(Control::Progress {
        request_id,
        epoch,
        progress,
    })
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|_| Error::protocol("Invalid local control message"))
}

pub(crate) async fn send_blob<W: AsyncWrite + Unpin>(io: &mut W, blob: &Blob) -> Result<()> {
    send_blob_cancellable(io, blob, &CancellationToken::new()).await
}

pub(crate) async fn send_blob_cancellable<W: AsyncWrite + Unpin>(
    io: &mut W,
    blob: &Blob,
    stop: &CancellationToken,
) -> Result<()> {
    let mut offset = 0;
    while offset < blob.len() {
        if stop.is_cancelled() {
            return Err(Error::Stopped);
        }
        let chunk = blob.chunk(offset).await?;
        if chunk.is_empty() {
            return Err(Error::protocol("Incomplete local transfer"));
        }
        tokio::select! {_=stop.cancelled()=>return Err(Error::Stopped),result=send_frame(io,&chunk)=>result?}
        offset += chunk.len() as u64;
    }
    Ok(())
}

pub(crate) async fn receive_blob<R: AsyncRead + Unpin>(
    io: &mut R,
    descriptor: BlobDescriptor,
    directory: &Path,
    owner: Option<Arc<File>>,
) -> Result<Blob> {
    receive_blob_cancellable(io, descriptor, directory, owner, &CancellationToken::new()).await
}

pub(crate) async fn receive_blob_cancellable<R: AsyncRead + Unpin>(
    io: &mut R,
    descriptor: BlobDescriptor,
    directory: &Path,
    owner: Option<Arc<File>>,
    stop: &CancellationToken,
) -> Result<Blob> {
    descriptor.validate()?;
    let blob = Blob::empty(directory, descriptor.clone(), owner).await?;
    let mut received = 0u64;
    let mut digest = Sha256::new();
    while received < descriptor.bytes {
        let chunk = tokio::select! {_=stop.cancelled()=>return Err(Error::Stopped),result=read_frame(io,FRAME_BYTES)=>result?};
        received = received
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| Error::protocol("Local transfer length overflow"))?;
        if received > descriptor.bytes {
            return Err(Error::protocol("Unexpected local transfer bytes"));
        }
        digest.update(&chunk);
        // Actual disk writes finish before observing stop; abandoning their waiter would lie about drain.
        blob.append(chunk).await?;
    }
    if crate::hex(&digest.finalize()) != descriptor.sha256 {
        return Err(Error::protocol("Local transfer checksum mismatch"));
    }
    Ok(blob)
}

async fn send_frame<W: AsyncWrite + Unpin>(io: &mut W, bytes: &[u8]) -> Result<()> {
    tokio::time::timeout(IO_TIMEOUT, async {
        io.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
        io.write_all(bytes).await?;
        io.flush().await
    })
    .await
    .map_err(|_| Error::Unavailable("Local owner transfer timed out".into()))??;
    Ok(())
}

async fn read_frame<R: AsyncRead + Unpin>(io: &mut R, limit: usize) -> Result<Vec<u8>> {
    tokio::time::timeout(IO_TIMEOUT, async {
        let size = io.read_u32().await? as usize;
        if size == 0 || size > limit {
            return Err(Error::protocol("Invalid local frame length"));
        }
        let mut bytes = vec![0; size];
        io.read_exact(&mut bytes).await?;
        Ok(bytes)
    })
    .await
    .map_err(|_| Error::Unavailable("Local owner transfer timed out".into()))?
}

pub(crate) fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn framed_progress_roundtrips_integer_elapsed_time_without_narrowing() {
        for elapsed_ms in [0, 1, u32::MAX as u128 + 1, u64::MAX as u128, u128::MAX] {
            let expected = Control::Progress {
                request_id: uuid::Uuid::new_v4().to_string(),
                epoch: uuid::Uuid::new_v4().to_string(),
                progress: LocalCliProgress {
                    schema_version: "2".into(),
                    phase: "indexing".into(),
                    processed: 1,
                    total: Some(2),
                    added: 1,
                    updated: 0,
                    deleted: 0,
                    elapsed_ms,
                },
            };
            let (mut sender, mut receiver) = tokio::io::duplex(CONTROL_BYTES);
            send_control(&mut sender, &expected).await.unwrap();
            let actual = read_control(&mut receiver).await.unwrap();
            assert_eq!(
                serde_json::to_string(&actual).unwrap(),
                serde_json::to_string(&expected).unwrap()
            );
        }
    }

    #[test]
    fn progress_rejects_negative_fractional_and_unknown_fields() {
        let base = r#"{"kind":"progress","request_id":"request","epoch":"epoch","progress":{"schema_version":"2","phase":"indexing","processed":1,"total":2,"added":1,"updated":0,"deleted":0,"elapsed_ms":0}}"#;
        for changed in [
            base.replace("\"elapsed_ms\":0", "\"elapsed_ms\":-1"),
            base.replace("\"elapsed_ms\":0", "\"elapsed_ms\":0.5"),
            base.replace("\"elapsed_ms\":0", "\"elapsed_ms\":0,\"future\":true"),
        ] {
            assert!(decode_control(changed.as_bytes()).is_err());
        }
    }
}

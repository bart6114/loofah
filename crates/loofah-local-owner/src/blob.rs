use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{Error, Result, wire::BlobDescriptor};

pub(crate) const FRAME_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct Blob {
    inner: Arc<Inner>,
    descriptor: BlobDescriptor,
}

struct Inner {
    file: Mutex<tempfile::NamedTempFile>,
    _owner: Option<Arc<File>>,
}

impl Blob {
    pub async fn copy_from(mut source: File, directory: &Path) -> Result<Self> {
        crate::validate_temporary_root(directory)?;
        let directory = directory.to_owned();
        blocking(move || {
            source.seek(SeekFrom::Start(0))?;
            let mut output = tempfile::NamedTempFile::new_in(directory)?;
            let mut digest = Sha256::new();
            let mut bytes = 0u64;
            let mut chunk = [0; FRAME_BYTES];
            loop {
                let count = source.read(&mut chunk)?;
                if count == 0 {
                    break;
                }
                output.write_all(&chunk[..count])?;
                digest.update(&chunk[..count]);
                bytes = bytes
                    .checked_add(count as u64)
                    .ok_or_else(|| Error::protocol("Blob length overflow"))?;
            }
            output.flush()?;
            Ok(Self::finished(
                output,
                bytes,
                crate::hex(&digest.finalize()),
                None,
            ))
        })
        .await
    }

    pub(crate) async fn encode<T: Serialize + Send + 'static>(
        value: T,
        directory: &Path,
        owner: Option<Arc<File>>,
    ) -> Result<Self> {
        let directory = directory.to_owned();
        blocking(move || {
            let mut output = tempfile::NamedTempFile::new_in(directory)?;
            let (bytes, sha256) = {
                let mut writer = HashWriter {
                    output: BufWriter::with_capacity(FRAME_BYTES, output.as_file_mut()),
                    digest: Sha256::new(),
                    bytes: 0,
                };
                serde_json::to_writer(&mut writer, &value)
                    .map_err(|_| Error::protocol("Cannot encode local operation"))?;
                writer.flush()?;
                let digest = std::mem::take(&mut writer.digest).finalize();
                (writer.bytes, crate::hex(&digest))
            };
            Ok(Self::finished(output, bytes, sha256, owner))
        })
        .await
    }

    pub async fn decode<T: DeserializeOwned + Send + 'static>(&self) -> Result<T> {
        let blob = self.clone();
        blocking(move || {
            let mut file = blob
                .inner
                .file
                .lock()
                .map_err(|_| Error::protocol("Blob file lock failed"))?;
            file.seek(SeekFrom::Start(0))?;
            let reader = BufReader::with_capacity(
                FRAME_BYTES,
                file.as_file_mut().take(blob.descriptor.bytes),
            );
            let mut decoder = serde_json::Deserializer::from_reader(reader);
            let value = T::deserialize(&mut decoder)
                .map_err(|_| Error::protocol("Invalid local operation payload"))?;
            decoder
                .end()
                .map_err(|_| Error::protocol("Trailing local operation bytes"))?;
            Ok(value)
        })
        .await
    }

    pub fn len(&self) -> u64 {
        self.descriptor.bytes
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub async fn with_file<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut File) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let blob = self.clone();
        blocking(move || {
            let mut file = blob
                .inner
                .file
                .lock()
                .map_err(|_| Error::protocol("Blob file lock failed"))?;
            file.seek(SeekFrom::Start(0))?;
            operation(file.as_file_mut())
        })
        .await
    }

    /// Host-owned temporary artifact. The returned path is valid only while this Blob is retained.
    pub fn trusted_path(&self) -> Result<PathBuf> {
        Ok(self
            .inner
            .file
            .lock()
            .map_err(|_| Error::protocol("Blob file lock failed"))?
            .path()
            .to_owned())
    }

    pub(crate) fn descriptor(&self) -> BlobDescriptor {
        self.descriptor.clone()
    }

    pub(crate) async fn empty(
        directory: &Path,
        descriptor: BlobDescriptor,
        owner: Option<Arc<File>>,
    ) -> Result<Self> {
        let directory = directory.to_owned();
        blocking(move || {
            Ok(Self {
                inner: Arc::new(Inner {
                    file: Mutex::new(tempfile::NamedTempFile::new_in(directory)?),
                    _owner: owner,
                }),
                descriptor,
            })
        })
        .await
    }

    pub(crate) async fn append(&self, chunk: Vec<u8>) -> Result<()> {
        let blob = self.clone();
        blocking(move || {
            let mut file = blob
                .inner
                .file
                .lock()
                .map_err(|_| Error::protocol("Blob file lock failed"))?;
            file.write_all(&chunk)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn chunk(&self, offset: u64) -> Result<Vec<u8>> {
        let blob = self.clone();
        blocking(move || {
            let count =
                usize::try_from(blob.len().saturating_sub(offset).min(FRAME_BYTES as u64)).unwrap();
            let mut file = blob
                .inner
                .file
                .lock()
                .map_err(|_| Error::protocol("Blob file lock failed"))?;
            file.seek(SeekFrom::Start(offset))?;
            let mut output = vec![0; count];
            file.read_exact(&mut output)?;
            Ok(output)
        })
        .await
    }

    fn finished(
        file: tempfile::NamedTempFile,
        bytes: u64,
        sha256: String,
        owner: Option<Arc<File>>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                file: Mutex::new(file),
                _owner: owner,
            }),
            descriptor: BlobDescriptor { bytes, sha256 },
        }
    }
}

async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| Error::protocol("Local transfer worker stopped"))?
}

struct HashWriter<'a> {
    output: BufWriter<&'a mut File>,
    digest: Sha256,
    bytes: u64,
}
impl Write for HashWriter<'_> {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        let count = self.output.write(input)?;
        self.digest.update(&input[..count]);
        self.bytes = self
            .bytes
            .checked_add(count as u64)
            .ok_or_else(|| std::io::Error::other("Blob length overflow"))?;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

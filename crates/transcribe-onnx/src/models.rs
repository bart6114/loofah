use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Model {
    Streaming,
    Batch,
    Diarizer,
}

struct Asset {
    name: &'static str,
    bytes: u64,
    sha256: &'static str,
    url: Option<&'static str>,
}

impl Model {
    pub fn id(self) -> &'static str {
        match self {
            Self::Streaming => "onnx-parakeet-streaming",
            Self::Batch => "onnx-parakeet-batch",
            Self::Diarizer => "onnx-reverb-titanet",
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            Self::Streaming => {
                "https://huggingface.co/altunenes/parakeet-rs/resolve/a61d2818df4659c956b9661a9447f46e98c15126/realtime_eou_120m-v1-onnx"
            }
            Self::Diarizer => {
                "https://huggingface.co/csukuangfj/sherpa-onnx-reverb-diarization-v1/resolve/d6a516efb21b1d22cb1c7baae704acacaa3ff71a"
            }
            Self::Batch => {
                "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce"
            }
        }
    }

    fn assets(self) -> &'static [Asset] {
        match self {
            Self::Streaming => &[
                Asset {
                    url: None,
                    name: "encoder.onnx",
                    bytes: 459341289,
                    sha256: "d472887cc38a784a5bfc21c2dbe247639edc3b3f9992388d8ceceaec07256b5b",
                },
                Asset {
                    url: None,
                    name: "decoder_joint.onnx",
                    bytes: 21347639,
                    sha256: "9d2553ac043c2fc5f69e970769b0fb8ab9103fbfdeb7d26a1ea9729d4bd2dddd",
                },
                Asset {
                    url: None,
                    name: "tokenizer.json",
                    bytes: 20053,
                    sha256: "f6b0ad8690559351fa478116fe0985a203b76f7c040f3a9381f485c99c0325f8",
                },
            ],
            Self::Diarizer => &[
                Asset {
                    name: "model.onnx",
                    bytes: 9512223,
                    sha256: "8249e2e323f8fb0566a387d94b40a73d0b75d54ee60d02dcd729a56a5ba8ecea",
                    url: None,
                },
                Asset {
                    name: "nemo_en_titanet_small.onnx",
                    bytes: 40257283,
                    sha256: "ad4a1802485d8b34c722d2a9d04249662f2ece5d28a7a039063ca22f515a789e",
                    url: Some(
                        "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_small.onnx",
                    ),
                },
            ],
            Self::Batch => &[
                Asset {
                    url: None,
                    name: "encoder-model.int8.onnx",
                    bytes: 652183999,
                    sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
                },
                Asset {
                    url: None,
                    name: "decoder_joint-model.int8.onnx",
                    bytes: 18202004,
                    sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
                },
                Asset {
                    url: None,
                    name: "vocab.txt",
                    bytes: 93939,
                    sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
                },
            ],
        }
    }

    pub fn size_bytes(self) -> u64 {
        self.assets().iter().map(|asset| asset.bytes).sum()
    }

    pub fn cache_dir(self) -> Result<PathBuf> {
        Ok(dirs::cache_dir()
            .context("User cache directory unavailable")?
            .join("loofah")
            .join("models")
            .join(self.id())
            .join(match self {
                Self::Streaming | Self::Diarizer => "fp32-v1",
                Self::Batch => "int8-v1",
            }))
    }

    pub fn verify(self) -> Result<PathBuf> {
        let dir = self.cache_dir()?;
        for asset in self.assets() {
            verify_asset(&dir.join(asset.name), asset)?;
        }
        Ok(dir)
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadState {
    pub status: String,
    pub current_file: Option<String>,
    pub progress_percent: Option<u8>,
    pub local_path: String,
    pub error: Option<String>,
}

struct Download {
    state: Arc<Mutex<DownloadState>>,
    cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

static DOWNLOADS: LazyLock<Mutex<HashMap<Model, Download>>> = LazyLock::new(Mutex::default);

// This marker is written only after all hashes pass. Inference verifies hashes again before load.
fn complete(model: Model, dir: &Path) -> bool {
    fs::read_to_string(dir.join("verified.json"))
        .ok()
        .as_deref()
        == Some(model.base_url())
        && model.assets().iter().all(|asset| {
            fs::metadata(dir.join(asset.name)).is_ok_and(|meta| meta.len() == asset.bytes)
        })
}

pub fn state(model: Model) -> Result<DownloadState> {
    let downloads = DOWNLOADS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(download) = downloads.get(&model) {
        return Ok(download
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone());
    }
    let dir = model.cache_dir()?;
    let ready = complete(model, &dir);
    Ok(DownloadState {
        status: if ready { "ready" } else { "notDownloaded" }.into(),
        progress_percent: ready.then_some(100),
        local_path: dir.to_string_lossy().into_owned(),
        ..Default::default()
    })
}

pub fn start(model: Model) -> Result<()> {
    let mut downloads = DOWNLOADS.lock().unwrap_or_else(|e| e.into_inner());
    if downloads
        .get(&model)
        .is_some_and(|d| d.thread.as_ref().is_some_and(|t| !t.is_finished()))
    {
        return Ok(());
    }
    let dir = model.cache_dir()?;
    let state = Arc::new(Mutex::new(DownloadState {
        status: "downloading".into(),
        progress_percent: Some(0),
        local_path: dir.to_string_lossy().into_owned(),
        ..Default::default()
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let thread = {
        let state = state.clone();
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                runtime.block_on(download(model, &dir, &cancel, |file, bytes| {
                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                    state.current_file = Some(file.into());
                    state.progress_percent =
                        Some(((bytes * 100 / model.size_bytes()).min(99)) as u8);
                }))
            })();
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            state.current_file = None;
            match result {
                Ok(()) => {
                    state.status = "ready".into();
                    state.progress_percent = Some(100);
                }
                Err(error) => {
                    state.status = "error".into();
                    state.error = Some(format!("{error:#}"));
                }
            }
        })
    };
    downloads.insert(
        model,
        Download {
            state,
            cancel,
            thread: Some(thread),
        },
    );
    Ok(())
}

pub fn reset(model: Model) {
    // Keep the registry locked until the writer exits so a new download cannot race deletion.
    let mut downloads = DOWNLOADS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut download) = downloads.remove(&model) {
        download.cancel.store(true, Ordering::Release);
        if let Some(thread) = download.thread.take() {
            let _ = thread.join();
        }
    }
}

fn verify_asset(path: &Path, asset: &Asset) -> Result<()> {
    let mut file =
        File::open(path).with_context(|| format!("Missing model file {}", asset.name))?;
    if file.metadata()?.len() != asset.bytes {
        bail!("Incorrect size for {}", asset.name);
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        != asset.sha256
    {
        bail!("Checksum mismatch for {}", asset.name);
    }
    Ok(())
}

async fn download(
    model: Model,
    dir: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&str, u64),
) -> Result<()> {
    fs::create_dir_all(dir)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(15))
        .build()?;
    let mut finished = 0;
    for asset in model.assets() {
        if cancel.load(Ordering::Acquire) {
            bail!("Download cancelled");
        }
        let path = dir.join(asset.name);
        if verify_asset(&path, asset).is_ok() {
            finished += asset.bytes;
            progress(asset.name, finished);
            continue;
        }
        let partial = dir.join(format!("{}.part", asset.name));
        let mut offset = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
        if offset >= asset.bytes {
            offset = 0;
        }
        let url = asset
            .url
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{}/{}", model.base_url(), asset.name));
        let mut request = client.get(url);
        if offset > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let mut response = request.send().await?.error_for_status()?;
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            offset = 0;
        }
        if offset > 0 {
            let expected = format!("bytes {offset}-");
            if !response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h.starts_with(&expected))
            {
                bail!("Invalid resume response for {}", asset.name);
            }
        }
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(offset == 0)
            .append(offset > 0)
            .open(&partial)?;
        let mut bytes = offset;
        loop {
            if cancel.load(Ordering::Acquire) {
                bail!("Download cancelled");
            }
            let Some(chunk) = response.chunk().await? else {
                break;
            };
            bytes += chunk.len() as u64;
            if bytes > asset.bytes {
                bail!("Oversized model file {}", asset.name);
            }
            file.write_all(&chunk)?;
            progress(asset.name, finished + bytes);
        }
        file.sync_all()?;
        drop(file);
        if let Err(error) = verify_asset(&partial, asset) {
            // A bad prefix cannot be repaired by another range request.
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
        if path.exists() {
            fs::remove_file(&path)?;
        }
        fs::rename(partial, path)?;
        finished += asset.bytes;
    }
    fs::write(dir.join("verified.json"), model.base_url())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corruption_and_truncation_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.onnx");
        let asset = Asset {
            name: "model.onnx",
            url: None,
            bytes: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        };
        fs::write(&path, b"abc").unwrap();
        verify_asset(&path, &asset).unwrap();
        fs::write(&path, b"abd").unwrap();
        assert!(verify_asset(&path, &asset).is_err());
        fs::write(&path, b"ab").unwrap();
        assert!(verify_asset(&path, &asset).is_err());
    }
}

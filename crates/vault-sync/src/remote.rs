use std::path::Path;
use std::time::{Duration, Instant};

use reqwest::{
    Client, Response,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    Error, Result, enrollment::hex, scheduler::MULTIPART_BYTES, snapshot::FileDigest,
    spool::UploadSpool,
};

#[derive(Clone, Copy)]
pub enum Environment {
    Staging,
    Production,
}

impl Environment {
    pub fn origin(self) -> &'static str {
        match self {
            Self::Staging => "https://staging-app.loofah.io",
            Self::Production => "https://app.loofah.io",
        }
    }
}

#[derive(Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    expires_in: u64,
    interval: u64,
}

impl Drop for DeviceCode {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.device_code.zeroize();
    }
}

pub struct BrowserLogin {
    client: Client,
    environment: Environment,
    code: DeviceCode,
    expires: Instant,
    next_poll: Instant,
}

impl BrowserLogin {
    pub async fn begin(environment: Environment) -> Result<Self> {
        let client = client(HeaderMap::new())?;
        let response = client
            .post(format!("{}/api/auth/device/code", environment.origin()))
            .json(&serde_json::json!({ "client_id": "loofah-macos" }))
            .send()
            .await?;
        let code: DeviceCode = json(response).await?;
        if code.expires_in == 0
            || code.expires_in > 900
            || code.interval < 5
            || code.interval > 60
            || code.device_code.len() > 256
            || code.user_code.len() > 32
            || !code
                .user_code
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(Error::Authentication);
        }
        let now = Instant::now();
        Ok(Self {
            client,
            environment,
            expires: now + Duration::from_secs(code.expires_in),
            next_poll: now + Duration::from_secs(code.interval),
            code,
        })
    }

    pub fn browser_url(&self) -> String {
        format!(
            "{}/device?user_code={}",
            self.environment.origin(),
            self.code.user_code
        )
    }

    pub fn user_code(&self) -> &str {
        &self.code.user_code
    }

    pub async fn poll(&mut self) -> Result<Option<Zeroizing<String>>> {
        if Instant::now() >= self.expires {
            return Err(Error::Authentication);
        }
        tokio::time::sleep_until(self.next_poll.into()).await;
        let response = self.client.post(format!("{}/api/auth/device/token", self.environment.origin()))
            .json(&serde_json::json!({ "client_id": "loofah-macos", "device_code": self.code.device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code" })).send().await?;
        self.next_poll = Instant::now() + Duration::from_secs(self.code.interval);
        if !response.status().is_success() {
            let error = cloud_error(response).await;
            if let Error::Cloud { code, .. } = &error {
                if code == "authorization_pending" {
                    return Ok(None);
                }
                if code == "slow_down" {
                    self.code.interval = (self.code.interval + 5).min(60);
                    self.next_poll = Instant::now() + Duration::from_secs(self.code.interval);
                    return Ok(None);
                }
            }
            return Err(error);
        }
        #[derive(Deserialize)]
        struct Token {
            access_token: String,
            token_type: String,
        }
        let token: Token = json(response).await?;
        if token.token_type != "Bearer"
            || token.access_token.is_empty()
            || token.access_token.len() > 4096
        {
            return Err(Error::Authentication);
        }
        Ok(Some(Zeroizing::new(token.access_token)))
    }
}

#[derive(Clone, Copy, Default)]
pub struct ProgressSnapshot {
    pub active: usize,
    pub completed: u64,
    pub total: u64,
}
#[derive(Clone, Default)]
pub struct TransferProgress(std::sync::Arc<std::sync::Mutex<ProgressSnapshot>>);
impl TransferProgress {
    pub fn snapshot(&self) -> ProgressSnapshot {
        *self.0.lock().unwrap()
    }
    fn begin(&self, total: u64) -> ProgressGuard {
        let mut progress = self.0.lock().unwrap();
        if progress.active == 0 {
            progress.completed = 0;
            progress.total = 0;
        }
        progress.active += 1;
        progress.total = progress.total.saturating_add(total);
        ProgressGuard(self.clone())
    }
    fn advance(&self, bytes: u64) {
        let mut progress = self.0.lock().unwrap();
        progress.completed = progress.completed.saturating_add(bytes);
    }
}
struct ProgressGuard(TransferProgress);
impl Drop for ProgressGuard {
    fn drop(&mut self) {
        self.0.0.lock().unwrap().active -= 1;
    }
}

#[derive(Clone)]
pub struct RemoteClient {
    progress: TransferProgress,
    client: Client,
    environment: Environment,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Revision {
    pub id: Uuid,
    pub entity: String,
    pub expected: Option<Uuid>,
    pub manifest: Uuid,
    pub operation: Operation,
    pub objects: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<Uuid>,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Checkpoint,
    Delete,
    Restore,
    Resolve,
    Conflict,
}

impl RemoteClient {
    pub fn with_progress(mut self, progress: TransferProgress) -> Self {
        self.progress = progress;
        self
    }
    pub fn new(
        environment: Environment,
        credential: &str,
        generation: Option<Uuid>,
    ) -> Result<Self> {
        let mut headers = HeaderMap::new();
        let token = Zeroizing::new(format!("Bearer {credential}"));
        let mut value = HeaderValue::from_str(&token).map_err(|_| Error::Authentication)?;
        value.set_sensitive(true);
        headers.insert(AUTHORIZATION, value);
        if let Some(generation) = generation {
            headers.insert(
                "x-loofah-recovery-generation",
                HeaderValue::from_str(&generation.to_string())
                    .map_err(|_| Error::Authentication)?,
            );
        }
        Ok(Self {
            client: client(headers)?,
            progress: TransferProgress::default(),
            environment,
        })
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        json(self.client.get(self.url(path)?).send().await?).await
    }

    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        json(self.client.post(self.url(path)?).json(body).send().await?).await
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        json(self.client.delete(self.url(path)?).send().await?).await
    }

    pub async fn commit(&self, revision: &Revision) -> Result<()> {
        self.post::<serde_json::Value>("/sync/revisions", revision)
            .await?;
        Ok(())
    }

    /// The caller retains the spool until commit has been durably acknowledged locally.
    /// Retries send the same persisted ciphertext, including multipart boundaries.
    pub async fn upload(&self, spool: &UploadSpool, control: bool) -> Result<()> {
        #[derive(Serialize)]
        struct Reservation<'a> {
            id: Uuid,
            bytes: u64,
            digest: &'a str,
            kind: &'a str,
        }
        let manifest = spool.encrypted_manifest();
        let manifest_digest = hex(&Sha256::digest(manifest));
        let mut reservations = spool
            .pending_objects()
            .iter()
            .map(|(id, digest)| Reservation {
                id: *id,
                bytes: digest.bytes,
                digest: &digest.sha256,
                kind: "file",
            })
            .collect::<Vec<_>>();
        reservations.push(Reservation {
            id: spool.revision(),
            bytes: manifest.len() as u64,
            digest: &manifest_digest,
            kind: if control {
                "control_manifest"
            } else {
                "manifest"
            },
        });
        for chunk in reservations.chunks(256) {
            self.post::<serde_json::Value>("/sync/reservations", &chunk)
                .await?;
        }
        let mut uploads = tokio::task::JoinSet::new();
        for (id, digest) in spool.pending_objects() {
            while uploads.len() >= crate::scheduler::TRANSFER_CONCURRENCY {
                uploads
                    .join_next()
                    .await
                    .ok_or(Error::RecoveryRequired)?
                    .map_err(|_| Error::RecoveryRequired)??;
            }
            let remote = self.clone();
            let id = *id;
            let bytes = digest.bytes;
            let file = tokio::fs::File::from_std(spool.open_object(id)?);
            uploads.spawn(async move { remote.upload_reader(id, bytes, file).await });
        }
        while let Some(result) = uploads.join_next().await {
            result.map_err(|_| Error::RecoveryRequired)??;
        }
        self.upload_reader(spool.revision(), manifest.len() as u64, manifest)
            .await
    }

    async fn upload_reader(
        &self,
        id: Uuid,
        bytes: u64,
        mut reader: impl tokio::io::AsyncRead + Unpin,
    ) -> Result<()> {
        let _progress = self.progress.begin(bytes);
        if bytes <= MULTIPART_BYTES as u64 {
            let mut body = vec![0; bytes as usize];
            reader.read_exact(&mut body).await?;
            let response = self
                .client
                .put(self.url(&format!("/sync/objects/{id}"))?)
                .body(body)
                .send()
                .await?;
            json::<serde_json::Value>(response).await?;
            self.progress.advance(bytes);
            return Ok(());
        }
        #[derive(Deserialize)]
        struct Part {
            number: u64,
            bytes: u64,
            digest: String,
        }
        #[derive(Deserialize)]
        struct Multipart {
            complete: bool,
            parts: Vec<Part>,
        }
        let state: Multipart = self
            .post(
                &format!("/sync/objects/{id}/multipart"),
                &serde_json::json!({}),
            )
            .await?;
        if state.complete {
            self.progress.advance(bytes);
            return Ok(());
        }
        let count = bytes.div_ceil(MULTIPART_BYTES as u64);
        for number in 1..=count {
            let size = (bytes - (number - 1) * MULTIPART_BYTES as u64).min(MULTIPART_BYTES as u64)
                as usize;
            let mut body = vec![0; size];
            reader.read_exact(&mut body).await?;
            let digest = hex(&Sha256::digest(&body));
            if let Some(part) = state.parts.iter().find(|part| part.number == number) {
                if part.bytes != size as u64 || part.digest != digest {
                    return Err(Error::Authentication);
                }
                self.progress.advance(size as u64);
                continue;
            }
            let response = self
                .client
                .put(self.url(&format!("/sync/objects/{id}/parts/{number}"))?)
                .header("x-loofah-sha256", digest)
                .body(body)
                .send()
                .await?;
            json::<serde_json::Value>(response).await?;
            self.progress.advance(size as u64);
        }
        self.post::<serde_json::Value>(
            &format!("/sync/objects/{id}/complete"),
            &serde_json::json!({}),
        )
        .await?;
        Ok(())
    }

    pub async fn download(
        &self,
        id: Uuid,
        expected: &FileDigest,
        directory: &Path,
    ) -> Result<tempfile::TempPath> {
        let _progress = self.progress.begin(expected.bytes);
        if fs2::available_space(directory)? < expected.bytes.saturating_add(16 * 1024 * 1024) {
            return Err(Error::DiskFull);
        }
        let temp = tempfile::NamedTempFile::new_in(directory)?;
        let (file, path) = temp.into_parts();
        let mut output = tokio::fs::File::from_std(file);
        let mut response = self
            .client
            .get(self.url(&format!("/sync/objects/{id}"))?)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(cloud_error(response).await);
        }
        if response.content_length() != Some(expected.bytes) {
            return Err(Error::Authentication);
        }
        let mut length = 0u64;
        let mut hasher = Sha256::new();
        while let Some(chunk) = response.chunk().await? {
            length = length
                .checked_add(chunk.len() as u64)
                .ok_or(Error::Authentication)?;
            if length > expected.bytes {
                return Err(Error::Authentication);
            }
            hasher.update(&chunk);
            output.write_all(&chunk).await?;
            self.progress.advance(chunk.len() as u64);
        }
        if length != expected.bytes || hex(&hasher.finalize()) != expected.sha256 {
            return Err(Error::Authentication);
        }
        output.sync_all().await?;
        Ok(path)
    }

    fn url(&self, path: &str) -> Result<String> {
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains(['\\', '#', '\r', '\n'])
        {
            return Err(Error::Invalid("invalid sync endpoint".into()));
        }
        Ok(format!("{}/api{path}", self.environment.origin()))
    }
}

fn client(headers: HeaderMap) -> Result<Client> {
    Ok(Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .https_only(true)
        .user_agent("Loofah Sync/1")
        .build()?)
}

async fn json<T: DeserializeOwned>(response: Response) -> Result<T> {
    if !response.status().is_success() {
        return Err(cloud_error(response).await);
    }
    let bytes = limited_body(response, 64 * 1024 * 1024).await?;
    serde_json::from_slice(&bytes).map_err(|_| Error::Authentication)
}

async fn cloud_error(response: Response) -> Error {
    let status = response.status().as_u16();
    let value = limited_body(response, 64 * 1024)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let code = value
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(|value| value.as_str())
        .unwrap_or("unavailable");
    let code = if code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
    {
        code
    } else {
        "unavailable"
    };
    Error::Cloud {
        status,
        code: code.to_owned(),
    }
}

async fn limited_body(mut response: Response, limit: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(Error::Authentication);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(Error::Authentication);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

use std::path::{Path, PathBuf};
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{
    Call, Credentials, Error, Result, ServerInfo, Upload, Versions,
    client::invoke_request,
    filesystem::{atomic_json, file_version, safe_path},
    recovery::{Destination, PendingMutation, RecoverySpool},
};

pub trait CredentialStore: Send + Sync {
    fn load(&self, key: &str) -> Result<Option<Credentials>>;
    fn save(&self, key: &str, credentials: &Credentials) -> Result<()>;
    fn remove(&self, key: &str) -> Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub base_url: String,
    pub server_id: String,
    pub user_id: String,
    pub vault_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferJob {
    pub id: String,
    pub destination: Destination,
    pub session_id: String,
    pub source: PathBuf,
    pub filename: String,
    pub audio: bool,
    pub expected: Versions,
    pub error: Option<Error>,
    #[serde(default)]
    pub finished: bool,
}

impl Profile {
    fn same_account(&self, other: &Self) -> bool {
        self.server_id == other.server_id
            && self.user_id == other.user_id
            && self.base_url == other.base_url
    }

    pub fn credential_key(&self) -> String {
        format!("{}:{}", self.server_id, self.user_id)
    }
    pub fn destination(&self) -> Result<Destination> {
        Ok(Destination {
            server_id: self.server_id.clone(),
            user_id: self.user_id.clone(),
            vault_id: self
                .vault_id
                .clone()
                .ok_or_else(|| Error::new("invalid_request", "select a vault"))?,
        })
    }
}

pub struct ConnectionState {
    pub http: reqwest::Client,
    pub root: PathBuf,
    pub recording: AtomicBool,
    profile: RwLock<Option<Profile>>,
    secrets: Arc<dyn CredentialStore>,
    credentials: Mutex<Option<Credentials>>,
    versions: Mutex<Versions>,
    mutations: Mutex<()>,
    transfers_gate: Mutex<()>,
    last_error: RwLock<Option<Error>>,
    activities: RwLock<std::collections::BTreeSet<String>>,
}

impl ConnectionState {
    pub fn open(root: PathBuf, secrets: Arc<dyn CredentialStore>) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        let profile = match std::fs::read(root.join("connection.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let state = Self {
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| Error::new("unavailable", error.to_string()))?,
            root,
            recording: false.into(),
            profile: RwLock::new(profile),
            secrets,
            credentials: Mutex::new(None),
            versions: Mutex::new(Versions::new()),
            mutations: Mutex::new(()),
            transfers_gate: Mutex::new(()),
            last_error: RwLock::new(None),
            activities: RwLock::new(Default::default()),
        };
        state.recover_processing()?;
        Ok(state)
    }

    pub fn profile(&self) -> Option<Profile> {
        self.profile.read().unwrap().clone()
    }
    pub fn active(&self) -> bool {
        self.profile()
            .is_some_and(|profile| profile.vault_id.is_some())
    }

    pub fn scratch_base(&self) -> Option<PathBuf> {
        self.profile()
            .and_then(|profile| profile.destination().ok())
            .map(|destination| {
                self.root
                    .join("processing")
                    .join(destination.server_id)
                    .join(destination.user_id)
                    .join(destination.vault_id)
            })
    }

    pub fn status(&self) -> Value {
        json!({"profile":self.profile(),"active":self.active(),"recording":self.recording.load(Ordering::Acquire),"processing":!self.activities.read().unwrap().is_empty(),"error":self.last_error.read().unwrap().clone()})
    }

    pub async fn begin_processing(&self) -> String {
        let _gate = self.mutations.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        self.activities.write().unwrap().insert(id.clone());
        id
    }
    pub fn end_processing(&self, id: &str) {
        self.activities.write().unwrap().remove(id);
    }
    fn check_processing(&self) -> Result<()> {
        if !self.activities.read().unwrap().is_empty() {
            return Err(Error::new(
                "conflict",
                "wait for transcription and summaries to finish before switching vaults",
            ));
        }
        Ok(())
    }

    pub async fn connect(
        &self,
        base_url: String,
        server: ServerInfo,
        credentials: Credentials,
    ) -> Result<()> {
        crate::validate_id(&server.id)?;
        crate::validate_id(&credentials.user.id)?;
        let base_url = validate_server_url(&base_url)?;
        if self.recording.load(Ordering::Acquire) {
            return Err(Error::new(
                "conflict",
                "stop recording before changing connections",
            ));
        }
        let _gate = self.mutations.lock().await;
        self.check_processing()?;
        let origins_path = self.root.join("server-origins.json");
        let mut origins: std::collections::BTreeMap<String, String> =
            match std::fs::read(&origins_path) {
                Ok(bytes) => serde_json::from_slice(&bytes)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
                Err(error) => return Err(error.into()),
            };
        if let Some(profile) = self.profile() {
            origins.entry(profile.server_id).or_insert(profile.base_url);
        }
        if origins
            .get(&server.id)
            .is_some_and(|origin| origin != &base_url)
        {
            return Err(Error::new(
                "invalid_request",
                "this server identity is already associated with another address",
            ));
        }
        origins.insert(server.id.clone(), base_url.clone());
        atomic_json(&origins_path, &origins)?;
        let previous = self.profile();
        let same_account = previous.as_ref().is_some_and(|profile| {
            profile.server_id == server.id && profile.user_id == credentials.user.id
        });
        if previous.as_ref().is_some_and(|profile| {
            profile.vault_id.is_some()
                && (profile.server_id != server.id || profile.user_id != credentials.user.id)
        }) {
            return Err(Error::new(
                "conflict",
                "choose your local folder before connecting another account or server",
            ));
        }
        let vault_id = previous
            .filter(|profile| {
                profile.server_id == server.id && profile.user_id == credentials.user.id
            })
            .and_then(|profile| profile.vault_id);
        let profile = Profile {
            base_url,
            server_id: server.id,
            user_id: credentials.user.id.clone(),
            vault_id,
        };
        self.secrets.save(&profile.credential_key(), &credentials)?;
        atomic_json(&self.root.join("connection.json"), &Some(&profile))?;
        *self.profile.write().unwrap() = Some(profile);
        *self.credentials.lock().await = Some(credentials);
        if !same_account {
            self.versions.lock().await.clear();
        }
        Ok(())
    }

    pub async fn select(&self, vault_id: Option<String>) -> Result<()> {
        if self.recording.load(Ordering::Acquire) {
            return Err(Error::new(
                "conflict",
                "stop recording before switching vaults",
            ));
        }
        let _gate = self.mutations.lock().await;
        self.check_processing()?;
        let mut profile = self
            .profile()
            .ok_or_else(|| Error::new("unauthenticated", "connect to a company server"))?;
        if let Some(id) = &vault_id {
            crate::validate_id(id)?;
            let vaults = self.request("GET", "/v1/vaults", None).await?;
            if !vaults
                .as_array()
                .is_some_and(|vaults| vaults.iter().any(|vault| vault["id"] == *id))
            {
                return Err(Error::new("forbidden", "vault is unavailable"));
            }
        }
        profile.vault_id = vault_id;
        atomic_json(&self.root.join("connection.json"), &Some(&profile))?;
        *self.profile.write().unwrap() = Some(profile);
        self.versions.lock().await.clear();
        *self.last_error.write().unwrap() = None;
        Ok(())
    }

    pub async fn logout(&self) -> Result<()> {
        if self.recording.load(Ordering::Acquire) {
            return Err(Error::new("conflict", "stop recording before signing out"));
        }
        let _gate = self.mutations.lock().await;
        self.check_processing()?;
        if let Some(profile) = self.profile() {
            self.request("POST", "/v1/auth/logout", Some(json!({})))
                .await?;
            self.secrets.remove(&profile.credential_key())?;
        }
        atomic_json(&self.root.join("connection.json"), &Value::Null)?;
        *self.profile.write().unwrap() = None;
        *self.credentials.lock().await = None;
        self.versions.lock().await.clear();
        Ok(())
    }

    pub async fn authenticated(&self) -> Result<(Profile, Credentials)> {
        let profile = self
            .profile()
            .ok_or_else(|| Error::new("unauthenticated", "connect to a company server"))?;
        let mut current = self.credentials.lock().await;
        if current.is_none() {
            *current = self.secrets.load(&profile.credential_key())?;
        }
        let credentials = current
            .clone()
            .ok_or_else(|| Error::new("unauthenticated", "sign in to the company server"))?;
        if credentials.user.id != profile.user_id {
            return Err(Error::new(
                "unauthenticated",
                "credentials belong to another account",
            ));
        }
        if credentials.expires_at > chrono_now() + 30 {
            return Ok((profile, credentials));
        }
        let response = self
            .http
            .post(format!("{}/v1/auth/refresh", profile.base_url))
            .timeout(std::time::Duration::from_secs(30))
            .json(&json!({"refresh_token":credentials.refresh_token}))
            .send()
            .await
            .map_err(|error| Error::new("unavailable", error.to_string()))?;
        let credentials: Credentials = decode(response).await?;
        self.secrets.save(&profile.credential_key(), &credentials)?;
        *current = Some(credentials.clone());
        Ok((profile, credentials))
    }

    pub async fn refresh_rejected_access(&self, rejected: &str) -> Result<(Profile, Credentials)> {
        let authenticated = self.authenticated().await?;
        if authenticated.1.access_token != rejected {
            return Ok(authenticated);
        }
        {
            let mut current = self.credentials.lock().await;
            if let Some(credentials) = current.as_mut()
                && credentials.access_token == rejected
            {
                credentials.expires_at = 0;
            }
        }
        self.authenticated().await
    }

    async fn send_authenticated(
        &self,
        profile: &Profile,
        credentials: &Credentials,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        let same_account = || {
            self.profile()
                .is_some_and(|current| current.same_account(profile))
        };
        if !same_account() {
            return Err(Error::new(
                "conflict",
                "connection changed before sending the request",
            ));
        }
        let retry = request.try_clone();
        let response = request
            .bearer_auth(&credentials.access_token)
            .send()
            .await
            .map_err(|error| Error::new("unavailable", error.to_string()))?;
        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        if !same_account() {
            return Err(Error::new(
                "conflict",
                "connection changed while the request was in flight",
            ));
        }
        let refreshed = self
            .refresh_rejected_access(&credentials.access_token)
            .await;
        if !same_account() {
            return Err(Error::new(
                "conflict",
                "connection changed while refreshing credentials",
            ));
        }
        let (current, credentials) = refreshed?;
        if !current.same_account(profile) {
            return Err(Error::new(
                "conflict",
                "refreshed credentials belong to another connection",
            ));
        }
        retry
            .ok_or_else(|| Error::new("unavailable", "request body cannot be retried"))?
            .bearer_auth(credentials.access_token)
            .send()
            .await
            .map_err(|error| Error::new("unavailable", error.to_string()))
    }

    pub async fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        if !path.starts_with("/v1/")
            || path.contains("..")
            || path.contains('\\')
            || path.contains('#')
        {
            return Err(Error::new("invalid_request", "invalid company API path"));
        }
        let (profile, credentials) = self.authenticated().await?;
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| Error::new("invalid_request", "invalid HTTP method"))?;
        let mut request = self
            .http
            .request(method, format!("{}{}", profile.base_url, path))
            .timeout(std::time::Duration::from_secs(60));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = self
            .send_authenticated(&profile, &credentials, request)
            .await?;
        decode(response).await
    }

    pub async fn call(&self, operation: &str, args: Value, query: bool) -> Result<Value> {
        let profile = self
            .profile()
            .ok_or_else(|| Error::new("unauthenticated", "connect to a company server"))?;
        let destination = profile.destination()?;
        if matches!(
            operation,
            "session_prepare_recording" | "session_begin_audio_import"
        ) {
            let session_id = args["session_id"]
                .as_str()
                .ok_or_else(|| Error::new("invalid_request", "missing session_id"))?;
            if self.transfers()?.iter().any(|(_, job)| {
                job.destination == destination && job.session_id == session_id && job.audio
            }) {
                return Err(Error::new(
                    "conflict",
                    "save or export the pending recording before recording into this session again",
                ));
            }
            let artifacts = self.artifacts(session_id).await?;
            if let Some(base) = self.scratch_base() {
                for name in ["audio.mp3", "audio.wav", "audio.ogg"] {
                    let _ = std::fs::remove_file(base.join("sessions").join(session_id).join(name));
                }
            }
            let transfers = self.root.join("transfers");
            if transfers.is_dir() {
                for entry in transfers.read_dir()? {
                    let entry = entry?;
                    if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                        continue;
                    }
                    let job: TransferJob = serde_json::from_slice(&std::fs::read(entry.path())?)?;
                    if job.finished
                        && job.audio
                        && job.destination == destination
                        && job.session_id == session_id
                    {
                        std::fs::remove_file(entry.path())?;
                    }
                }
            }
            let mut baseline = Versions::new();
            for name in ["audio.mp3", "audio.wav", "audio.ogg", "audio.peaks.json"] {
                let key = format!("sessions/{session_id}/{name}");
                let version = artifacts
                    .iter()
                    .find(|item| item["key"] == key)
                    .and_then(|item| item["version"].as_str().map(str::to_string));
                baseline.insert(key, version);
            }
            atomic_json(&self.baseline_path(&destination, session_id), &baseline)?;
            if operation == "session_prepare_recording"
                && artifacts.iter().any(|item| {
                    matches!(
                        item["filename"].as_str(),
                        Some("audio.mp3" | "audio.wav" | "audio.ogg")
                    )
                })
            {
                self.processing_audio(session_id).await?;
            }
        }
        if query {
            let mut call = Call::query(operation, args);
            let mut all_headers = Vec::new();
            loop {
                let result = self.send(call.clone(), &destination).await;
                let mut versions = self.versions.lock().await;
                if self
                    .profile()
                    .and_then(|profile| profile.destination().ok())
                    .as_ref()
                    != Some(&destination)
                {
                    return Err(Error::new("conflict", "vault changed while reading"));
                }
                let reply = result?;
                // An explicit reload resolves a conflict; background reads must not advance an editor's save baseline.
                for (key, version) in reply.versions {
                    versions.entry(key).or_insert(version);
                }
                drop(versions);
                if operation != "session_list_headers" {
                    return Ok(reply.data);
                }
                let headers = reply
                    .data
                    .as_array()
                    .ok_or_else(|| Error::new("invalid_data", "invalid session page"))?;
                all_headers.extend(headers.iter().cloned());
                if headers.len() < 200 {
                    return Ok(json!(all_headers));
                }
                call.args["after"] = headers.last().unwrap()["id"].clone();
            }
        }
        let _gate = self.mutations.lock().await;
        if self
            .profile()
            .and_then(|profile| profile.destination().ok())
            .as_ref()
            != Some(&destination)
        {
            return Err(Error::new("conflict", "vault changed before saving"));
        }
        let expected = if matches!(
            operation,
            "session_prepare_recording"
                | "session_begin_audio_import"
                | "session_finish_audio_import"
                | "session_mark_recording_started"
                | "session_mark_recording_ended"
                | "people_ensure"
                | "tags_ensure"
                | "session_append_transcript"
                | "session_flush_transcript"
                | "session_finish_transcript"
                | "session_update_meta"
                | "session_accept_tag_suggestion"
                | "session_dismiss_tag_suggestion"
                | "session_ensure_summary"
                | "session_remove_tasks"
                | "session_move_tasks"
        ) {
            Versions::new()
        } else {
            self.versions.lock().await.clone()
        };
        let call = Call::mutation(operation, args, expected);
        let mut pending = PendingMutation {
            attempted: false,
            created_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            destination,
            call,
            error: None,
        };
        let backlog = self.spool()?.list(&pending.destination)?;
        if !backlog.is_empty() {
            pending.created_at_ms = pending
                .created_at_ms
                .max(backlog.last().unwrap().created_at_ms + 1);
            if let Some(previous) = backlog.into_iter().rev().find(|previous| {
                !previous.attempted
                    && matches!(
                        pending.call.operation.as_str(),
                        "session_save_note" | "session_write_note"
                    )
                    && previous.call.operation == pending.call.operation
                    && previous.call.args["session_id"] == pending.call.args["session_id"]
                    && previous.call.args["doc_id"] == pending.call.args["doc_id"]
            }) {
                pending.call.mutation_id = previous.call.mutation_id;
                pending.created_at_ms = previous.created_at_ms;
            }
            let error = Error::new(
                "unavailable",
                "changes are saved on this device and waiting for earlier saves",
            );
            pending.error = Some(error.clone());
            self.spool()?.save(&pending)?;
            self.set_error(Some(error.clone()));
            return Err(error);
        }
        self.spool()?.save(&pending)?;
        self.perform(pending).await
    }

    async fn send(&self, call: Call, destination: &Destination) -> Result<crate::Reply> {
        let (profile, credentials) = self.authenticated().await?;
        if profile.destination().ok().as_ref() != Some(destination) {
            return Err(Error::new(
                "conflict",
                "vault changed before sending the request",
            ));
        }
        let request = invoke_request(&self.http, &profile.base_url, &destination.vault_id, &call);
        decode(
            self.send_authenticated(&profile, &credentials, request)
                .await?,
        )
        .await
    }

    fn spool(&self) -> Result<RecoverySpool> {
        RecoverySpool::new(&self.root.join("pending"))
    }

    async fn perform(&self, mut pending: PendingMutation) -> Result<Value> {
        pending.attempted = true;
        self.spool()?.save(&pending)?;
        match self.send(pending.call.clone(), &pending.destination).await {
            Ok(reply) => {
                self.versions.lock().await.extend(reply.versions.clone());
                for mut waiting in self.spool()?.list(&pending.destination)? {
                    if waiting.attempted {
                        continue;
                    }
                    for (key, version) in &reply.versions {
                        if waiting.call.expected.get(key) == pending.call.expected.get(key) {
                            waiting.call.expected.insert(key.clone(), version.clone());
                        }
                    }
                    self.spool()?.save(&waiting)?;
                }
                self.spool()?
                    .acknowledge(pending.call.mutation_id.as_deref().unwrap())?;
                *self.last_error.write().unwrap() = None;
                Ok(reply.data)
            }
            Err(error) => {
                pending.error = Some(error.clone());
                self.spool()?.save(&pending)?;
                *self.last_error.write().unwrap() = Some(error.clone());
                Err(error)
            }
        }
    }

    pub async fn retry(&self) -> Result<()> {
        let _gate = self.mutations.lock().await;
        let destination = self
            .profile()
            .ok_or_else(|| Error::new("unauthenticated", "connect to a company server"))?
            .destination()?;
        for pending in self.spool()?.list(&destination)? {
            if pending.error.as_ref().is_some_and(|error| {
                matches!(
                    error.code.as_str(),
                    "conflict" | "forbidden" | "precondition_required" | "invalid_request"
                )
            }) {
                continue;
            }
            let current = self
                .spool()?
                .list(&destination)?
                .into_iter()
                .find(|current| current.call.mutation_id == pending.call.mutation_id)
                .unwrap_or(pending);
            self.perform(current).await?;
        }
        Ok(())
    }

    pub fn pending(&self) -> Result<Vec<PendingMutation>> {
        let Some(profile) = self.profile() else {
            return Ok(vec![]);
        };
        let Ok(destination) = profile.destination() else {
            return Ok(vec![]);
        };
        self.spool()?.list(&destination)
    }

    pub async fn resolve(&self, id: &str, action: &str, expected: Versions) -> Result<()> {
        let _gate = self.mutations.lock().await;
        let pending = self
            .pending()?
            .into_iter()
            .find(|pending| pending.call.mutation_id.as_deref() == Some(id))
            .ok_or_else(|| Error::new("not_found", "pending draft not found"))?;
        match action {
            "discard" => {
                self.spool()?.acknowledge(id)?;
                self.versions.lock().await.clear();
                *self.last_error.write().unwrap() = None;
            }
            "replace" => {
                self.versions.lock().await.clear();
                if expected.is_empty() {
                    return Err(Error::new(
                        "precondition_required",
                        "preview the current saved version before replacing it",
                    ));
                }
                let mut replacement = pending.clone();
                replacement.call.mutation_id = Some(uuid::Uuid::new_v4().to_string());
                replacement.call.expected = expected;
                replacement.call.args["expected_markdown"] = Value::Null;
                if let Some(patch) = replacement
                    .call
                    .args
                    .get_mut("patch")
                    .and_then(Value::as_object_mut)
                {
                    patch.insert("expected_markdown".into(), Value::Null);
                }
                replacement.error = None;
                replacement.attempted = false;
                self.spool()?.save(&replacement)?;
                self.perform(replacement).await?;
                self.spool()?.acknowledge(id)?;
            }
            _ => return Err(Error::new("invalid_request", "unsupported recovery action")),
        }
        Ok(())
    }

    pub async fn save_separate(&self, id: &str, created_at: &str) -> Result<String> {
        let _gate = self.mutations.lock().await;
        let pending = self
            .pending()?
            .into_iter()
            .find(|pending| pending.call.mutation_id.as_deref() == Some(id))
            .ok_or_else(|| Error::new("not_found", "pending draft not found"))?;
        let args = &pending.call.args;
        let markdown = args["markdown"]
            .as_str()
            .or(args["patch"]["markdown"].as_str())
            .or(args["doc"]["markdown"].as_str())
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string_pretty(args).unwrap_or_default());
        let session = uuid::Uuid::new_v4().to_string();
        for (operation, args) in [
            (
                "session_write_meta",
                json!({"meta":{"id":session,"title":"Recovered draft","created_at":created_at,"started_at":null,"ended_at":null,"tags":[]}}),
            ),
            (
                "session_save_note",
                json!({"session_id":session,"markdown":markdown,"title":null}),
            ),
        ] {
            let recovery = PendingMutation {
                attempted: false,
                created_at_ms: chrono_now() as u64 * 1000,
                destination: pending.destination.clone(),
                call: Call::mutation(operation, args, Versions::new()),
                error: None,
            };
            self.spool()?.save(&recovery)?;
            self.perform(recovery).await?;
        }
        self.spool()?.acknowledge(id)?;
        Ok(session)
    }

    pub async fn artifacts(&self, session_id: &str) -> Result<Vec<Value>> {
        crate::validate_id(session_id)?;
        let profile = self
            .profile()
            .ok_or_else(|| Error::new("unauthenticated", "connect to a company server"))?;
        let vault = profile
            .vault_id
            .ok_or_else(|| Error::new("invalid_request", "select a vault"))?;
        let result = self
            .request(
                "GET",
                &format!("/v1/vaults/{vault}/sessions/{session_id}/artifacts"),
                None,
            )
            .await?;
        serde_json::from_value(result).map_err(Into::into)
    }

    pub async fn media_url(&self, session_id: &str, key: &str) -> Result<String> {
        crate::validate_id(session_id)?;
        let profile = self
            .profile()
            .ok_or_else(|| Error::new("unauthenticated", "connect to a company server"))?;
        let vault = profile
            .vault_id
            .ok_or_else(|| Error::new("invalid_request", "select a vault"))?;
        let result = self
            .request(
                "POST",
                &format!("/v1/vaults/{vault}/sessions/{session_id}/media"),
                Some(json!({"key":key})),
            )
            .await?;
        result["url"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::new("invalid_data", "invalid media URL"))
    }

    pub async fn remove_artifact(&self, session_id: &str, key: &str) -> Result<()> {
        let artifact = self
            .artifacts(session_id)
            .await?
            .into_iter()
            .find(|item| item["key"] == key);
        let Some(artifact) = artifact else {
            return Ok(());
        };
        self.versions
            .lock()
            .await
            .insert(key.into(), artifact["version"].as_str().map(str::to_string));
        self.call(
            "artifact_remove",
            json!({"session_id":session_id,"key":key}),
            false,
        )
        .await?;
        Ok(())
    }

    pub async fn read_artifact(&self, session_id: &str, key: &str) -> Result<Vec<u8>> {
        let url = self.media_url(session_id, key).await?;
        self.read_media_url(&url).await
    }

    pub async fn read_media_url(&self, url: &str) -> Result<Vec<u8>> {
        let profile = self
            .profile()
            .ok_or_else(|| Error::new("unauthenticated", "connect to a company server"))?;
        if !url.starts_with(&format!("{}/v1/media/", profile.base_url)) {
            return Err(Error::new(
                "invalid_request",
                "media URL must belong to the company server",
            ));
        }
        let mut response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| Error::new("unavailable", error.to_string()))?
            .error_for_status()
            .map_err(|error| Error::new("unavailable", error.to_string()))?;
        if response
            .content_length()
            .is_some_and(|length| length > 32 * 1024 * 1024)
        {
            return Err(Error::new(
                "too_large",
                "attachment is too large to load into memory",
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| Error::new("unavailable", error.to_string()))?
        {
            if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
                return Err(Error::new(
                    "too_large",
                    "attachment is too large to load into memory",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    pub async fn upload(
        &self,
        session_id: &str,
        source: &Path,
        filename: &str,
        audio: bool,
    ) -> Result<Value> {
        self.enqueue_upload(session_id, source, filename, audio)?;
        self.retry_uploads().await?;
        Ok(Value::Null)
    }

    pub async fn publish_peaks(
        &self,
        session_id: &str,
        source: &Path,
        audio_name: &str,
        audio_version: &str,
    ) -> Result<()> {
        let destination = self
            .profile()
            .ok_or_else(|| Error::new("unauthenticated", "connect to a company server"))?
            .destination()?;
        let artifacts = self.artifacts(session_id).await?;
        let audio_key = format!("sessions/{session_id}/{audio_name}");
        if !artifacts
            .iter()
            .any(|item| item["key"] == audio_key && item["version"] == audio_version)
        {
            return Err(Error::new(
                "conflict",
                "recording changed while preparing the waveform",
            ));
        }
        let mut expected = Versions::new();
        for name in ["audio.mp3", "audio.wav", "audio.ogg", "audio.peaks.json"] {
            let key = format!("sessions/{session_id}/{name}");
            let version = artifacts
                .iter()
                .find(|item| item["key"] == key)
                .and_then(|item| item["version"].as_str().map(str::to_string));
            expected.insert(key, version);
        }
        self.upload_to(
            destination,
            session_id,
            source,
            "audio.peaks.json",
            true,
            expected,
        )
        .await?;
        Ok(())
    }

    async fn upload_to(
        &self,
        destination: Destination,
        session_id: &str,
        source: &Path,
        filename: &str,
        audio: bool,
        expected: Versions,
    ) -> Result<Value> {
        crate::validate_id(session_id)?;
        let profile = self
            .profile()
            .ok_or_else(|| Error::new("unauthenticated", "connect to a company server"))?;
        if profile.server_id != destination.server_id || profile.user_id != destination.user_id {
            return Err(Error::new(
                "unauthenticated",
                "upload belongs to another account",
            ));
        }
        let size = source.metadata()?.len();
        let source_path = source.to_path_buf();
        let checksum = tokio::task::spawn_blocking(move || file_version(&source_path))
            .await
            .map_err(|error| Error::new("storage", error.to_string()))??
            .ok_or_else(|| Error::new("not_found", "upload source disappeared"))?;
        let key = if audio {
            format!("sessions/{session_id}/{filename}")
        } else {
            format!("sessions/{session_id}/attachments/{filename}")
        };
        let previous = expected.get(&key).cloned().flatten();
        let state_key = format!(
            "{}-{}-{}",
            destination.server_id,
            destination.vault_id,
            crate::digest(format!("{}:{key}:{checksum}", destination.user_id).as_bytes())
        );
        let state_path = self.root.join("uploads").join(format!("{state_key}.json"));
        let upload = match std::fs::read(&state_path) {
            Ok(bytes) => serde_json::from_slice::<Upload>(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let value = self.request("POST", &format!("/v1/vaults/{}/uploads", destination.vault_id), Some(json!({"session_id":session_id,"filename":filename,"size":size,"sha256":checksum,"expected":previous,"expected_versions":expected,"audio":audio}))).await?;
                let upload: Upload = serde_json::from_value(value)?;
                atomic_json(&state_path, &upload)?;
                upload
            }
            Err(error) => return Err(error.into()),
        };
        let prefix = format!("/v1/vaults/{}/uploads/{}", destination.vault_id, upload.id);
        let mut upload: Upload = serde_json::from_value(self.request("GET", &prefix, None).await?)?;
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let mut file = tokio::fs::File::open(source).await?;
        while upload.offset < upload.size {
            file.seek(std::io::SeekFrom::Start(upload.offset)).await?;
            let mut bytes = vec![0; (upload.size - upload.offset).min(8 * 1024 * 1024) as usize];
            file.read_exact(&mut bytes).await?;
            let (current, credentials) = self.authenticated().await?;
            if current.server_id != destination.server_id || current.user_id != destination.user_id
            {
                return Err(Error::new(
                    "unauthenticated",
                    "account changed during upload",
                ));
            }
            let request = self
                .http
                .put(format!("{}{}", profile.base_url, prefix))
                .timeout(std::time::Duration::from_secs(60))
                .header("Upload-Offset", upload.offset)
                .body(bytes);
            let response = self
                .send_authenticated(&current, &credentials, request)
                .await?;
            upload = decode(response).await?;
            atomic_json(&state_path, &upload)?;
        }
        let value = self
            .request("POST", &format!("{prefix}/finish"), Some(json!({})))
            .await?;
        std::fs::remove_file(state_path)?;
        Ok(value)
    }

    pub fn enqueue_upload(
        &self,
        session_id: &str,
        source: &Path,
        filename: &str,
        audio: bool,
    ) -> Result<()> {
        crate::validate_id(session_id)?;
        let destination = self
            .profile()
            .ok_or_else(|| Error::new("unauthenticated", "connect to a company server"))?
            .destination()?;
        let id = crate::digest(
            format!(
                "{}:{}:{}:{session_id}:{filename}",
                destination.server_id, destination.user_id, destination.vault_id
            )
            .as_bytes(),
        );
        let job_path = self.root.join("transfers").join(format!("{id}.json"));
        if job_path.exists() {
            return Ok(());
        }
        let expected = if audio {
            serde_json::from_slice(
                &std::fs::read(self.baseline_path(&destination, session_id)).map_err(|_| {
                    Error::new(
                        "precondition_required",
                        "prepare the recording before uploading it",
                    )
                })?,
            )?
        } else {
            Versions::from([(
                format!("sessions/{session_id}/attachments/{filename}"),
                None,
            )])
        };
        let job = TransferJob {
            id,
            destination,
            session_id: session_id.into(),
            source: source.to_path_buf(),
            filename: filename.into(),
            audio,
            expected,
            error: None,
            finished: false,
        };
        std::fs::File::open(source)?.sync_all()?;
        atomic_json(&job_path, &job)
    }

    fn baseline_path(&self, destination: &Destination, session: &str) -> PathBuf {
        self.root.join("baselines").join(format!(
            "{}-{}-{}-{session}.json",
            destination.server_id, destination.user_id, destination.vault_id
        ))
    }

    pub fn transfers(&self) -> Result<Vec<(PathBuf, TransferJob)>> {
        let root = self.root.join("transfers");
        std::fs::create_dir_all(&root)?;
        let Some(profile) = self.profile() else {
            return Ok(vec![]);
        };
        let mut jobs = Vec::new();
        for entry in root.read_dir()? {
            let entry = entry?;
            if !entry.file_type()?.is_file()
                || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let job: TransferJob = serde_json::from_slice(&std::fs::read(entry.path())?)?;
            if !job.finished
                && job.destination.server_id == profile.server_id
                && job.destination.user_id == profile.user_id
            {
                jobs.push((entry.path(), job));
            }
        }
        Ok(jobs)
    }

    pub async fn retry_uploads(&self) -> Result<()> {
        let _gate = self.transfers_gate.lock().await;
        if self.recording.load(Ordering::Acquire) {
            return Ok(());
        }
        for (path, mut job) in self.transfers()? {
            if job.error.as_ref().is_some_and(|error| {
                matches!(
                    error.code.as_str(),
                    "conflict" | "forbidden" | "not_found" | "invalid_request"
                )
            }) {
                continue;
            }
            match self
                .upload_to(
                    job.destination.clone(),
                    &job.session_id,
                    &job.source,
                    &job.filename,
                    job.audio,
                    job.expected.clone(),
                )
                .await
            {
                Ok(_) => {
                    job.finished = true;
                    job.error = None;
                    atomic_json(&path, &job)?;
                }
                Err(error) => {
                    job.error = Some(error.clone());
                    atomic_json(&path, &job)?;
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    pub async fn discard_transfer(&self, id: &str) -> Result<()> {
        crate::validate_id(id)?;
        let _gate = self.transfers_gate.lock().await;
        let (path, _) = self
            .transfers()?
            .into_iter()
            .find(|(_, job)| job.id == id)
            .ok_or_else(|| Error::new("not_found", "pending upload not found"))?;
        std::fs::remove_file(&path)?;
        std::fs::File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    }

    pub fn set_error(&self, error: Option<Error>) {
        *self.last_error.write().unwrap() = error;
    }

    fn recover_processing(&self) -> Result<()> {
        let Some(base) = self.scratch_base() else {
            return Ok(());
        };
        let Some(destination) = self
            .profile()
            .and_then(|profile| profile.destination().ok())
        else {
            return Ok(());
        };
        let sessions = base.join("sessions");
        if !sessions.is_dir() {
            return Ok(());
        }
        let transfers = self.root.join("transfers");
        if transfers.is_dir() {
            for entry in transfers.read_dir()? {
                let entry = entry?;
                if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let job: TransferJob = serde_json::from_slice(&std::fs::read(entry.path())?)?;
                if job.finished && job.source.starts_with(&base) {
                    let _ = std::fs::remove_file(&job.source);
                }
            }
        }
        for entry in sessions.read_dir()? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            crate::validate_id(&id)?;
            for name in ["audio.mp3", "audio.wav", "audio.ogg"] {
                let source = entry.path().join(name);
                if source.is_file() {
                    if self.baseline_path(&destination, &id).is_file() {
                        self.enqueue_upload(&id, &source, name, true)?;
                    } else {
                        std::fs::remove_file(source)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn processing_audio(&self, session_id: &str) -> Result<PathBuf> {
        crate::validate_id(session_id)?;
        let root = self
            .scratch_base()
            .ok_or_else(|| Error::new("invalid_request", "select a vault"))?;
        for name in ["audio.mp3", "audio.wav", "audio.ogg"] {
            let path = safe_path(&root, &format!("sessions/{session_id}/{name}"))?;
            if path.is_file() {
                return Ok(path);
            }
        }
        let artifact = self
            .artifacts(session_id)
            .await?
            .into_iter()
            .find(|item| {
                item["filename"]
                    .as_str()
                    .is_some_and(|name| matches!(name, "audio.mp3" | "audio.wav" | "audio.ogg"))
            })
            .ok_or_else(|| Error::new("not_found", "no recording"))?;
        let key = artifact["key"].as_str().unwrap();
        let url = self.media_url(session_id, key).await?;
        let path = safe_path(&root, key)?;
        tokio::fs::create_dir_all(path.parent().unwrap()).await?;
        let partial = path.with_extension("download");
        let mut response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| Error::new("unavailable", error.to_string()))?
            .error_for_status()
            .map_err(|error| Error::new("unavailable", error.to_string()))?;
        let mut file = tokio::fs::File::create(&partial).await?;
        use tokio::io::AsyncWriteExt;
        while let Some(bytes) = response
            .chunk()
            .await
            .map_err(|error| Error::new("unavailable", error.to_string()))?
        {
            file.write_all(&bytes).await?;
        }
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(partial, &path).await?;
        Ok(path)
    }
}

pub async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    if !response.status().is_success() {
        return Err(response
            .json::<Error>()
            .await
            .unwrap_or_else(|_| Error::new("unavailable", "invalid server response")));
    }
    response
        .json()
        .await
        .map_err(|error| Error::new("invalid_data", error.to_string()))
}

pub fn validate_server_url(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| Error::new("invalid_request", "invalid company server URL"))?;
    if url.scheme() != "https"
        && !(url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost")))
    {
        return Err(Error::new(
            "invalid_request",
            "use HTTPS for company servers",
        ));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(Error::new(
            "invalid_request",
            "enter only the company server origin",
        ));
    }
    Ok(url.origin().ascii_serialization())
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

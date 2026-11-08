use axum::{Json, Router, routing::get};
use hypr_loofah_protocol::*;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct IdentityProvider {
    issuer: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for IdentityProvider {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl IdentityProvider {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let metadata = json!({"issuer":issuer,"jwks_uri":format!("{issuer}/keys"),"authorization_endpoint":format!("{issuer}/authorize"),"token_endpoint":format!("{issuer}/token"),"code_challenge_methods_supported":["S256"]});
        let keys: Value = serde_json::from_str(include_str!("jwks.json")).unwrap();
        let router = Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || async move { Json(metadata) }),
            )
            .route("/keys", get(move || async move { Json(keys) }));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { issuer, task }
    }
    fn token(&self, subject: &str) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut header = Header::new(Algorithm::RS256);
        header.typ = Some("at+jwt".into());
        header.kid = Some("test-1".into());
        encode(&header,&json!({"iss":self.issuer,"sub":subject,"aud":"loofah-api","exp":now+3600,"iat":now,"scope":"openid profile loofah:access","name":subject}),&EncodingKey::from_rsa_pem(include_bytes!("oidc-private.pem")).unwrap()).unwrap()
    }
}
struct Server {
    child: tokio::process::Child,
    origin: String,
    client: Client,
    log: PathBuf,
}
impl Server {
    async fn start(root: &Path, storage: &Path, cache: &Path, provider: &IdentityProvider) -> Self {
        let reserved = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = reserved.local_addr().unwrap();
        drop(reserved);
        let id = uuid::Uuid::new_v4().to_string();
        let config = root.join(format!("server-{id}.json"));
        let log = root.join(format!("server-{id}.log"));
        fs::write(&config,serde_json::to_vec(&json!({"storage_root":storage,"cache_root":cache,"listen":address,"public_url":"https://maintenance.example/","oidc":{"issuer":provider.issuer,"audience":"loofah-api","client_id":"loofah-desktop","required_scope":"loofah:access"},"allow_insecure_loopback":true,"processing":{"providers":[{"id":"openai","models":["approved"]}]}})).unwrap()).unwrap();
        let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_loofah-server"))
            .arg("--config")
            .arg(config)
            .current_dir(root)
            .stdout(Stdio::null())
            .stderr(Stdio::from(fs::File::create(&log).unwrap()))
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut server = Self {
            child,
            origin: format!("http://{address}"),
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
            log,
        };
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(status) = server.child.try_wait().unwrap() {
                    panic!(
                        "server exited {status}: {}",
                        fs::read_to_string(&server.log).unwrap()
                    );
                }
                if server
                    .client
                    .get(format!("{}/health", server.origin))
                    .send()
                    .await
                    .is_ok_and(|reply| reply.status() == StatusCode::NO_CONTENT)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("actual service must become healthy");
        server
    }
    fn request(
        &self,
        provider: &IdentityProvider,
        subject: &str,
        method: &str,
        path: &str,
        id: &str,
    ) -> reqwest::RequestBuilder {
        self.client
            .request(method.parse().unwrap(), format!("{}{path}", self.origin))
            .bearer_auth(provider.token(subject))
            .header("x-loofah-api-major", "1")
            .header("x-request-id", id)
    }
    async fn call(
        &self,
        provider: &IdentityProvider,
        subject: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
        id: Option<&str>,
    ) -> (StatusCode, Value) {
        let id = id
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let request = self.request(provider, subject, method, path, &id);
        let request = body.as_ref().map_or_else(
            || request.try_clone().unwrap(),
            |body| request.try_clone().unwrap().json(body),
        );
        let response = request.send().await.unwrap();
        let status = response.status();
        let bytes = response.bytes().await.unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            panic!(
                "unexpected response {status}: {}",
                String::from_utf8_lossy(&bytes)
            )
        });
        (status, value)
    }
    async fn ok(
        &self,
        provider: &IdentityProvider,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Value {
        let (status, value) = self.call(provider, "alice", method, path, body, None).await;
        assert!(status.is_success(), "{method} {path}: {status} {value}");
        value
    }
    async fn stop(mut self) {
        assert!(
            tokio::process::Command::new("/bin/kill")
                .arg("-TERM")
                .arg(self.child.id().unwrap().to_string())
                .status()
                .await
                .unwrap()
                .success()
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(15), self.child.wait())
                .await
                .expect("service shutdown must drain")
                .unwrap()
                .success(),
            "{}",
            fs::read_to_string(self.log).unwrap()
        );
    }
    async fn crash(mut self) {
        self.child.kill().await.unwrap();
        self.child.wait().await.unwrap();
    }
}
async fn maintenance(root: &Path, args: &[&str], success: bool) -> Value {
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_loofah-server"))
        .args(args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .expect("maintenance command deadline")
        .unwrap();
    assert_eq!(
        output.status.success(),
        success,
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("OPERATOR-SECRET-CANARY"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("OPERATOR-SECRET-CANARY"));
    if success {
        serde_json::from_slice(&output.stdout).unwrap()
    } else {
        Value::Null
    }
}
fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
fn inventory(root: &Path) -> BTreeMap<PathBuf, (bool, u32, u64, String)> {
    use std::os::unix::fs::PermissionsExt;
    fn visit(root: &Path, current: &Path, out: &mut BTreeMap<PathBuf, (bool, u32, u64, String)>) {
        for item in fs::read_dir(current).unwrap() {
            let path = item.unwrap().path();
            if path == root.join(".loofah-server.lock") {
                continue;
            }
            let meta = fs::symlink_metadata(&path).unwrap();
            assert!(!meta.file_type().is_symlink());
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let mode = meta.permissions().mode() & 0o777;
            if meta.is_dir() {
                out.insert(relative, (true, mode, 0, String::new()));
                visit(root, &path, out);
            } else {
                assert!(meta.is_file());
                let bytes = fs::read(path).unwrap();
                out.insert(relative, (false, mode, bytes.len() as u64, hash(bytes)));
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}
fn capture(me: &Value, session: &str) -> CaptureIngestRequest {
    CaptureIngestRequest {
        identity: CaptureIdentity {
            scope: CaptureScope {
                server: "https://maintenance.example/".into(),
                member_id: me["member"]["id"].as_str().unwrap().into(),
                vault_id: me["personal_vault_id"].as_str().unwrap().into(),
            },
            capture_id: uuid::Uuid::new_v4().to_string(),
            request_id: uuid::Uuid::new_v4().to_string(),
            target: CaptureTarget::ExistingSession {
                session_id: session.into(),
                expected_audio_sha256: None,
            },
        },
        format: CaptureFormat::stereo(),
        first_admitted_audio_observed_at_ms: 1_794_096_001_123,
    }
}
async fn sealed_capture(
    server: &Server,
    provider: &IdentityProvider,
    input: &CaptureIngestRequest,
) -> (String, CaptureSeal, Value, Vec<u8>) {
    let base = format!("/api/v1/vaults/{}/captures", input.identity.scope.vault_id);
    let (status, value) = server
        .call(
            provider,
            "alice",
            "POST",
            &base,
            Some(serde_json::to_value(input).unwrap()),
            Some(&input.identity.request_id),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let path = format!("{base}/{}", input.identity.capture_id);
    let frames = 160;
    let pcm: Vec<u8> = (0..frames)
        .flat_map(|_| [0.25f32.to_le_bytes(), (-0.5f32).to_le_bytes()].concat())
        .collect();
    let fingerprint = capture_fingerprint(&input.identity, &input.format);
    let mut chunk = CaptureChunkDescriptor {
        capture_fingerprint: fingerprint.clone(),
        sequence: 0,
        first_frame: 0,
        pcm_offset: 0,
        frames,
        bytes: pcm.len() as u32,
        sha256: hash(&pcm),
        previous_digest: capture_chain_seed(&fingerprint),
        digest: String::new(),
    };
    chunk.digest = capture_chunk_digest(&chunk);
    let response = server
        .request(
            provider,
            "alice",
            "PUT",
            &format!("{path}/chunks/0"),
            &uuid::Uuid::new_v4().to_string(),
        )
        .header("content-type", "application/octet-stream")
        .header("x-capture-chunk", serde_json::to_string(&chunk).unwrap())
        .body(pcm.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let prefix: CapturePrefixReceipt = response.json().await.unwrap();
    assert_eq!(prefix.chain_digest, chunk.digest);
    let seal = CaptureSeal {
        capture_fingerprint: fingerprint,
        chunk_count: 1,
        frames: frames.into(),
        pcm_bytes: pcm.len() as u64,
        chain_digest: chunk.digest,
        source_sha256: hash(&pcm),
    };
    let sealed = server
        .ok(
            provider,
            "POST",
            &format!("{path}/seal"),
            Some(serde_json::to_value(&seal).unwrap()),
        )
        .await;
    assert!(sealed["publication"].is_null());
    (path, seal, sealed, pcm)
}

#[tokio::test]
async fn executable_stopped_restore_preserves_scopes_secrets_pending_capture_and_canonical_bytes() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let storage = root.join("storage");
    let provider = IdentityProvider::new().await;
    let server = Server::start(&root, &storage, &root.join("cache"), &provider).await;
    let me = server.ok(&provider, "POST", "/api/v1/me", None).await;
    let vault = me["personal_vault_id"].as_str().unwrap();
    let sessions = format!("/api/v1/vaults/{vault}/sessions");
    let create_id = uuid::Uuid::new_v4().to_string();
    let create_input =
        json!({"title":"Before upgrade","markdown":"Original note with durable create receipt"});
    let (status, created) = server
        .call(
            &provider,
            "alice",
            "POST",
            &sessions,
            Some(create_input.clone()),
            Some(&create_id),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let session = created["meta"]["id"].as_str().unwrap();
    let note_path = format!("{sessions}/{session}/note");
    server.ok(&provider,"PUT",&note_path,Some(json!({"markdown":"Canonical note changed after initial creation","expected_revision":created["note"]["revision"]}))).await;
    let canonical = server.ok(&provider, "GET", &note_path, None).await;
    let input = capture(&me, session);
    let (capture_path, seal, sealed, pcm) = sealed_capture(&server, &provider, &input).await;
    let credentials = format!("/api/v1/vaults/{vault}/processing/providers/openai/credential");
    let initial = server.ok(&provider, "GET", &credentials, None).await;
    let saved = server
        .ok(
            &provider,
            "PUT",
            &credentials,
            Some(
                json!({"expected_revision":initial["revision"],"api_key":"OPERATOR-SECRET-CANARY"}),
            ),
        )
        .await;
    maintenance(
        &root,
        &[
            "backup",
            "--storage-root",
            "storage",
            "--output",
            "busy-snapshot",
            "--secrets-output",
            "busy-private",
        ],
        false,
    )
    .await;
    assert!(!root.join("busy-snapshot").exists());
    server.stop().await;
    let relative = PathBuf::from(format!("vaults/{vault}/sessions/{session}"));
    let directory = storage.join(&relative);
    fs::write(
        directory.join("unknown-private-attachment.bin"),
        b"USER ATTACHMENT\0\xff",
    )
    .unwrap();
    fs::write(
        directory.join(".hidden-user-data"),
        b"HIDDEN USER ATTACHMENT",
    )
    .unwrap();
    fs::create_dir(directory.join("empty-user-directory")).unwrap();
    let before = inventory(&storage);
    let report = maintenance(
        &root,
        &[
            "backup",
            "--storage-root",
            "storage",
            "--output",
            "snapshot",
            "--secrets-output",
            "private",
        ],
        true,
    )
    .await;
    assert_eq!(report["secrets_required"], true);
    let private_path = PathBuf::from(format!(
        "runtime/{vault}/creates/provider-credentials/credentials.json"
    ));
    assert!(!root.join("snapshot/data").join(&private_path).exists());
    assert!(root.join("private/data").join(&private_path).exists());
    for (path, (directory, _, _, _)) in inventory(&root.join("snapshot")) {
        if !directory {
            assert!(
                !fs::read(root.join("snapshot").join(path))
                    .unwrap()
                    .windows(b"OPERATOR-SECRET-CANARY".len())
                    .any(|part| part == b"OPERATOR-SECRET-CANARY")
            );
        }
    }
    maintenance(
        &root,
        &[
            "verify-backup",
            "--snapshot",
            "snapshot",
            "--secrets",
            "private",
        ],
        true,
    )
    .await;
    maintenance(
        &root,
        &[
            "restore",
            "--snapshot",
            "snapshot",
            "--storage-root",
            "missing-secrets",
        ],
        false,
    )
    .await;
    assert!(!root.join("missing-secrets").exists());
    maintenance(
        &root,
        &[
            "backup",
            "--storage-root",
            "storage",
            "--output",
            "other-snapshot",
            "--secrets-output",
            "other-private",
        ],
        true,
    )
    .await;
    maintenance(
        &root,
        &[
            "restore",
            "--snapshot",
            "snapshot",
            "--secrets",
            "other-private",
            "--storage-root",
            "wrong-secrets",
        ],
        false,
    )
    .await;
    assert!(!root.join("wrong-secrets").exists());
    maintenance(
        &root,
        &[
            "restore",
            "--snapshot",
            "snapshot",
            "--secrets",
            "private",
            "--storage-root",
            "restored",
        ],
        true,
    )
    .await;
    let restored = root.join("restored");
    assert_eq!(inventory(&restored), before);
    maintenance(
        &root,
        &[
            "restore",
            "--snapshot",
            "snapshot",
            "--secrets",
            "private",
            "--storage-root",
            "restored",
        ],
        false,
    )
    .await;
    assert_eq!(inventory(&restored), before);
    assert!(!root.join("rebuilt-cache").exists());
    let server = Server::start(&root, &restored, &root.join("rebuilt-cache"), &provider).await;
    assert_eq!(server.ok(&provider, "POST", "/api/v1/me", None).await, me);
    assert_eq!(
        server.ok(&provider, "GET", &note_path, None).await,
        canonical
    );
    let (status, _) = server
        .call(&provider, "bob", "POST", "/api/v1/me", None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        server
            .call(&provider, "bob", "GET", &note_path, None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (status, replayed) = server
        .call(
            &provider,
            "alice",
            "POST",
            &sessions,
            Some(create_input),
            Some(&create_id),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed["meta"]["id"], session);
    assert_eq!(replayed["note"], canonical);
    assert_eq!(
        server.ok(&provider, "GET", &credentials, None).await,
        saved["status"]
    );
    assert_eq!(
        server.ok(&provider, "GET", &capture_path, None).await,
        sealed
    );
    let published = server
        .ok(
            &provider,
            "POST",
            &format!("{capture_path}/publish"),
            Some(serde_json::to_value(&seal).unwrap()),
        )
        .await;
    assert_eq!(published["publication"]["session_id"], session);
    let audio = fs::read(restored.join(&relative).join("audio.wav")).unwrap();
    assert_eq!(
        hash(&audio),
        published["publication"]["canonical_audio_sha256"]
    );
    assert_eq!(
        audio.len() as u64,
        published["publication"]["canonical_audio_bytes"]
            .as_u64()
            .unwrap()
    );
    let wav = hound::WavReader::new(std::io::Cursor::new(audio)).unwrap();
    assert_eq!(wav.spec().channels, 2);
    assert_eq!(wav.spec().sample_rate, 16000);
    let decoded: Vec<u8> = wav
        .into_samples::<f32>()
        .flat_map(|sample| sample.unwrap().to_le_bytes())
        .collect();
    assert_eq!(decoded, pcm);
    assert_eq!(
        fs::read(restored.join(&private_path)).unwrap(),
        fs::read(storage.join(&private_path)).unwrap()
    );
    assert_eq!(
        fs::read(
            restored
                .join(&relative)
                .join("unknown-private-attachment.bin")
        )
        .unwrap(),
        b"USER ATTACHMENT\0\xff"
    );
    assert_eq!(
        fs::read(restored.join(&relative).join(".hidden-user-data")).unwrap(),
        b"HIDDEN USER ATTACHMENT"
    );
    assert!(
        restored
            .join(&relative)
            .join("empty-user-directory")
            .is_dir()
    );
    assert!(root.join("rebuilt-cache").is_dir());
    server.stop().await;
    assert_eq!(inventory(&storage), before);
    maintenance(
        &root,
        &[
            "restore",
            "--snapshot",
            "snapshot",
            "--secrets",
            "private",
            "--storage-root",
            "rollback",
        ],
        true,
    )
    .await;
    assert_eq!(inventory(&root.join("rollback")), before);
    let rollback = Server::start(
        &root,
        &root.join("rollback"),
        &root.join("rollback-cache"),
        &provider,
    )
    .await;
    assert_eq!(
        rollback.ok(&provider, "GET", &note_path, None).await,
        canonical
    );
    assert_eq!(
        rollback.ok(&provider, "GET", &capture_path, None).await,
        sealed
    );
    assert!(
        !root
            .join("rollback")
            .join(&relative)
            .join("audio.wav")
            .exists()
    );
    rollback.stop().await;
    let damaged = root.join("snapshot/data").join(&relative).join("notes.md");
    let original = fs::read(&damaged).unwrap();
    fs::write(&damaged, b"corrupted backup").unwrap();
    maintenance(
        &root,
        &[
            "restore",
            "--snapshot",
            "snapshot",
            "--secrets",
            "private",
            "--storage-root",
            "corrupt-target",
        ],
        false,
    )
    .await;
    assert!(!root.join("corrupt-target").exists());
    fs::write(damaged, original).unwrap();
    assert_eq!(inventory(&storage), before);
}

#[tokio::test]
async fn executable_restart_recovers_known_legacy_operation_and_refuses_future_operation_without_data_loss()
 {
    for version in [1, 999] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let storage = root.join("storage");
        let provider = IdentityProvider::new().await;
        let server = Server::start(&root, &storage, &root.join("cache"), &provider).await;
        let me = server.ok(&provider, "POST", "/api/v1/me", None).await;
        let vault = me["personal_vault_id"].as_str().unwrap();
        let created = server
            .ok(
                &provider,
                "POST",
                &format!("/api/v1/vaults/{vault}/sessions"),
                Some(json!({"title":"Legacy operation","markdown":"Unmodified user note"})),
            )
            .await;
        let session = created["meta"]["id"].as_str().unwrap();
        server.stop().await;
        let vault_path = storage.join("vaults").join(vault);
        let directory = vault_path.join("sessions").join(session);
        let store = hypr_vault_write::SessionStore::new(vault_path.clone());
        store.ensure_summary(session).await.unwrap();
        store
            .update_generated_summary(session, "- [ ] Call Alice", Some(""))
            .await
            .unwrap();
        let before = store
            .read_document_snapshot(session, &DocumentTarget::Summary)
            .await
            .unwrap()
            .unwrap();
        let mut tasks = store.session_tasks(session).await.unwrap();
        assert_eq!(tasks.len(), 1);
        tasks[0].status = "done".into();
        let original_tasks = fs::read(directory.join("tasks.json")).unwrap();
        let mut guarded_tasks = vec![1];
        guarded_tasks.extend(&original_tasks);
        fs::write(directory.join("summary.md"), "- [x] Call Alice").unwrap();
        let after = store
            .read_document_snapshot(session, &DocumentTarget::Summary)
            .await
            .unwrap()
            .unwrap();
        drop(store);
        let pending = PathBuf::from(format!(
            "runtime/{vault}/creates/document-saves/{}.json",
            hash(session.as_bytes())
        ));
        let intent = json!({"schema_version":version,"session_id":session,"target":DocumentTarget::Summary,"before_revision":before.revision,"after_revision":after.revision,"before_tasks_hash":hash(guarded_tasks),"tasks":tasks});
        hypr_storage::fs::atomic_write(&storage.join(&pending), &intent.to_string()).unwrap();
        let original = inventory(&storage);
        maintenance(
            &root,
            &[
                "backup",
                "--storage-root",
                "storage",
                "--output",
                "snapshot",
                "--secrets-output",
                "private",
            ],
            true,
        )
        .await;
        maintenance(
            &root,
            &[
                "restore",
                "--snapshot",
                "snapshot",
                "--storage-root",
                "restored",
            ],
            true,
        )
        .await;
        assert_eq!(inventory(&root.join("restored")), original);
        // A killed upgrade process must not consume a receipt merely by reporting global health.
        Server::start(
            &root,
            &root.join("restored"),
            &root.join("new-cache"),
            &provider,
        )
        .await
        .crash()
        .await;
        assert_eq!(
            fs::read(root.join("restored").join(&pending)).unwrap(),
            intent.to_string().as_bytes()
        );
        let restored = Server::start(
            &root,
            &root.join("restored"),
            &root.join("new-cache"),
            &provider,
        )
        .await;
        let (status, documents) = restored
            .call(
                &provider,
                "alice",
                "GET",
                &format!("/api/v1/vaults/{vault}/sessions/{session}/documents"),
                None,
                None,
            )
            .await;
        if version == 1 {
            assert_eq!(status, StatusCode::OK, "{documents}");
            assert!(documents.to_string().contains("- [x] Call Alice"));
            assert_eq!(
                serde_json::from_slice::<Value>(
                    &fs::read(
                        root.join("restored/vaults")
                            .join(vault)
                            .join("sessions")
                            .join(session)
                            .join("tasks.json")
                    )
                    .unwrap()
                )
                .unwrap()["tasks"],
                serde_json::to_value(&tasks).unwrap()
            );
            assert!(!root.join("restored").join(&pending).exists());
        } else {
            assert!(
                !status.is_success(),
                "future operation must fail vault readiness: {documents}"
            );
            assert_eq!(
                fs::read(root.join("restored").join(&pending)).unwrap(),
                intent.to_string().as_bytes()
            );
            assert_eq!(
                fs::read(
                    root.join("restored/vaults")
                        .join(vault)
                        .join("sessions")
                        .join(session)
                        .join("tasks.json")
                )
                .unwrap(),
                original_tasks
            );
        }
        restored.stop().await;
        assert_eq!(inventory(&storage), original);
    }
}

use std::sync::{Arc, Mutex};

use hypr_vault_access::{
    Call, Credentials, Result, ServerInfo, Versions,
    connection::{ConnectionState, CredentialStore},
    digest,
};
use loofah_server::{Config, Server, content, http::router, registry::Session};
use serde_json::json;

#[derive(Default)]
struct RequestGate {
    target: Mutex<Option<(String, String)>>,
    pause: std::sync::atomic::AtomicBool,
    attempts: std::sync::atomic::AtomicUsize,
    bodies: Mutex<Vec<String>>,
    rejection: std::sync::atomic::AtomicU16,
    arrived: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl RequestGate {
    fn arm(&self, method: &str, path: &str, pause: bool) {
        *self.target.lock().unwrap() = Some((method.into(), path.into()));
        self.attempts.store(0, std::sync::atomic::Ordering::Release);
        self.bodies.lock().unwrap().clear();
        self.pause
            .store(pause, std::sync::atomic::Ordering::Release);
    }
}

struct Live {
    _data: tempfile::TempDir,
    device: tempfile::TempDir,
    server: Arc<Server>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    client: ConnectionState,
    secrets: Arc<Secrets>,
    info: ServerInfo,
    credentials: Credentials,
    lose_upload_reply: Arc<std::sync::atomic::AtomicBool>,
    gate: Arc<RequestGate>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Live {
    async fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let device = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = Server::open(Config {
            data_dir: data.path().into(),
            public_url: format!("http://{address}"),
            issuer: "http://127.0.0.1:8948".into(),
            client_id: "test".into(),
            client_secret: None,
            max_upload_bytes: 20 * 1024 * 1024,
        })
        .unwrap();
        let user = server
            .provision("issuer", "owner", "Owner", "owner@example.com")
            .await
            .unwrap();
        let vault = server.create_vault(&user, "Team").await.unwrap();
        let expires = chrono::Utc::now().timestamp() + 3600;
        server.registry.write().await.sessions.insert(
            digest(b"refresh"),
            Session {
                user_id: user.id.clone(),
                access_hash: digest(b"access"),
                refresh_hash: digest(b"refresh"),
                access_expires: expires,
                expires: expires + 3600,
            },
        );
        server
            .save_registry(&*server.registry.read().await)
            .await
            .unwrap();
        let info = ServerInfo {
            id: server.registry.read().await.server_id.clone(),
            api_version: 1,
            authentication: "oidc".into(),
        };
        let credentials = Credentials {
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: expires,
            user,
        };
        let lose_upload_reply = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fault = lose_upload_reply.clone();
        let gate = Arc::new(RequestGate::default());
        let control = gate.clone();
        let app=router(server.clone()).layer(axum::middleware::from_fn(move |request:axum::extract::Request,next:axum::middleware::Next| {
            let fault=fault.clone();
            let control=control.clone();
            async move {
                let mut request=request;
                let matches=control.target.lock().unwrap().as_ref().is_some_and(|(method,path)| (method.is_empty() || method == request.method().as_str()) && request.uri().path().contains(path));
                if matches {
                    control.attempts.fetch_add(1,std::sync::atomic::Ordering::AcqRel);
                    if request.method() == axum::http::Method::POST {
                        let (parts,body)=request.into_parts();
                        let bytes=axum::body::to_bytes(body,16*1024*1024).await.unwrap();
                        control.bodies.lock().unwrap().push(digest(&bytes));
                        request=axum::extract::Request::from_parts(parts,axum::body::Body::from(bytes));
                    }
                    if control.pause.swap(false,std::sync::atomic::Ordering::AcqRel) {
                        control.arrived.notify_one();
                        control.release.notified().await;
                    }
                }
                let upload=request.method() == axum::http::Method::PUT && request.uri().path().contains("/uploads/");
                let rejection=control.rejection.load(std::sync::atomic::Ordering::Acquire);
                if matches && rejection != 0 {
                    use axum::response::IntoResponse;
                    let code=if rejection == 401 {"unauthenticated"}else{"unavailable"};
                    return (axum::http::StatusCode::from_u16(rejection).unwrap(),axum::Json(json!({"code":code,"message":"test rejection"}))).into_response();
                }
                let response=next.run(request).await;
                if upload && response.status().is_success() && fault.swap(false,std::sync::atomic::Ordering::AcqRel) {
                    use axum::response::IntoResponse;
                    return (axum::http::StatusCode::SERVICE_UNAVAILABLE,axum::Json(json!({"code":"unavailable","message":"test proxy lost the response"}))).into_response();
                }
                response
            }
        }));
        let task = tokio::spawn(axum::serve(listener, app).into_future());
        let secrets = Arc::new(Secrets::default());
        let client = ConnectionState::open(device.path().into(), secrets.clone()).unwrap();
        client
            .connect(
                format!("http://{address}"),
                info.clone(),
                credentials.clone(),
            )
            .await
            .unwrap();
        client.select(Some(vault.id)).await.unwrap();
        client.call("session_write_meta",json!({"meta":{"id":"s1","title":"Meeting","created_at":"2026-10-18T10:00:00Z","tags":[]}}),false).await.unwrap();
        Self {
            _data: data,
            device,
            server,
            task,
            client,
            secrets,
            info,
            credentials,
            lose_upload_reply,
            gate,
        }
    }

    fn restart_client(&mut self) {
        self.client =
            ConnectionState::open(self.device.path().into(), self.secrets.clone()).unwrap();
    }
}

#[tokio::test]
async fn in_flight_requests_survive_token_rotation_without_duplicate_writes() {
    let mut failures = Vec::new();
    for operation in ["request", "query", "save", "create", "chunk"] {
        let f = Live::new().await;
        let source = f.device.path().join("source.bin");
        std::fs::write(&source, b"upload during token rotation").unwrap();
        let (method, path) = match operation {
            "request" => ("GET", "/v1/me"),
            "create" => ("POST", "/v1/vaults"),
            "chunk" => ("PUT", "/uploads/"),
            _ => ("POST", "/invoke"),
        };
        f.gate.arm(method, path, true);
        let request = async {
            match operation {
                "request" => f.client.request("GET", "/v1/me", None).await,
                "query" => {
                    f.client
                        .call("session_get", json!({"session_id":"s1"}), true)
                        .await
                }
                "save" => f
                    .client
                    .call(
                        "session_save_note",
                        json!({"session_id":"s1","markdown":"Saved during rotation","title":null}),
                        false,
                    )
                    .await,
                "create" => {
                    f.client
                        .request(
                            "POST",
                            "/v1/vaults",
                            Some(json!({"name":"Created during rotation"})),
                        )
                        .await
                }
                "chunk" => f.client.upload("s1", &source, "rotation.bin", false).await,
                _ => unreachable!(),
            }
        };
        let rotate = async {
            tokio::time::timeout(std::time::Duration::from_secs(5), f.gate.arrived.notified())
                .await
                .unwrap();
            let credentials = f.client.refresh_rejected_access("access").await.unwrap().1;
            f.gate.release.notify_one();
            credentials
        };
        let (result, credentials) =
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                tokio::join!(request, rotate)
            })
            .await
            .unwrap();
        if let Err(error) = result {
            failures.push(format!("{operation}: {error}"));
            continue;
        }
        assert_eq!(
            f.gate.attempts.load(std::sync::atomic::Ordering::Acquire),
            2,
            "{operation}"
        );
        if method == "POST" {
            let bodies = f.gate.bodies.lock().unwrap();
            assert_eq!(bodies.len(), 2);
            assert_eq!(
                bodies[0], bodies[1],
                "retries must preserve payloads and mutation IDs"
            );
        }
        assert_eq!(
            f.client.authenticated().await.unwrap().1.access_token,
            credentials.access_token
        );
        assert!(f.client.pending().unwrap().is_empty());
        match operation {
            "save" => {
                let profile = f.client.profile().unwrap();
                let vault = profile.vault_id.unwrap();
                assert_eq!(
                    std::fs::read_to_string(
                        f._data
                            .path()
                            .join(format!("vaults/{vault}/sessions/s1/notes.md"))
                    )
                    .unwrap(),
                    "Saved during rotation"
                );
                assert_eq!(
                    f._data
                        .path()
                        .join(format!("service/vaults/{vault}/journal"))
                        .read_dir()
                        .unwrap()
                        .count(),
                    2
                );
            }
            "create" => assert_eq!(
                f.server
                    .registry
                    .read()
                    .await
                    .vaults
                    .values()
                    .filter(|v| v.name == "Created during rotation")
                    .count(),
                1
            ),
            "chunk" => {
                assert_eq!(
                    f.client
                        .read_artifact("s1", "sessions/s1/attachments/rotation.bin")
                        .await
                        .unwrap(),
                    b"upload during token rotation"
                );
                assert!(f.client.transfers().unwrap().is_empty());
            }
            _ => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn a_forbidden_write_does_not_rotate_credentials_or_retry() {
    let f = Live::new().await;
    let vault = f.client.profile().unwrap().vault_id.unwrap();
    let other = f
        .server
        .provision("issuer", "other", "Other", "other@example.com")
        .await
        .unwrap();
    f.server
        .update_member(
            &f.credentials.user,
            &vault,
            &other.id,
            Some(hypr_vault_access::Role::Owner),
        )
        .await
        .unwrap();
    f.server
        .update_member(
            &other,
            &vault,
            &f.credentials.user.id,
            Some(hypr_vault_access::Role::Viewer),
        )
        .await
        .unwrap();
    f.gate.arm("POST", "/invoke", false);
    assert_eq!(
        f.client
            .call(
                "session_save_note",
                json!({"session_id":"s1","markdown":"Cannot write","title":null}),
                false
            )
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        f.gate.attempts.load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        f.client.authenticated().await.unwrap().1.access_token,
        "access"
    );
}

#[tokio::test]
async fn a_late_query_cannot_seed_another_vaults_revision_baselines() {
    let f = Live::new().await;
    let other = f
        .server
        .create_vault(&f.credentials.user, "Other vault")
        .await
        .unwrap();
    content::invoke(f.server.clone(),&f.credentials.user,&other.id,Call::mutation("session_write_meta",json!({"meta":{"id":"s1","title":"Other meeting","created_at":"2026-10-18T10:00:00Z","tags":[]}}),Versions::new())).await.unwrap();
    content::invoke(
        f.server.clone(),
        &f.credentials.user,
        &other.id,
        Call::mutation(
            "session_save_note",
            json!({"session_id":"s1","markdown":"Other vault text","title":null}),
            Versions::new(),
        ),
    )
    .await
    .unwrap();
    f.gate.arm("POST", "/invoke", true);
    let read = f
        .client
        .call("session_get", json!({"session_id":"s1"}), true);
    let switch = async {
        tokio::time::timeout(std::time::Duration::from_secs(5), f.gate.arrived.notified())
            .await
            .unwrap();
        f.client.select(Some(other.id)).await.unwrap();
        f.gate.release.notify_one();
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(read, switch)
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().code, "conflict");
    assert_eq!(
        f.client
            .call("session_get", json!({"session_id":"s1"}), true)
            .await
            .unwrap()["note_markdown"],
        "Other vault text"
    );
    f.client
        .call(
            "session_save_note",
            json!({"session_id":"s1","markdown":"Edited in the new vault","title":null}),
            false,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn rejected_requests_never_refresh_or_retarget_a_different_account_or_server() {
    for operation in ["request", "query", "chunk"] {
        let f = Live::new().await;
        let other = Live::new().await;
        other.gate.arm("", "/v1/", false);
        let source = f.device.path().join("source.bin");
        std::fs::write(&source, b"private upload for the original server").unwrap();
        let (method, path) = match operation {
            "request" => ("GET", "/v1/me"),
            "query" => ("POST", "/invoke"),
            _ => ("PUT", "/uploads/"),
        };
        f.gate.arm(method, path, true);
        let request = async {
            match operation {
                "request" => f.client.request("GET", "/v1/me", None).await,
                "query" => {
                    f.client
                        .call("session_get", json!({"session_id":"s1"}), true)
                        .await
                }
                _ => f.client.upload("s1", &source, "private.bin", false).await,
            }
        };
        let switch = async {
            tokio::time::timeout(std::time::Duration::from_secs(5), f.gate.arrived.notified())
                .await
                .unwrap();
            f.server.registry.write().await.sessions.clear();
            f.client.select(None).await.unwrap();
            f.client
                .connect(
                    other.client.profile().unwrap().base_url,
                    other.info.clone(),
                    other.credentials.clone(),
                )
                .await
                .unwrap();
            f.gate.release.notify_one();
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(request, switch)
        })
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "conflict", "{operation}");
        assert_eq!(
            other
                .gate
                .attempts
                .load(std::sync::atomic::Ordering::Acquire),
            0,
            "old requests must never contact the new identity's server"
        );
        assert_eq!(
            f.gate.attempts.load(std::sync::atomic::Ordering::Acquire),
            1
        );
        assert!(source.exists());
    }
}

#[tokio::test]
async fn authentication_retries_are_bounded_and_server_errors_do_not_rotate_credentials() {
    for status in [401, 503] {
        let f = Live::new().await;
        f.gate.arm("GET", "/v1/me", false);
        f.gate
            .rejection
            .store(status, std::sync::atomic::Ordering::Release);
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            f.client.request("GET", "/v1/me", None),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(
            error.code,
            if status == 401 {
                "unauthenticated"
            } else {
                "unavailable"
            }
        );
        assert_eq!(
            f.gate.attempts.load(std::sync::atomic::Ordering::Acquire),
            if status == 401 { 2 } else { 1 }
        );
        let token = f.client.authenticated().await.unwrap().1.access_token;
        assert_eq!(token == "access", status != 401);
    }
}

#[tokio::test]
async fn a_failed_in_flight_refresh_cannot_revoke_a_new_connection() {
    for query in [false, true] {
        let f = Live::new().await;
        let other = Live::new().await;
        other.gate.arm("", "/v1/", false);
        f.server
            .registry
            .write()
            .await
            .sessions
            .get_mut(&digest(b"refresh"))
            .unwrap()
            .access_expires = 0;
        f.gate.arm("POST", "/v1/auth/refresh", true);
        let request = async {
            if query {
                f.client
                    .call("session_get", json!({"session_id":"s1"}), true)
                    .await
            } else {
                f.client.request("GET", "/v1/me", None).await
            }
        };
        let switch = async {
            tokio::time::timeout(std::time::Duration::from_secs(5), f.gate.arrived.notified())
                .await
                .unwrap();
            f.server.registry.write().await.sessions.clear();
            f.client.select(None).await.unwrap();
            let mut connect = Box::pin(f.client.connect(
                other.client.profile().unwrap().base_url,
                other.info.clone(),
                other.credentials.clone(),
            ));
            let waiting = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(std::future::Future::poll(connect.as_mut(), cx).is_pending())
            })
            .await;
            assert!(waiting);
            assert_eq!(
                f.client.profile().unwrap().user_id,
                other.credentials.user.id
            );
            f.gate.release.notify_one();
            connect.await.unwrap();
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(request, switch)
        })
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "conflict");
        assert_eq!(
            other
                .gate
                .attempts
                .load(std::sync::atomic::Ordering::Acquire),
            0
        );
        assert_eq!(
            f.client.authenticated().await.unwrap().1.user.id,
            other.credentials.user.id
        );
    }
}

#[derive(Default)]
struct Secrets(Mutex<std::collections::BTreeMap<String, Credentials>>);
impl CredentialStore for Secrets {
    fn load(&self, key: &str) -> Result<Option<Credentials>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn save(&self, key: &str, value: &Credentials) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), value.clone());
        Ok(())
    }
    fn remove(&self, key: &str) -> Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

#[tokio::test]
async fn credentials_rotate_once_for_concurrent_requests_and_remain_in_the_secret_store() {
    let mut f = Live::new().await;
    let profile = f.client.profile().unwrap();
    let mut credentials = f.credentials.clone();
    credentials.expires_at = 0;
    f.client
        .connect(profile.base_url.clone(), f.info.clone(), credentials)
        .await
        .unwrap();
    let (a, b) = tokio::join!(f.client.authenticated(), f.client.authenticated());
    let a = a.unwrap().1;
    let b = b.unwrap().1;
    assert_eq!(a.access_token, b.access_token);
    assert_ne!(a.access_token, "access");
    assert_eq!(f.server.registry.read().await.sessions.len(), 1);
    assert_eq!(
        f.secrets
            .load(&profile.credential_key())
            .unwrap()
            .unwrap()
            .access_token,
        a.access_token
    );
    let persisted = std::fs::read_to_string(f.device.path().join("connection.json")).unwrap();
    assert!(!persisted.contains(&a.access_token));
    assert!(!persisted.contains(&a.refresh_token));
    f.restart_client();
    assert_eq!(
        f.client.authenticated().await.unwrap().1.access_token,
        a.access_token
    );
    f.client.logout().await.unwrap();
    assert!(f.client.profile().is_none());
    assert!(f.secrets.load(&profile.credential_key()).unwrap().is_none());
    assert!(f.server.registry.read().await.sessions.is_empty());
    assert_eq!(
        f.client.authenticated().await.unwrap_err().code,
        "unauthenticated"
    );
}

#[tokio::test]
async fn recording_processing_and_account_guards_preserve_the_selected_destination() {
    let f = Live::new().await;
    let profile = f.client.profile().unwrap();
    f.client
        .recording
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(f.client.select(None).await.unwrap_err().code, "conflict");
    assert_eq!(f.client.logout().await.unwrap_err().code, "conflict");
    assert_eq!(
        f.client
            .connect(
                profile.base_url.clone(),
                f.info.clone(),
                f.credentials.clone()
            )
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    f.client
        .recording
        .store(false, std::sync::atomic::Ordering::Release);
    let lease = f.client.begin_processing().await;
    assert_eq!(f.client.logout().await.unwrap_err().code, "conflict");
    f.client.end_processing(&lease);
    let mut another = f.credentials.clone();
    another.user.id = "another-account".into();
    assert_eq!(
        f.client
            .connect(profile.base_url.clone(), f.info.clone(), another)
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(
        f.client
            .select(Some("not-a-vault".into()))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    for path in [
        "https://attacker.invalid/v1/me",
        "//attacker.invalid/v1/me",
        "/v1/../secret",
        "/v1/a\\b",
        "/v1/me#fragment",
    ] {
        assert_eq!(
            f.client.request("GET", path, None).await.unwrap_err().code,
            "invalid_request"
        );
    }
    assert_eq!(f.client.profile().unwrap().vault_id, profile.vault_id);
    f.client.select(None).await.unwrap();
    assert!(!f.client.active());
    assert_eq!(
        f.client
            .call("session_ids", json!({}), true)
            .await
            .unwrap_err()
            .code,
        "invalid_request"
    );
}

#[tokio::test]
async fn concurrent_access_rejections_refresh_once_and_stale_rejections_keep_the_new_credentials() {
    let f = Live::new().await;
    let (a, b) = tokio::join!(
        f.client.refresh_rejected_access("access"),
        f.client.refresh_rejected_access("access")
    );
    let a = a.unwrap().1;
    let b = b.unwrap().1;
    assert_eq!(a.access_token, b.access_token);
    assert_ne!(a.access_token, "access");
    assert_eq!(
        f.client
            .refresh_rejected_access("access")
            .await
            .unwrap()
            .1
            .access_token,
        a.access_token
    );
    assert_eq!(
        f.client.request("GET", "/v1/me", None).await.unwrap()["id"],
        f.credentials.user.id
    );
    assert_eq!(f.server.registry.read().await.sessions.len(), 1);
}

#[tokio::test]
async fn lost_chunk_reply_resumes_after_restart_without_resending_or_corrupting_committed_bytes() {
    let mut f = Live::new().await;
    let source = f.device.path().join("source.bin");
    let data = vec![0x5a; 8 * 1024 * 1024 + 19];
    std::fs::write(&source, &data).unwrap();
    f.lose_upload_reply
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        f.client
            .upload("s1", &source, "document.bin", false)
            .await
            .unwrap_err()
            .code,
        "unavailable"
    );
    assert_eq!(f.client.transfers().unwrap().len(), 1);
    assert!(f.client.artifacts("s1").await.unwrap().is_empty());
    let uploads = f.device.path().join("uploads");
    let state = uploads.read_dir().unwrap().next().unwrap().unwrap().path();
    let begin: serde_json::Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
    let profile = f.client.profile().unwrap();
    assert_eq!(
        f.client
            .request(
                "GET",
                &format!(
                    "/v1/vaults/{}/uploads/{}",
                    profile.vault_id.unwrap(),
                    begin["id"].as_str().unwrap()
                ),
                None
            )
            .await
            .unwrap()["offset"],
        8 * 1024 * 1024
    );
    f.restart_client();
    f.client.retry_uploads().await.unwrap();
    assert!(f.client.transfers().unwrap().is_empty());
    let key = "sessions/s1/attachments/document.bin";
    assert_eq!(f.client.read_artifact("s1", key).await.unwrap(), data);
    assert!(source.exists());
    assert_eq!(uploads.read_dir().unwrap().count(), 0);
    f.client
        .enqueue_upload("s1", &source, "document.bin", false)
        .unwrap();
    assert!(f.client.transfers().unwrap().is_empty());
    f.client.remove_artifact("s1", key).await.unwrap();
    assert!(f.client.artifacts("s1").await.unwrap().is_empty());
    f.client.remove_artifact("s1", key).await.unwrap();
}

#[tokio::test]
async fn recording_recovery_keeps_unsent_audio_and_cleans_only_acknowledged_or_downloaded_files() {
    let mut f = Live::new().await;
    let scratch = f.client.scratch_base().unwrap();
    let source = scratch.join("sessions/s1/audio.wav");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, b"not yet prepared").unwrap();
    assert_eq!(
        f.client
            .enqueue_upload("s1", &source, "audio.wav", true)
            .unwrap_err()
            .code,
        "precondition_required"
    );
    f.client
        .call(
            "session_prepare_recording",
            json!({"session_id":"s1"}),
            false,
        )
        .await
        .unwrap();
    assert!(!source.exists());
    std::fs::write(&source, b"captured audio").unwrap();
    f.restart_client();
    assert_eq!(f.client.transfers().unwrap().len(), 1);
    assert_eq!(
        std::fs::read(f.client.processing_audio("s1").await.unwrap()).unwrap(),
        b"captured audio"
    );
    assert_eq!(
        f.client
            .call(
                "session_prepare_recording",
                json!({"session_id":"s1"}),
                false
            )
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    f.client.retry_uploads().await.unwrap();
    assert!(source.exists());
    assert!(f.client.transfers().unwrap().is_empty());
    f.restart_client();
    assert!(!source.exists());
    let downloaded = f.client.processing_audio("s1").await.unwrap();
    assert_eq!(std::fs::read(&downloaded).unwrap(), b"captured audio");
    let peaks = f.device.path().join("peaks.json");
    std::fs::write(&peaks, b"[0,1,0]").unwrap();
    assert_eq!(
        f.client
            .publish_peaks("s1", &peaks, "audio.wav", "wrong")
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    f.client
        .publish_peaks("s1", &peaks, "audio.wav", &digest(b"captured audio"))
        .await
        .unwrap();
    assert_eq!(
        f.client
            .read_artifact("s1", "sessions/s1/audio.peaks.json")
            .await
            .unwrap(),
        b"[0,1,0]"
    );
    assert_eq!(f.client.artifacts("s1").await.unwrap().len(), 2);
    f.client
        .call(
            "session_prepare_recording",
            json!({"session_id":"s1"}),
            false,
        )
        .await
        .unwrap();
    assert!(source.exists());
    assert!(!f.device.path().join("sessions").exists());
}

#[tokio::test]
async fn pending_uploads_remain_account_scoped_and_can_be_explicitly_discarded() {
    let f = Live::new().await;
    let source = f.device.path().join("source");
    std::fs::write(&source, b"keep this file").unwrap();
    f.client
        .enqueue_upload("s1", &source, "file.txt", false)
        .unwrap();
    let job = f.client.transfers().unwrap().remove(0).1;
    let profile = f.client.profile().unwrap();
    f.client.select(None).await.unwrap();
    let mut credentials = f.credentials.clone();
    credentials.user.id = "other-user".into();
    f.client
        .connect(profile.base_url.clone(), f.info.clone(), credentials)
        .await
        .unwrap();
    assert!(f.client.transfers().unwrap().is_empty());
    assert_eq!(
        f.client.discard_transfer(&job.id).await.unwrap_err().code,
        "not_found"
    );
    f.client
        .connect(profile.base_url, f.info.clone(), f.credentials.clone())
        .await
        .unwrap();
    f.client.discard_transfer(&job.id).await.unwrap();
    assert!(f.client.transfers().unwrap().is_empty());
    assert_eq!(std::fs::read(source).unwrap(), b"keep this file");
}

#[tokio::test]
async fn offline_draft_survives_restart_stays_scoped_and_requires_review_to_replace() {
    let data = tempfile::tempdir().unwrap();
    let device = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = Config {
        data_dir: data.path().into(),
        public_url: format!("http://{address}"),
        issuer: "http://127.0.0.1:8948".into(),
        client_id: "test".into(),
        client_secret: None,
        max_upload_bytes: 1024,
    };
    let server = Server::open(config.clone()).unwrap();
    let owner = server
        .provision("issuer", "owner", "Owner", "owner@example.com")
        .await
        .unwrap();
    let a = server.create_vault(&owner, "A").await.unwrap();
    let b = server.create_vault(&owner, "B").await.unwrap();
    let expires = chrono::Utc::now().timestamp() + 3600;
    {
        let mut registry = server.registry.write().await;
        registry.sessions.insert(
            "test".into(),
            Session {
                user_id: owner.id.clone(),
                access_hash: digest(b"access"),
                refresh_hash: digest(b"refresh"),
                access_expires: expires,
                expires,
            },
        );
        server.save_registry(&registry).await.unwrap();
    }
    let server_id = server.registry.read().await.server_id.clone();
    let task = tokio::spawn(axum::serve(listener, router(server.clone())).into_future());
    let secrets = Arc::new(Secrets::default());
    let client = ConnectionState::open(device.path().into(), secrets.clone()).unwrap();
    client
        .connect(
            config.public_url.clone(),
            ServerInfo {
                id: server_id.clone(),
                api_version: 1,
                authentication: "oidc".into(),
            },
            Credentials {
                access_token: "access".into(),
                refresh_token: "refresh".into(),
                expires_at: expires,
                user: owner.clone(),
            },
        )
        .await
        .unwrap();
    let server_info = ServerInfo {
        id: server_id.clone(),
        api_version: 1,
        authentication: "oidc".into(),
    };
    let credentials = Credentials {
        access_token: "access".into(),
        refresh_token: "refresh".into(),
        expires_at: expires,
        user: owner.clone(),
    };
    assert_eq!(
        client
            .connect(
                "https://another-server.invalid".into(),
                server_info.clone(),
                credentials.clone()
            )
            .await
            .unwrap_err()
            .code,
        "invalid_request"
    );
    let mut invalid = server_info;
    invalid.id = "../escape".into();
    assert_eq!(
        client
            .connect("http://127.0.0.1:8947".into(), invalid, credentials)
            .await
            .unwrap_err()
            .code,
        "invalid_request"
    );
    client.select(Some(a.id.clone())).await.unwrap();
    client.call("session_write_meta",json!({"meta":{"id":"s1","title":"Note","created_at":"2026-10-18T10:00:00Z","started_at":null,"ended_at":null,"tags":[]}}),false).await.unwrap();
    client
        .call(
            "session_save_note",
            json!({"session_id":"s1","markdown":"original","title":null}),
            false,
        )
        .await
        .unwrap();
    client
        .call("session_get", json!({"session_id":"s1"}), true)
        .await
        .unwrap();
    let lease = client.begin_processing().await;
    assert_eq!(
        client.select(Some(b.id.clone())).await.unwrap_err().code,
        "conflict"
    );
    client.end_processing(&lease);
    task.abort();
    let _ = task.await;
    assert_eq!(
        client
            .call(
                "session_save_note",
                json!({"session_id":"s1","markdown":"offline draft","title":null}),
                false
            )
            .await
            .unwrap_err()
            .code,
        "unavailable"
    );
    assert_eq!(client.pending().unwrap().len(), 1);
    drop(client);
    drop(server);
    let server = Server::open(config).unwrap();
    let current = content::invoke(
        server.clone(),
        &owner,
        &a.id,
        Call::query("session_get", json!({"session_id":"s1"})),
    )
    .await
    .unwrap();
    content::invoke(
        server.clone(),
        &owner,
        &a.id,
        Call::mutation(
            "session_save_note",
            json!({"session_id":"s1","markdown":"another device","title":null}),
            current.versions,
        ),
    )
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let task = tokio::spawn(axum::serve(listener, router(server.clone())).into_future());
    let client = ConnectionState::open(device.path().into(), secrets.clone()).unwrap();
    assert_eq!(
        client.pending().unwrap()[0].call.args["markdown"],
        "offline draft"
    );
    client.select(Some(b.id.clone())).await.unwrap();
    client.retry().await.unwrap();
    assert!(client.pending().unwrap().is_empty());
    assert_eq!(
        client
            .call("session_get", json!({"session_id":"s1"}), true)
            .await
            .unwrap(),
        serde_json::Value::Null
    );
    client.select(Some(a.id.clone())).await.unwrap();
    assert_eq!(client.retry().await.unwrap_err().code, "conflict");
    let pending = client.pending().unwrap().remove(0);
    assert_eq!(pending.call.args["markdown"], "offline draft");
    let preview = content::invoke(
        server.clone(),
        &owner,
        &a.id,
        Call::query("session_get", json!({"session_id":"s1"})),
    )
    .await
    .unwrap();
    assert_eq!(preview.data["note_markdown"], "another device");
    let id = pending.call.mutation_id.unwrap();
    assert!(
        client
            .resolve(&id, "replace", Versions::new())
            .await
            .is_err()
    );
    client
        .resolve(&id, "replace", preview.versions)
        .await
        .unwrap();
    assert!(client.pending().unwrap().is_empty());
    assert_eq!(
        client
            .call("session_get", json!({"session_id":"s1"}), true)
            .await
            .unwrap()["note_markdown"],
        "offline draft"
    );
    task.abort();
    let _ = task.await;
    for text in ["queued one", "queued two", "queued latest"] {
        assert!(
            client
                .call(
                    "session_save_note",
                    json!({"session_id":"s1","markdown":text,"title":null}),
                    false
                )
                .await
                .is_err()
        );
    }
    let pending = client.pending().unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[1].call.args["markdown"], "queued latest");
    assert!(!pending[1].attempted);
    drop(client);
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let task = tokio::spawn(axum::serve(listener, router(server.clone())).into_future());
    let client = ConnectionState::open(device.path().into(), secrets).unwrap();
    client.retry().await.unwrap();
    assert!(client.pending().unwrap().is_empty());
    assert_eq!(
        client
            .call("session_get", json!({"session_id":"s1"}), true)
            .await
            .unwrap()["note_markdown"],
        "queued latest"
    );
    let current = content::invoke(
        server.clone(),
        &owner,
        &a.id,
        Call::query("session_get", json!({"session_id":"s1"})),
    )
    .await
    .unwrap();
    content::invoke(
        server.clone(),
        &owner,
        &a.id,
        Call::mutation(
            "session_save_note",
            json!({"session_id":"s1","markdown":"server changed again","title":null}),
            current.versions,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        client
            .call(
                "session_save_note",
                json!({"session_id":"s1","markdown":"save this separately","title":null}),
                false
            )
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    let draft = client.pending().unwrap().remove(0);
    let recovered = client
        .save_separate(
            draft.call.mutation_id.as_deref().unwrap(),
            "2026-10-18T12:00:00Z",
        )
        .await
        .unwrap();
    assert!(client.pending().unwrap().is_empty());
    assert_eq!(
        client
            .call("session_get", json!({"session_id":recovered}), true)
            .await
            .unwrap()["note_markdown"],
        "save this separately"
    );
    assert_eq!(
        client
            .call("session_get", json!({"session_id":"s1"}), true)
            .await
            .unwrap()["note_markdown"],
        "server changed again"
    );
    assert!(!device.path().join("sessions").exists());
    assert!(!device.path().join("processing").exists());
    task.abort();
    let _ = task.await;
}

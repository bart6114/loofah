use super::*;

struct NoSecrets;
impl CredentialStore for NoSecrets {
    fn load(&self, _: &str) -> hypr_vault_access::Result<Option<Credentials>> {
        Ok(None)
    }
    fn save(&self, _: &str, _: &Credentials) -> hypr_vault_access::Result<()> {
        Ok(())
    }
    fn remove(&self, _: &str) -> hypr_vault_access::Result<()> {
        Ok(())
    }
}

fn app(root: &std::path::Path) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![
            crate::session_store::commands::session_save_note,
            crate::session_store::commands::session_get,
            super::company_action,
        ])
        .manage(Arc::new(
            ConnectionState::open(root.into(), Arc::new(NoSecrets)).unwrap(),
        ))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

async fn ipc(
    window: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
    args: Value,
) -> Result<Value, Value> {
    let window = window.clone();
    let command = command.to_string();
    tokio::task::spawn_blocking(move || {
        tauri::test::get_ipc_response(
            &window,
            tauri::webview::InvokeRequest {
                cmd: command,
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .map(|response| response.deserialize::<Value>().unwrap())
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn local_mode_keeps_store_commands_local_and_native_processing_leases_are_released() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    assert!(
        invoke::<_, Value>(app.handle(), "session_get", json!({"session_id":"s1"}))
            .await
            .is_none()
    );
    let status = action(app.handle(), "status", json!({})).await.unwrap();
    assert_eq!(status["active"], false);
    assert_eq!(status["pending"], json!([]));
    assert_eq!(status["transfers"], json!([]));
    let lease = action(app.handle(), "begin_processing", json!({}))
        .await
        .unwrap();
    assert_eq!(
        action(app.handle(), "status", json!({})).await.unwrap()["processing"],
        true
    );
    action(app.handle(), "end_processing", json!({"id":lease}))
        .await
        .unwrap();
    assert_eq!(
        action(app.handle(), "status", json!({})).await.unwrap()["processing"],
        false
    );
    assert_eq!(
        action(app.handle(), "unknown", json!({}))
            .await
            .unwrap_err()
            .code,
        "invalid_request"
    );
}

#[tokio::test]
async fn standalone_note_windows_block_destination_changes_before_any_configuration_is_changed() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    require_main_only(app.handle()).unwrap();
    let note = tauri::WebviewWindowBuilder::new(
        &app,
        "note-s1",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .build()
    .unwrap();
    for operation in ["select", "logout"] {
        let error = action(app.handle(), operation, json!({"vault_id":null}))
            .await
            .unwrap_err();
        assert_eq!(error.code, "conflict");
        assert!(error.message.contains("standalone note windows"));
    }
    assert!(app.state::<Arc<ConnectionState>>().profile().is_none());
    note.close().unwrap();
}

#[tokio::test]
async fn native_remote_command_failure_emits_revocation_instead_of_falling_back_to_local_storage() {
    use tauri::Listener;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("connection.json"),serde_json::to_vec(&json!({"base_url":"https://company.example","server_id":"server","user_id":"me","vault_id":"team"})).unwrap()).unwrap();
    let app = app(root.path());
    let revoked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let event = revoked.clone();
    app.listen_any("company-access-revoked", move |_| {
        event.store(true, std::sync::atomic::Ordering::Release);
    });
    let result = invoke::<_, Value>(app.handle(), "session_get", json!({"session_id":"s1"}))
        .await
        .unwrap();
    assert!(result.unwrap_err().contains("unauthenticated"));
    assert!(revoked.load(std::sync::atomic::Ordering::Acquire));
    assert!(!root.path().join("sessions").exists());
}

struct Service {
    app: tauri::App<tauri::test::MockRuntime>,
    server: Arc<loofah_server::Server>,
    owner: hypr_vault_access::User,
    vault: hypr_vault_access::Vault,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    root: tempfile::TempDir,
    device: tempfile::TempDir,
}

impl Drop for Service {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Service {
    async fn new() -> Self {
        use hypr_vault_access::digest;
        use loofah_server::{Config, Server, registry::Session};
        let root = tempfile::tempdir().unwrap();
        let device = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = Server::open(Config {
            data_dir: root.path().into(),
            public_url: base.clone(),
            issuer: "http://127.0.0.1:8948".into(),
            client_id: "test".into(),
            client_secret: None,
            max_upload_bytes: 1024,
        })
        .unwrap();
        let owner = server
            .provision("issuer", "owner", "Owner", "owner@example.com")
            .await
            .unwrap();
        let vault = server.create_vault(&owner, "Team").await.unwrap();
        let expires = chrono::Utc::now().timestamp() + 3600;
        server.registry.write().await.sessions.insert(
            digest(b"refresh"),
            Session {
                user_id: owner.id.clone(),
                access_hash: digest(b"access"),
                refresh_hash: digest(b"refresh"),
                access_expires: expires,
                expires,
            },
        );
        let task = tokio::spawn(
            axum::serve(listener, loofah_server::http::router(server.clone())).into_future(),
        );
        let app = app(device.path());
        app.manage(tauri_plugin_settings::ConfigState::load_or_default(
            device.path(),
        ));
        let events = tauri_specta::Builder::<tauri::test::MockRuntime>::new().events(
            tauri_specta::collect_events![crate::session_store::IndexChanged],
        );
        events.mount_events(app.handle());
        let state = app.state::<Arc<ConnectionState>>().inner().clone();
        state
            .connect(
                base,
                ServerInfo {
                    id: server.registry.read().await.server_id.clone(),
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
        state.select(Some(vault.id.clone())).await.unwrap();
        Self {
            app,
            server,
            owner,
            vault,
            task,
            root,
            device,
        }
    }

    fn state(&self) -> Arc<ConnectionState> {
        self.app.state::<Arc<ConnectionState>>().inner().clone()
    }

    async fn events(
        &self,
    ) -> (
        hypr_vault_access::connection::Profile,
        Credentials,
        reqwest::Response,
    ) {
        let (profile, credentials) = self.state().authenticated().await.unwrap();
        let mut response = reqwest::Client::new()
            .get(format!("{}/v1/events", profile.base_url))
            .bearer_auth(&credentials.access_token)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        next_event(&mut response, "reset").await;
        (profile, credentials, response)
    }
}

async fn next_event(response: &mut reqwest::Response, name: &str) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut buffer = String::new();
        loop {
            let chunk = response.chunk().await.unwrap().expect("event stream ended");
            buffer.push_str(std::str::from_utf8(&chunk).unwrap());
            while let Some(end) = buffer.find("\n\n") {
                let message = buffer[..end].to_string();
                buffer.drain(..end + 2);
                if message.lines().any(|line| line == format!("event: {name}")) {
                    return message;
                }
            }
        }
    })
    .await
    .unwrap()
}

fn revocations(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
) -> Arc<std::sync::atomic::AtomicUsize> {
    use tauri::Listener;
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let received = count.clone();
    app.listen_any("company-access-revoked", move |_| {
        received.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    });
    count
}

#[tokio::test]
async fn event_stream_authentication_rejection_refreshes_or_reuses_credentials() {
    for already_refreshed in [true, false] {
        let service = Service::new().await;
        let state = service.state();
        let revoked = revocations(service.app.handle());
        let (profile, mut credentials, response) = service.events().await;
        if already_refreshed {
            credentials.expires_at = 0;
            state
                .connect(
                    profile.base_url.clone(),
                    ServerInfo {
                        id: profile.server_id.clone(),
                        api_version: 1,
                        authentication: "oidc".into(),
                    },
                    credentials.clone(),
                )
                .await
                .unwrap();
            assert_ne!(
                state.authenticated().await.unwrap().1.access_token,
                credentials.access_token
            );
        } else {
            service
                .server
                .registry
                .write()
                .await
                .sessions
                .get_mut(&hypr_vault_access::digest(b"refresh"))
                .unwrap()
                .access_expires = 0;
        }
        service
            .server
            .notify(Some(service.vault.id.clone()), "sessions", vec![]);
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            receive_events(
                service.app.handle(),
                &state,
                &profile,
                &credentials,
                response,
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            revoked.load(std::sync::atomic::Ordering::Acquire),
            0,
            "routine token renewal must preserve vault access"
        );
        assert_ne!(
            state.authenticated().await.unwrap().1.access_token,
            credentials.access_token
        );
        assert!(state.status()["error"].is_null());
        use tauri_specta::Event;
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received = updates.clone();
        crate::session_store::IndexChanged::listen(service.app.handle(), move |event| {
            received.lock().unwrap().push(event.payload);
        });
        let (new_profile, new_credentials, mut reconnected) = service.events().await;
        service.server.notify(
            Some(service.vault.id.clone()),
            "sessions",
            vec!["s1".into()],
        );
        let message = next_event(&mut reconnected, "change").await;
        assert!(
            !event_received(
                service.app.handle(),
                &state,
                &new_profile,
                &new_credentials.access_token,
                &message
            )
            .await
        );
        assert!(updates.lock().unwrap().iter().any(|event| event.entity
            == crate::session_store::IndexEntity::Sessions
            && event.ids == ["s1"]));
    }
}

#[tokio::test]
async fn actual_membership_revocation_locks_the_selected_vault() {
    let service = Service::new().await;
    let other = service
        .server
        .provision("issuer", "other-owner", "Other", "other@example.com")
        .await
        .unwrap();
    service
        .server
        .update_member(
            &service.owner,
            &service.vault.id,
            &other.id,
            Some(hypr_vault_access::Role::Owner),
        )
        .await
        .unwrap();
    let state = service.state();
    let revoked = revocations(service.app.handle());
    let (profile, credentials, response) = service.events().await;
    service
        .server
        .update_member(&other, &service.vault.id, &service.owner.id, None)
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        receive_events(
            service.app.handle(),
            &state,
            &profile,
            &credentials,
            response,
        ),
    )
    .await
    .unwrap();
    assert_eq!(revoked.load(std::sync::atomic::Ordering::Acquire), 1);
    assert_eq!(service.server.registry.read().await.sessions.len(), 1);
}

#[tokio::test]
async fn events_from_an_old_vault_or_account_cannot_lock_a_new_destination() {
    for switch_account in [false, true] {
        let service = Service::new().await;
        let state = service.state();
        let other = service
            .server
            .provision("issuer", "other-owner", "Other", "other@example.com")
            .await
            .unwrap();
        service
            .server
            .update_member(
                &service.owner,
                &service.vault.id,
                &other.id,
                Some(hypr_vault_access::Role::Owner),
            )
            .await
            .unwrap();
        let revoked = revocations(service.app.handle());
        let (profile, credentials, response) = service.events().await;
        let mut receiving = Box::pin(receive_events(
            service.app.handle(),
            &state,
            &profile,
            &credentials,
            response,
        ));
        let waiting = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(receiving.as_mut(), cx).is_pending())
        })
        .await;
        assert!(waiting);
        state.select(None).await.unwrap();
        if switch_account {
            let expires = chrono::Utc::now().timestamp() + 3600;
            service.server.registry.write().await.sessions.insert(
                hypr_vault_access::digest(b"other-refresh"),
                loofah_server::registry::Session {
                    user_id: other.id.clone(),
                    access_hash: hypr_vault_access::digest(b"other-access"),
                    refresh_hash: hypr_vault_access::digest(b"other-refresh"),
                    access_expires: expires,
                    expires,
                },
            );
            state
                .connect(
                    profile.base_url.clone(),
                    ServerInfo {
                        id: profile.server_id.clone(),
                        api_version: 1,
                        authentication: "oidc".into(),
                    },
                    Credentials {
                        access_token: "other-access".into(),
                        refresh_token: "other-refresh".into(),
                        expires_at: expires,
                        user: other.clone(),
                    },
                )
                .await
                .unwrap();
            state.select(Some(service.vault.id.clone())).await.unwrap();
        }
        service
            .server
            .update_member(&other, &service.vault.id, &service.owner.id, None)
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), receiving)
            .await
            .unwrap();
        assert_eq!(revoked.load(std::sync::atomic::Ordering::Acquire), 0);
    }
}

#[tokio::test]
async fn disabled_users_are_locked_when_refresh_is_rejected() {
    let service = Service::new().await;
    let state = service.state();
    let revoked = revocations(service.app.handle());
    let (profile, credentials, response) = service.events().await;
    service
        .server
        .registry
        .write()
        .await
        .users
        .get_mut(&service.owner.id)
        .unwrap()
        .disabled = true;
    service
        .server
        .notify(Some(service.vault.id.clone()), "sessions", vec![]);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        receive_events(
            service.app.handle(),
            &state,
            &profile,
            &credentials,
            response,
        ),
    )
    .await
    .unwrap();
    assert_eq!(revoked.load(std::sync::atomic::Ordering::Acquire), 1);
}

#[tokio::test]
async fn an_expired_stream_handshake_refreshes_without_locking_the_vault() {
    let service = Service::new().await;
    let state = service.state();
    let revoked = revocations(service.app.handle());
    let (profile, credentials) = state.authenticated().await.unwrap();
    service
        .server
        .registry
        .write()
        .await
        .sessions
        .get_mut(&hypr_vault_access::digest(b"refresh"))
        .unwrap()
        .access_expires = 0;
    let response = state
        .http
        .get(format!("{}/v1/events", profile.base_url))
        .bearer_auth(&credentials.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    receive_events(
        service.app.handle(),
        &state,
        &profile,
        &credentials,
        response,
    )
    .await;
    assert_eq!(revoked.load(std::sync::atomic::Ordering::Acquire), 0);
    assert_ne!(
        state.authenticated().await.unwrap().1.access_token,
        credentials.access_token
    );
    assert!(state.request("GET", "/v1/me", None).await.is_ok());
}

#[tokio::test]
async fn a_refresh_transport_failure_preserves_the_destination_and_reports_unavailability() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            drop(socket);
        }
    });
    std::fs::write(
        root.path().join("connection.json"),
        serde_json::to_vec(
            &json!({"base_url":base,"server_id":"server","user_id":"user","vault_id":"vault"}),
        )
        .unwrap(),
    )
    .unwrap();
    let app = app(root.path());
    let state = app.state::<Arc<ConnectionState>>().inner().clone();
    state
        .connect(
            base,
            ServerInfo {
                id: "server".into(),
                api_version: 1,
                authentication: "oidc".into(),
            },
            Credentials {
                access_token: "access".into(),
                refresh_token: "refresh".into(),
                expires_at: chrono::Utc::now().timestamp() + 3600,
                user: hypr_vault_access::User {
                    id: "user".into(),
                    issuer: "issuer".into(),
                    subject: "subject".into(),
                    name: "Name".into(),
                    email: "name@example.com".into(),
                    disabled: false,
                },
            },
        )
        .await
        .unwrap();
    let profile = state.profile().unwrap();
    let revoked = revocations(app.handle());
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            event_received(
                app.handle(),
                &state,
                &profile,
                "access",
                "event: unauthenticated"
            )
        )
        .await
        .unwrap()
    );
    assert_eq!(revoked.load(std::sync::atomic::Ordering::Acquire), 0);
    assert!(state.active());
    assert_eq!(state.status()["error"]["code"], "unavailable");
    task.abort();
}

#[tokio::test]
async fn an_in_flight_refresh_cannot_lock_the_local_vault_after_switching() {
    let service = Service::new().await;
    let state = service.state();
    let revoked = revocations(service.app.handle());
    let (profile, credentials) = state.authenticated().await.unwrap();
    service
        .server
        .registry
        .write()
        .await
        .sessions
        .get_mut(&hypr_vault_access::digest(b"refresh"))
        .unwrap()
        .expires = 0;
    let mut handling = Box::pin(event_received(
        service.app.handle(),
        &state,
        &profile,
        &credentials.access_token,
        "event: unauthenticated",
    ));
    let waiting = std::future::poll_fn(|cx| {
        std::task::Poll::Ready(std::future::Future::poll(handling.as_mut(), cx).is_pending())
    })
    .await;
    assert!(waiting);
    state.select(None).await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), handling)
            .await
            .unwrap()
    );
    assert_eq!(revoked.load(std::sync::atomic::Ordering::Acquire), 0);
    assert!(state.status()["error"].is_null());
}

#[tokio::test]
async fn native_ipc_writes_to_the_real_service_and_recovers_conflicts_without_a_local_store() {
    use crate::session_store::commands;
    use hypr_vault_access::Call;
    use loofah_server::content;
    use tauri_specta::Event;

    let service = Service::new().await;
    let root = &service.root;
    let device = &service.device;
    let app = &service.app;
    let server = service.server.clone();
    let owner = service.owner.clone();
    let vault = service.vault.clone();
    let window = tauri::WebviewWindowBuilder::new(app, "main", Default::default())
        .build()
        .unwrap();
    let received = Arc::new(std::sync::Mutex::new(Vec::new()));
    let changes = received.clone();
    crate::session_store::IndexChanged::listen(app.handle(), move |event| {
        changes.lock().unwrap().push(event.payload);
    });
    let state = app.state::<Arc<ConnectionState>>().inner().clone();
    commands::session_write_meta(
        app.handle().clone(),
        serde_json::from_value(
            json!({"id":"s1","title":"Meeting","created_at":"2026-10-18T10:00:00Z","tags":[]}),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    ipc(
        &window,
        "session_save_note",
        json!({"sessionId":"s1","markdown":"Desktop note","title":"Desktop title"}),
    )
    .await
    .unwrap();
    commands::session_ensure_summary(app.handle().clone(), "s1".into())
        .await
        .unwrap();
    commands::session_update_summary(
        app.handle().clone(),
        "s1".into(),
        "Desktop summary".into(),
        Some(String::new()),
        Some(true),
        Some(vec![crate::session_store::ScoredTagSuggestion {
            name: "project".into(),
            confidence: 0.99,
        }]),
    )
    .await
    .unwrap();
    assert_eq!(
        commands::session_summary_get(app.handle().clone(), "s1".into())
            .await
            .unwrap()
            .as_deref(),
        Some("Desktop summary")
    );
    let read = commands::session_get(app.handle().clone(), "s1".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.meta.title, "Desktop title");
    assert!(read.meta.tags.contains(&"project".into()));
    assert_eq!(read.note_markdown.as_deref(), Some("Desktop note"));
    let doc_id = uuid::Uuid::new_v4().to_string();
    commands::session_write_enhanced_doc(app.handle().clone(),serde_json::from_value(json!({"id":doc_id,"session_id":"s1","kind":"template_output","title":"Actions","template_id":"template","sort_order":0,"markdown":"Initial"})).unwrap()).await.unwrap();
    commands::session_update_enhanced_doc(
        app.handle().clone(),
        "s1".into(),
        doc_id.clone(),
        crate::session_store::EnhancedDocPatch {
            markdown: Some("Updated".into()),
            suggested_tags: Some(vec![crate::session_store::ScoredTagSuggestion {
                name: "from-template".into(),
                confidence: 0.99,
            }]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        commands::session_get(app.handle().clone(), "s1".into())
            .await
            .unwrap()
            .unwrap()
            .meta
            .tags
            .contains(&"from-template".into())
    );
    app.state::<tauri_plugin_settings::ConfigState>()
        .set_values(std::collections::HashMap::from([(
            "auto_apply_high_confidence_tags".into(),
            json!(false),
        )]))
        .await
        .unwrap();
    commands::session_save_enhanced_doc(
        app.handle().clone(),
        "s1".into(),
        doc_id.clone(),
        crate::session_store::EnhancedDocPatch {
            markdown: Some("Manual suggestions".into()),
            suggested_tags: Some(vec![crate::session_store::ScoredTagSuggestion {
                name: "manual".into(),
                confidence: 0.99,
            }]),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    let manual = commands::session_get(app.handle().clone(), "s1".into())
        .await
        .unwrap()
        .unwrap();
    assert!(!manual.meta.tags.contains(&"manual".into()));
    assert!(
        manual
            .meta
            .tag_suggestions
            .unwrap()
            .items
            .contains(&"manual".into())
    );
    app.state::<tauri_plugin_settings::ConfigState>()
        .set_values(std::collections::HashMap::from([(
            "auto_apply_high_confidence_tags".into(),
            json!(true),
        )]))
        .await
        .unwrap();
    commands::session_save_enhanced_doc(
        app.handle().clone(),
        "s1".into(),
        doc_id,
        crate::session_store::EnhancedDocPatch {
            markdown: Some("Automatic suggestions".into()),
            suggested_tags: Some(vec![crate::session_store::ScoredTagSuggestion {
                name: "enabled-again".into(),
                confidence: 0.99,
            }]),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    assert!(
        commands::session_get(app.handle().clone(), "s1".into())
            .await
            .unwrap()
            .unwrap()
            .meta
            .tags
            .contains(&"enabled-again".into())
    );
    let canonical = root
        .path()
        .join("vaults")
        .join(&vault.id)
        .join("sessions/s1");
    assert_eq!(
        std::fs::read_to_string(canonical.join("notes.md")).unwrap(),
        "Desktop note"
    );
    assert_eq!(
        std::fs::read_to_string(canonical.join("summary.md")).unwrap(),
        "Desktop summary"
    );
    assert!(!device.path().join("sessions").exists());
    for entity in [
        crate::session_store::IndexEntity::Sessions,
        crate::session_store::IndexEntity::SessionHeaders,
        crate::session_store::IndexEntity::Docs,
        crate::session_store::IndexEntity::Tasks,
    ] {
        assert!(
            received
                .lock()
                .unwrap()
                .iter()
                .any(|event| event.entity == entity)
        );
    }
    let current = content::invoke(
        server.clone(),
        &owner,
        &vault.id,
        Call::query("session_get", json!({"session_id":"s1"})),
    )
    .await
    .unwrap();
    content::invoke(
        server.clone(),
        &owner,
        &vault.id,
        Call::mutation(
            "session_save_note",
            json!({"session_id":"s1","markdown":"Another device","title":null}),
            current.versions,
        ),
    )
    .await
    .unwrap();
    let error = ipc(
        &window,
        "session_save_note",
        json!({"sessionId":"s1","markdown":"Unsent desktop draft","title":null}),
    )
    .await
    .unwrap_err();
    assert!(error.as_str().unwrap().contains("conflict"));
    let pending = state.pending().unwrap().remove(0);
    let id = pending.call.mutation_id.unwrap();
    assert_eq!(pending.call.args["markdown"], "Unsent desktop draft");
    let preview = ipc(
        &window,
        "company_action",
        json!({"operation":"preview","args":{"id":id}}),
    )
    .await
    .unwrap();
    assert_eq!(preview["data"]["note_markdown"], "Another device");
    ipc(&window,"company_action",json!({"operation":"resolve","args":{"id":id,"action":"replace","expected":preview["versions"]}})).await.unwrap();
    assert!(state.pending().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(canonical.join("notes.md")).unwrap(),
        "Unsent desktop draft"
    );
    assert!(!device.path().join("sessions").exists());
    window.close().unwrap();
}

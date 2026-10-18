use std::sync::{Arc, Mutex};

use hypr_vault_access::{
    Call, Credentials, Result, ServerInfo, Versions,
    connection::{ConnectionState, CredentialStore},
    digest,
};
use loofah_server::{Config, Server, content, http::router, registry::Session};
use serde_json::json;

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

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
};

use hypr_vault_access::{Call, Versions, digest};
use loofah_server::{Config, Server, registry::Session};
use serde_json::{Value, json};

fn config(root: &Path) -> (Config, PathBuf) {
    let config = Config {
        data_dir: root.join("data"),
        public_url: "http://127.0.0.1:8947".into(),
        issuer: "http://127.0.0.1:8948".into(),
        client_id: "test".into(),
        client_secret: None,
        max_upload_bytes: 1024,
    };
    let path = root.join("server.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    (config, path)
}

fn command(path: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_loofah-server"));
    command.arg("--config").arg(path);
    command
}

fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn maintenance_commands_persist_revocation_and_respect_exclusive_ownership() {
    let root = tempfile::tempdir().unwrap();
    let (config, path) = config(root.path());
    let server = Server::open(config.clone()).unwrap();
    let user = server
        .provision("issuer", "subject", "Name", "mail@example.com")
        .await
        .unwrap();
    server.registry.write().await.sessions.insert(
        "session".into(),
        Session {
            user_id: user.id.clone(),
            access_hash: digest(b"access"),
            refresh_hash: digest(b"refresh"),
            access_expires: i64::MAX,
            expires: i64::MAX,
        },
    );
    server
        .save_registry(&*server.registry.read().await)
        .await
        .unwrap();
    assert!(
        !command(&path)
            .arg("recover")
            .output()
            .unwrap()
            .status
            .success()
    );
    drop(server);
    success(
        command(&path)
            .arg("disable-user")
            .arg(&user.id)
            .output()
            .unwrap(),
    );
    let server = Server::open(config.clone()).unwrap();
    assert!(server.registry.read().await.users[&user.id].disabled);
    assert!(server.registry.read().await.sessions.is_empty());
    assert!(
        server
            .provision("issuer", "subject", "Name", "mail@example.com")
            .await
            .is_err()
    );
    drop(server);
    success(
        command(&path)
            .arg("disable-user")
            .arg(&user.id)
            .arg("--enable")
            .output()
            .unwrap(),
    );
    let server = Server::open(config).unwrap();
    assert!(!server.registry.read().await.users[&user.id].disabled);
    assert_eq!(
        server
            .provision("issuer", "subject", "Name", "mail@example.com")
            .await
            .unwrap()
            .id,
        user.id
    );
    drop(server);
    success(command(&path).arg("recover").output().unwrap());
    assert!(
        !command(&path)
            .arg("disable-user")
            .arg("missing")
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[tokio::test]
async fn import_preserves_source_and_attachments_and_excludes_device_settings() {
    let root = tempfile::tempdir().unwrap();
    let (config, path) = config(root.path());
    let server = Server::open(config.clone()).unwrap();
    let user = server
        .provision("issuer", "subject", "Name", "mail@example.com")
        .await
        .unwrap();
    let vault = server.create_vault(&user, "Imported").await.unwrap();
    drop(server);
    let source = root.path().join("source");
    let session = source.join("sessions/s1");
    std::fs::create_dir_all(&session).unwrap();
    let meta = json!({"id":"s1","title":"Imported","created_at":"2026-10-18T10:00:00Z","tags":[]});
    std::fs::write(
        session.join("_meta.json"),
        serde_json::to_vec(&meta).unwrap(),
    )
    .unwrap();
    std::fs::write(session.join("notes.md"), "Original note").unwrap();
    std::fs::write(session.join("unknown.pdf"), b"private attachment").unwrap();
    std::fs::write(source.join("config.json"), b"{\"device_secret\":true}").unwrap();
    success(
        command(&path)
            .arg("import")
            .arg(&source)
            .arg(&vault.id)
            .output()
            .unwrap(),
    );
    let destination = config.data_dir.join("vaults").join(&vault.id);
    assert_eq!(
        std::fs::read_to_string(destination.join("sessions/s1/notes.md")).unwrap(),
        "Original note"
    );
    assert_eq!(
        std::fs::read(destination.join("sessions/s1/unknown.pdf")).unwrap(),
        b"private attachment"
    );
    assert!(!destination.join("config.json").exists());
    assert!(source.join("config.json").exists());
    assert_eq!(
        std::fs::read_to_string(session.join("notes.md")).unwrap(),
        "Original note"
    );
    assert!(
        !command(&path)
            .arg("import")
            .arg(&source)
            .arg(&vault.id)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        !command(&path)
            .arg("import")
            .arg(&source)
            .arg("unknown")
            .output()
            .unwrap()
            .status
            .success()
    );
    let server = Server::open(config).unwrap();
    let runtime = server.runtime(&vault.id).await.unwrap();
    assert_eq!(runtime.store.session_ids(), ["s1"]);
}

#[tokio::test]
async fn unsafe_and_invalid_imports_leave_the_destination_empty() {
    let root = tempfile::tempdir().unwrap();
    let (config, path) = config(root.path());
    let server = Server::open(config.clone()).unwrap();
    let user = server
        .provision("issuer", "subject", "Name", "mail@example.com")
        .await
        .unwrap();
    let vault = server.create_vault(&user, "Empty").await.unwrap();
    drop(server);
    let destination = config.data_dir.join("vaults").join(&vault.id);
    assert!(
        !command(&path)
            .arg("import")
            .arg(root.path())
            .arg(&vault.id)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        !command(&path)
            .arg("import")
            .arg(&config.data_dir)
            .arg(&vault.id)
            .output()
            .unwrap()
            .status
            .success()
    );
    let source = root.path().join("source");
    std::fs::create_dir_all(source.join("sessions/s1")).unwrap();
    std::fs::write(source.join("sessions/s1/_meta.json"), b"broken").unwrap();
    assert!(
        !command(&path)
            .arg("import")
            .arg(&source)
            .arg(&vault.id)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(destination.read_dir().unwrap().count(), 0);
    std::fs::remove_dir_all(source.join("sessions")).unwrap();
    std::os::unix::fs::symlink(root.path().join("server.json"), source.join("secret")).unwrap();
    assert!(
        !command(&path)
            .arg("import")
            .arg(&source)
            .arg(&vault.id)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(destination.read_dir().unwrap().count(), 0);
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn start(path: &Path, address: std::net::SocketAddr) -> Process {
    let mut child = Process(
        command(path)
            .arg("serve")
            .arg("--listen")
            .arg(address.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "service exited before readiness"
            );
            if reqwest::get(format!("http://{address}/ready"))
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    child
}

#[tokio::test]
async fn killed_process_restarts_with_the_same_identity_receipts_and_committed_files() {
    let root = tempfile::tempdir().unwrap();
    let (config, path) = config(root.path());
    let server = Server::open(config).unwrap();
    let user = server
        .provision("issuer", "subject", "Name", "mail@example.com")
        .await
        .unwrap();
    let vault = server.create_vault(&user, "Durability").await.unwrap();
    server.registry.write().await.sessions.insert(
        "session".into(),
        Session {
            user_id: user.id,
            access_hash: digest(b"access"),
            refresh_hash: digest(b"refresh"),
            access_expires: i64::MAX,
            expires: i64::MAX,
        },
    );
    server
        .save_registry(&*server.registry.read().await)
        .await
        .unwrap();
    let server_id = server.registry.read().await.server_id.clone();
    drop(server);
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let mut process = start(&path, address).await;
    let client = reqwest::Client::new();
    let invoke = format!("http://{address}/v1/vaults/{}/invoke", vault.id);
    let call = Call::mutation(
        "session_write_meta",
        json!({"meta":{"id":"s1","title":"Durable","created_at":"2026-10-18T10:00:00Z","tags":[]}}),
        Versions::new(),
    );
    let first: Value = client
        .post(&invoke)
        .bearer_auth("access")
        .json(&call)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    process.0.kill().unwrap();
    process.0.wait().unwrap();
    drop(process);
    let mut process = start(&path, address).await;
    let info: Value = client
        .get(format!("http://{address}/v1/server"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["id"], server_id);
    let retried: Value = client
        .post(&invoke)
        .bearer_auth("access")
        .json(&call)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first, retried);
    let read: Value = client
        .post(&invoke)
        .bearer_auth("access")
        .json(&Call::query("session_get", json!({"session_id":"s1"})))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read["data"]["meta"]["title"], "Durable");
    success(
        Command::new("kill")
            .arg("-TERM")
            .arg(process.0.id().to_string())
            .output()
            .unwrap(),
    );
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(status) = process.0.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(status.success());
}

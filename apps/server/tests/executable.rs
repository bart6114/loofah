#![cfg(unix)]

use std::{process::Stdio, time::Duration};

use axum::{Json, Router, routing::get};
use serde_json::{Value, json};

#[tokio::test]
async fn executable_starts_without_desktop_resources_and_exits_on_sigterm() {
    let provider = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", provider.local_addr().unwrap());
    let metadata = json!({"issuer":issuer,"jwks_uri":format!("{issuer}/keys"),"authorization_endpoint":format!("{issuer}/authorize"),"token_endpoint":format!("{issuer}/token"),"code_challenge_methods_supported":["S256"]});
    let keys: Value = serde_json::from_str(include_str!("fixtures/jwks.json")).unwrap();
    let provider_app = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || async move { Json(metadata) }),
        )
        .route("/keys", get(move || async move { Json(keys) }));
    let provider_task = tokio::spawn(async move {
        axum::serve(provider, provider_app).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let reserved = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = reserved.local_addr().unwrap();
    drop(reserved);
    let config = json!({
        "storage_root":root.path().join("storage"),"cache_root":root.path().join("cache"),
        "public_url":"https://loofah.company.example","listen":address,
        "oidc":{"issuer":issuer,"audience":"loofah-api","client_id":"loofah-desktop","required_scope":"loofah:access"},
        "allow_insecure_loopback":true
    });
    let path = root.path().join("server.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let log = root.path().join("server.log");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_loofah-server"))
        .arg("--config")
        .arg(path)
        .current_dir(root.path())
        .stdout(Stdio::null())
        .stderr(Stdio::from(std::fs::File::create(&log).unwrap()))
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = reqwest::Client::new();
    let url = format!("http://{address}");
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                panic!(
                    "server exited {status}: {}",
                    std::fs::read_to_string(&log).unwrap()
                );
            }
            if client
                .get(format!("{url}/health"))
                .send()
                .await
                .is_ok_and(|response| response.status() == 204)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let capabilities: Value = client
        .get(format!("{url}/api/v1/capabilities"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(capabilities["api_major"], 1);
    assert_eq!(capabilities["application_version"], env!("LOOFAH_VERSION"));
    assert_eq!(
        client
            .post(format!("{url}/api/v1/me"))
            .header("x-loofah-api-major", "1")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert!(
        tokio::process::Command::new("/bin/kill")
            .arg("-TERM")
            .arg(child.id().unwrap().to_string())
            .status()
            .await
            .unwrap()
            .success()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    provider_task.abort();
}

use std::sync::Arc;
use std::time::Instant;

use hypr_vault_access::{Call, Role, User, VaultAccess, Versions, client::RemoteVault, digest};
use loofah_server::{Config, Server, http::router, registry::Session};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "creates a 50,000-session fixture and runs 50 concurrent HTTP clients"]
async fn fifty_clients_large_vault() {
    let count = std::env::var("LOOFAH_PERF_SESSIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(50_000)
        .max(50);
    let root = tempfile::tempdir().unwrap();
    let server = Server::open(Config {
        data_dir: root.path().into(),
        public_url: "http://127.0.0.1:8947".into(),
        issuer: "http://127.0.0.1:8948".into(),
        client_id: "test".into(),
        client_secret: None,
        max_upload_bytes: 1024,
    })
    .unwrap();
    let owner = server
        .provision("test", "owner", "Owner", "owner@example.com")
        .await
        .unwrap();
    let vault = server.create_vault(&owner, "Scale fixture").await.unwrap();
    let content = root.path().join("vaults").join(&vault.id);
    let seeded = Instant::now();
    for index in 0..count {
        let id = format!("s{index:06}");
        let path = content.join("sessions").join(&id);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("_meta.json"),serde_json::to_vec(&json!({"id":id,"title":format!("Project review {index}"),"created_at":"2026-10-18T10:00:00Z","started_at":null,"ended_at":null,"tags":["project"]})).unwrap()).unwrap();
        std::fs::write(
            path.join("notes.md"),
            "Decisions and next steps for the project review.\n".repeat(20),
        )
        .unwrap();
        std::fs::write(
            path.join("summary.md"),
            "# Summary\n\nThe team reviewed delivery milestones.\n",
        )
        .unwrap();
    }
    let expires = chrono::Utc::now().timestamp() + 3600;
    {
        let mut registry = server.registry.write().await;
        for index in 0..50 {
            let id = format!("client{index}");
            registry.users.insert(
                id.clone(),
                User {
                    id: id.clone(),
                    issuer: "test".into(),
                    subject: id.clone(),
                    name: id.clone(),
                    email: String::new(),
                    disabled: false,
                },
            );
            registry
                .vaults
                .get_mut(&vault.id)
                .unwrap()
                .members
                .insert(id.clone(), Role::Editor);
            let token = format!("client-token-{index}");
            registry.sessions.insert(
                id.clone(),
                Session {
                    user_id: id,
                    access_hash: digest(token.as_bytes()),
                    refresh_hash: String::new(),
                    access_expires: expires,
                    expires,
                },
            );
        }
        server.save_registry(&registry).await.unwrap();
    }
    let cold = Instant::now();
    let runtime = server.runtime(&vault.id).await.unwrap();
    let cold_seconds = cold.elapsed().as_secs_f64();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let serving = tokio::spawn(axum::serve(listener, router(server)).into_future());
    let barrier = Arc::new(tokio::sync::Barrier::new(50));
    let mut tasks = tokio::task::JoinSet::new();
    let started = Instant::now();
    for index in 0..50 {
        let barrier = barrier.clone();
        let vault_id = vault.id.clone();
        tasks.spawn(async move {
            let client=RemoteVault{client:reqwest::Client::new(),base_url:format!("http://{address}"),vault_id,access_token:format!("client-token-{index}")};
            let id=format!("s{index:06}");let mut versions=Versions::new();let mut timings=Vec::new();barrier.wait().await;
            for turn in 0..12 {
                let call=if turn%3==0 {Call::query("session_list_headers",json!({"limit":200,"after":format!("s{:06}",(index*997+turn*200)%count)}))}
                else if turn%3==1 {Call::query("session_get",json!({"session_id":id}))}
                else {Call::mutation("session_save_note",json!({"session_id":id,"markdown":format!("Client {index}, save {turn}"),"title":null}),versions.clone())};
                let operation=if turn%3==2 {"write"} else {"read"};let at=Instant::now();let reply=client.invoke(call).await.unwrap();timings.push((operation,at.elapsed().as_secs_f64()*1000.0));versions.extend(reply.versions);
            }
            timings
        });
    }
    let mut read = Vec::new();
    let mut write = Vec::new();
    while let Some(result) = tasks.join_next().await {
        for (operation, ms) in result.unwrap() {
            if operation == "read" {
                read.push(ms)
            } else {
                write.push(ms)
            }
        }
    }
    let stats = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        json!({"p50_ms":values[values.len()/2],"p95_ms":values[values.len()*95/100],"p99_ms":values[values.len()*99/100]})
    };
    println!(
        "{}",
        json!({"sessions":count,"clients":50,"fixture_and_startup_seconds":seeded.elapsed().as_secs_f64(),"cold_index_seconds":cold_seconds,"workload_seconds":started.elapsed().as_secs_f64(),"read":stats(read),"write":stats(write),"search_ready":runtime.search_ready.load(std::sync::atomic::Ordering::Acquire)})
    );
    serving.abort();
    let _ = serving.await;
}

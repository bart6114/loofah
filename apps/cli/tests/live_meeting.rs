#![cfg(target_os = "macos")]

use hypr_meeting_live::{self as live, Request, Response};
use serde_json::{Value, json};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_and_mcp_use_the_same_recording_connection_without_cache_setup() {
    let vault = tempfile::tempdir().unwrap();
    let identity = live::Identity::new("stable", vault.path()).unwrap();
    let server = live::transport::Server::bind(&identity).unwrap();
    let mut state = live::LiveState::default();
    let recording_id = state.start("meeting", true);
    state.append(
        "meeting",
        vec![live::Word {
            id: "word-1".into(),
            text: "Deadline?".into(),
            start_ms: 100,
            end_ms: 400,
            channel: 0,
            speaker_index: None,
            speaker_label: "You".into(),
        }],
        vec![],
    );
    let state = Arc::new(Mutex::new(state));
    let nudges = Arc::new(Mutex::new(Vec::new()));
    let received_nudges = nudges.clone();
    let server_task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = server.listener.accept().await.unwrap();
            let request: live::Envelope = live::transport::read(&mut stream).await.unwrap();
            assert_eq!(request.identity, identity);
            assert_eq!(request.version, live::PROTOCOL_VERSION);
            let response = match request.request {
                Request::Status => Response::Status(live::Status {
                    active_meeting: state.lock().unwrap().active(),
                }),
                Request::Transcript(input) => {
                    Response::Transcript(state.lock().unwrap().transcript(&input).unwrap())
                }
                Request::Nudge(input) => {
                    let active = state
                        .lock()
                        .unwrap()
                        .check_recording(input.recording_id.as_deref())
                        .unwrap();
                    received_nudges
                        .lock()
                        .unwrap()
                        .push(live::normalize_nudge(&input.text).unwrap());
                    Response::Nudge(live::NudgeReceipt {
                        nudge_id: "accepted".into(),
                        recording_id: active.recording_id,
                    })
                }
            };
            live::transport::write(&mut stream, &response)
                .await
                .unwrap();
        }
    });

    let cli = |args: Vec<String>| {
        let vault = vault.path().to_path_buf();
        tokio::task::spawn_blocking(move || {
            std::process::Command::new(env!("CARGO_BIN_EXE_loof"))
                .arg("--vault-path")
                .arg(vault)
                .arg("--json")
                .args(args)
                .output()
                .unwrap()
        })
    };
    let output = cli(vec!["live".into(), "status".into()]).await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        status["data"]["active_meeting"]["recording_id"],
        recording_id
    );
    let output = cli(vec![
        "live".into(),
        "transcript".into(),
        "--recording-id".into(),
        recording_id.clone(),
    ])
    .await
    .unwrap();
    assert!(output.status.success());
    let transcript: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        transcript["data"]["changes"][0]["word"]["text"],
        "Deadline?"
    );
    let output = cli(vec![
        "nudge".into(),
        "Ask about timing".into(),
        "--recording-id".into(),
        recording_id.clone(),
    ])
    .await
    .unwrap();
    assert!(output.status.success());

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_loof"))
        .arg("--vault-path")
        .arg(vault.path())
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    input.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"live-test","version":"1"}}})).as_bytes()).await.unwrap();
    let initialized: Value = serde_json::from_str(
        &tokio::time::timeout(std::time::Duration::from_secs(10), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(initialized["id"], 1);
    input
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    for (id, name, args) in [
        (2, "get_active_meeting", json!({})),
        (
            3,
            "get_live_transcript",
            json!({"recording_id":recording_id}),
        ),
        (
            4,
            "nudge",
            json!({"text":"Ask about ownership","recording_id":recording_id}),
        ),
    ] {
        input.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}})).as_bytes()).await.unwrap();
        let result: Value = serde_json::from_str(
            &tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["id"], id);
        assert_ne!(result["result"]["isError"], true, "{result}");
        assert!(result["error"].is_null(), "{result}");
    }
    assert_eq!(
        *nudges.lock().unwrap(),
        ["Ask about timing", "Ask about ownership"]
    );
    assert_eq!(
        std::fs::read_dir(vault.path()).unwrap().count(),
        0,
        "Live tools must not write to the vault"
    );
    child.kill().await.unwrap();
    server_task.abort();
}

#[test]
fn cli_live_failures_are_json_errors_without_cache_initialization() {
    let vault = tempfile::tempdir().unwrap();
    for (args, expected) in [
        (vec!["live", "status"], "desktop_unavailable"),
        (vec!["nudge", "   "], "invalid_input"),
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_loof"))
            .arg("--vault-path")
            .arg(vault.path())
            .arg("--json")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let value: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(value["error"]["code"], expected);
        assert_eq!(std::fs::read_dir(vault.path()).unwrap().count(), 0);
    }
}

use super::*;
use crate::command_gateway::CommandGateway;
use hypr_loofah_protocol::{LocalCliOperation as Operation, LocalCliReply as Reply, *};
use serde_json::{Value, json};
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Credentials;
impl CredentialStore for Credentials {
    fn load<'a>(&'a self, _: &'a str) -> hypr_loofah_client::CredentialFuture<'a, Option<String>> {
        Box::pin(async {
            Ok(Some(
                r#"{"schema":1,"refresh_token":"original-refresh"}"#.into(),
            ))
        })
    }
    fn save<'a>(
        &'a self,
        _: &'a str,
        _: Option<&'a str>,
    ) -> hypr_loofah_client::CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}
struct State {
    member: String,
    note: Value,
    writes: Vec<(String, Value)>,
    lose_write_reply: bool,
}
struct Fixture {
    _root: tempfile::TempDir,
    lease: Arc<ProfileLease>,
    gateway: CommandGateway,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
        let lease = store
            .create("test", "https://original.example")
            .await
            .unwrap();
        let mut profile = lease.profile().await;
        profile.member_id = Some("alice".into());
        profile.vault_id = Some("private".into());
        profile.credential_accounts = vec!["a".repeat(64)];
        lease.save(profile).await.unwrap();
        let state = Arc::new(Mutex::new(State {
            member: "alice".into(),
            note: json!({"markdown":"Original","title":"Original title","revision":"a".repeat(64)}),
            writes: vec![],
            lose_write_reply: false,
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server_origin = origin.clone();
        let server_state = state.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (header_end, length) = loop {
                    let mut chunk = [0; 4096];
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break (0, 0);
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    assert!(bytes.len() < 128 * 1024);
                    if let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|n| n.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break (end + 4, length);
                        }
                    }
                };
                if header_end == 0 {
                    continue;
                }
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let line = headers.lines().next().unwrap();
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap();
                let target = parts.next().unwrap();
                let path = target.split_once('?').map_or(target, |(path, query)| {
                    assert!(query.is_empty(), "unexpected fixture query");
                    path
                });
                let request = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("x-request-id: "))
                    .unwrap_or("")
                    .to_owned();
                let body = if length > 0 {
                    serde_json::from_slice(&bytes[header_end..header_end + length]).ok()
                } else {
                    None
                };
                let (status, reply, lose) = {
                    let mut state = server_state.lock().unwrap();
                    match path {
                        "/api/v1/capabilities" => (
                            200,
                            json!({"api_major":1,"application_version":"test","features":[]}),
                            false,
                        ),
                        "/api/v1/auth/config" => (
                            200,
                            json!({"issuer":server_origin,"authorization_endpoint":format!("{server_origin}/authorize"),"token_endpoint":format!("{server_origin}/token"),"client_id":"cli","audience":"api","scopes":["openid"]}),
                            false,
                        ),
                        "/.well-known/openid-configuration" => (
                            200,
                            json!({"issuer":server_origin,"authorization_endpoint":format!("{server_origin}/authorize"),"token_endpoint":format!("{server_origin}/token"),"code_challenge_methods_supported":["S256"]}),
                            false,
                        ),
                        "/token" => (
                            200,
                            json!({"token_type":"Bearer","access_token":"fixture-access","refresh_token":"original-refresh","expires_in":3600}),
                            false,
                        ),
                        "/api/v1/me" => (
                            200,
                            json!({"member":{"id":state.member,"name":"Original"},"personal_vault_id":"private"}),
                            false,
                        ),
                        "/api/v1/vaults/private/sessions/note/note" => {
                            assert!(headers.contains("authorization: Bearer fixture-access"));
                            if method == "GET" {
                                (200, state.note.clone(), false)
                            } else {
                                let body: Value = body.unwrap();
                                state.writes.push((request, body.clone()));
                                if body["expected_revision"] != state.note["revision"] {
                                    (409, json!({"kind":"conflict","note":state.note}), false)
                                } else {
                                    state.note = json!({"markdown":body["markdown"],"title":"Original title","revision":"b".repeat(64)});
                                    (
                                        200,
                                        json!({"kind":"saved","note":state.note}),
                                        state.lose_write_reply,
                                    )
                                }
                            }
                        }
                        _ => panic!("Unexpected dispatch: {method} {path}"),
                    }
                };
                if lose {
                    continue;
                }
                let body = serde_json::to_vec(&reply).unwrap();
                let head = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
            }
        });
        // The transport fixture uses the SDK's explicit loopback test option. Production
        // RemoteTarget::begin always discovers the profile's original HTTPS origin.
        let client =
            RemoteClient::discover(&origin, lease.credentials(Arc::new(Credentials)), true)
                .await
                .unwrap();
        let generation = lease.generation().await.unwrap();
        let gateway = CommandGateway::Remote(RemoteSession {
            media_receipts: root.path().join("media-submissions"),
            client,
            lease: lease.clone(),
            generation,
            member: "alice".into(),
            vault: "private".into(),
            request_id: uuid::Uuid::new_v4().to_string(),
        });
        Self {
            _root: root,
            lease,
            gateway,
            state,
            task,
        }
    }
}
#[tokio::test]
async fn remote_append_keeps_original_revision_and_reports_conflict_without_write() {
    let f = Fixture::new().await;
    let reply = f
        .gateway
        .execute(Operation::AppendNote {
            session_id: "note".into(),
            markdown: "Suffix".into(),
            expected_revision: "a".repeat(64),
        })
        .await
        .unwrap();
    assert!(matches!(
        reply,
        Reply::NoteSaved {
            outcome: NoteSaveOutcome::Saved(_)
        }
    ));
    assert_eq!(f.state.lock().unwrap().note["markdown"], "Original\nSuffix");
    let reply = f
        .gateway
        .execute(Operation::AppendNote {
            session_id: "note".into(),
            markdown: "Stale".into(),
            expected_revision: "a".repeat(64),
        })
        .await
        .unwrap();
    assert!(matches!(
        reply,
        Reply::NoteSaved {
            outcome: NoteSaveOutcome::Conflict(_)
        }
    ));
    assert_eq!(f.state.lock().unwrap().writes.len(), 1);
}
#[tokio::test]
async fn remote_lost_write_reply_is_unknown_and_never_retried() {
    let f = Fixture::new().await;
    f.state.lock().unwrap().lose_write_reply = true;
    let error = f
        .gateway
        .execute(Operation::SaveNote {
            session_id: "note".into(),
            input: SaveNoteRequest {
                markdown: "Committed".into(),
                title: None,
                expected_revision: "a".repeat(64),
            },
        })
        .await
        .unwrap_err();
    let crate::Error::Remote(cause) = error else {
        panic!("typed remote error")
    };
    assert_eq!(cause.mutation_outcome, Some(MutationOutcome::Unknown));
    let state = f.state.lock().unwrap();
    assert_eq!(state.writes.len(), 1);
    assert_eq!(cause.request_id.as_ref(), Some(&state.writes[0].0));
    assert_eq!(state.note["markdown"], "Committed");
}
#[tokio::test]
async fn changed_profile_or_member_cannot_dispatch_a_mutation() {
    for profile_change in [false, true] {
        let f = Fixture::new().await;
        if profile_change {
            let mut profile = f.lease.profile().await;
            profile.vault_id = Some("other".into());
            f.lease.save(profile).await.unwrap();
        } else {
            f.state.lock().unwrap().member = "bob".into();
        }
        let error = f
            .gateway
            .execute(Operation::SaveNote {
                session_id: "note".into(),
                input: SaveNoteRequest {
                    markdown: "Wrong target".into(),
                    title: None,
                    expected_revision: "a".repeat(64),
                },
            })
            .await
            .unwrap_err();
        assert_eq!(
            error.code(),
            if profile_change {
                "conflict"
            } else {
                "unauthenticated"
            }
        );
        assert!(f.state.lock().unwrap().writes.is_empty());
    }
}

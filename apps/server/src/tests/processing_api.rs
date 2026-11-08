use super::processing::{Provider, prepare, terminal};
use super::*;
use hypr_loofah_client::{JobSubmissions, RemoteClient, RemoteOperation, RemoteReply};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

impl Fixture {
    async fn job_call(
        &self,
        subject: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
        request_id: Option<&str>,
    ) -> (StatusCode, Value) {
        self.call_token(&self.token(subject), method, path, body, request_id)
            .await
    }
}

pub(super) async fn input(context: &hypr_loofah_core::VaultContext) -> SummaryJobRequest {
    SummaryJobRequest {
        only_if_empty: false,
        title_policy: Default::default(),
        session_id: "note".into(),
        target: DocumentTarget::Summary,
        expected_revision: context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap()
            .revision,
    }
}
pub(super) async fn shared(fixture: &Fixture) -> (VaultInfo, Me) {
    fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let identity = crate::auth::Identity {
        issuer: fixture.issuer.clone(),
        subject: "alice".into(),
        name: "Alice".into(),
        expires_at: now() + 3600,
    };
    let vault = fixture
        .service
        .catalog
        .create_shared(&identity, &uuid::Uuid::new_v4().to_string(), "Team")
        .await
        .unwrap();
    fixture
        .service
        .catalog
        .set_membership(
            &identity,
            &vault.id,
            &bob.member.id,
            Some(VaultRole::Editor),
            &vault.revision,
        )
        .await
        .unwrap();
    (vault, bob)
}

#[tokio::test]
async fn public_jobs_replay_after_application_and_return_bound_private_recovery_results() {
    let provider = Provider::new(2).await;
    let fixture = Fixture::with_processing(Some(provider.config())).await;
    let me = fixture.login("alice").await;
    let vault = &me.personal_vault_id;
    let context = fixture.service.vault(vault).await.unwrap();
    prepare(&context).await;
    let input = serde_json::to_value(input(&context).await).unwrap();
    let request_id = uuid::Uuid::new_v4().to_string();
    let path = format!("/api/v1/vaults/{vault}/jobs");
    let (status, body) = fixture
        .job_call(
            "alice",
            "POST",
            &path,
            Some(input.clone()),
            Some(&request_id),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let job: ProcessingJobSnapshot = serde_json::from_value(body).unwrap();
    assert_eq!(
        terminal(&context, &job.id).await.state,
        ProcessingJobState::Succeeded
    );
    let (status, replayed) = fixture
        .job_call(
            "alice",
            "POST",
            &path,
            Some(input.clone()),
            Some(&request_id),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed["id"], job.id);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    let mut changed = input;
    changed["expected_revision"] = json!("different");
    assert_eq!(
        fixture
            .job_call("alice", "POST", &path, Some(changed), Some(&request_id))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (status, result) = fixture
        .job_call(
            "alice",
            "GET",
            &format!("{path}/{}/result", job.id),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let result: ProcessingJobResult = serde_json::from_value(result).unwrap();
    assert_eq!(result.vault_id, *vault);
    assert_eq!(result.job_id, job.id);
    assert_eq!(result.session_id, "note");
    assert_eq!(result.markdown, "# Release\n- Ship Friday.");
    let invalid_target = SummaryJobRequest {
        only_if_empty: false,
        title_policy: Default::default(),
        session_id: "note".into(),
        target: DocumentTarget::TemplateOutput {
            document_id: "../output".into(),
        },
        expected_revision: "revision".into(),
    };
    assert_eq!(
        fixture
            .job_call(
                "alice",
                "POST",
                &path,
                Some(serde_json::to_value(invalid_target).unwrap()),
                Some(&uuid::Uuid::new_v4().to_string())
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, page) = fixture
        .job_call(
            "alice",
            "GET",
            &format!("{path}?session_id=note&limit=1"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let page: ProcessingJobPage = serde_json::from_value(page).unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.next_cursor.is_none());
    let (status, page) = fixture
        .job_call(
            "alice",
            "GET",
            &format!("{path}?active_only=true"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(page["items"].as_array().unwrap().is_empty());
    let (status, capabilities) = fixture
        .job_call("alice", "GET", "/api/v1/capabilities", None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("summary_body_jobs"))
    );
    assert!(
        capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("generated_titles"))
    );
    fixture.service.shutdown().await.unwrap();
    provider.close().await;
}

#[tokio::test]
async fn another_shared_vault_editor_cannot_inspect_cancel_or_fetch_a_members_private_job() {
    let provider = Provider::new(1).await;
    let fixture = Fixture::with_processing(Some(provider.config())).await;
    let (vault, _) = shared(&fixture).await;
    let context = fixture.service.vault(&vault.id).await.unwrap();
    prepare(&context).await;
    let path = format!("/api/v1/vaults/{}/jobs", vault.id);
    let (status, job) = fixture
        .job_call(
            "alice",
            "POST",
            &path,
            Some(serde_json::to_value(input(&context).await).unwrap()),
            Some(&uuid::Uuid::new_v4().to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let job: ProcessingJobSnapshot = serde_json::from_value(job).unwrap();
    terminal(&context, &job.id).await;
    for (method, suffix) in [("GET", ""), ("GET", "/result"), ("POST", "/cancel")] {
        assert_eq!(
            fixture
                .job_call(
                    "bob",
                    method,
                    &format!("{path}/{}{suffix}", job.id),
                    None,
                    Some(&uuid::Uuid::new_v4().to_string())
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    let (status, page) = fixture.job_call("bob", "GET", &path, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page["items"].as_array().unwrap().is_empty());
    let (status, _) = fixture
        .job_call(
            "bob",
            "GET",
            &format!("{path}?after={}", job.id),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let identity = crate::auth::Identity {
        issuer: fixture.issuer.clone(),
        subject: "alice".into(),
        name: "Alice".into(),
        expires_at: now() + 3600,
    };
    let revision = fixture
        .service
        .catalog
        .access(&identity, &vault.id, false)
        .await
        .unwrap()
        .info
        .revision;
    let bob = fixture
        .service
        .catalog
        .me(&crate::auth::Identity {
            issuer: fixture.issuer.clone(),
            subject: "bob".into(),
            name: "Bob".into(),
            expires_at: now() + 3600,
        })
        .await
        .unwrap();
    fixture
        .service
        .catalog
        .set_membership(
            &identity,
            &vault.id,
            &bob.member.id,
            Some(VaultRole::Reader),
            &revision,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .job_call(
                "bob",
                "POST",
                &path,
                Some(serde_json::to_value(input(&context).await).unwrap()),
                Some(&uuid::Uuid::new_v4().to_string())
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    fixture.service.shutdown().await.unwrap();
    provider.close().await;
}

#[tokio::test]
async fn explicit_cancel_is_idempotent_and_does_not_apply_an_unfinished_provider_response() {
    let provider = Provider::new(0).await;
    let fixture = Fixture::with_processing(Some(provider.config())).await;
    let me = fixture.login("alice").await;
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    prepare(&context).await;
    let path = format!("/api/v1/vaults/{}/jobs", me.personal_vault_id);
    let (status, job) = fixture
        .job_call(
            "alice",
            "POST",
            &path,
            Some(serde_json::to_value(input(&context).await).unwrap()),
            Some(&uuid::Uuid::new_v4().to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let job: ProcessingJobSnapshot = serde_json::from_value(job).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while provider.requests.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let cancel = format!("{path}/{}/cancel", job.id);
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        fixture
            .job_call("alice", "POST", &cancel, None, Some(&id))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        terminal(&context, &job.id).await.state,
        ProcessingJobState::Cancelled
    );
    assert_eq!(
        fixture
            .job_call("alice", "POST", &cancel, None, Some(&id))
            .await
            .1["state"],
        "cancelled"
    );
    assert_eq!(
        context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .unwrap()
            .markdown,
        ""
    );
    assert_eq!(
        fixture
            .job_call(
                "alice",
                "GET",
                &format!("{path}/{}/result", job.id),
                None,
                None
            )
            .await
            .1,
        Value::Null
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    fixture.service.shutdown().await.unwrap();
    provider.close().await;
}

#[tokio::test]
async fn disabled_processing_and_unsupported_or_injected_inputs_fail_explicitly() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let path = format!("/api/v1/vaults/{}/jobs", me.personal_vault_id);
    assert_eq!(
        fixture.job_call("alice", "GET", &path, None, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        fixture
            .job_call("alice", "GET", &format!("{path}?owner=bob"), None, None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let request = json!({"session_id":"note","target":{"kind":"summary"},"expected_revision":"revision","api_key":"injected"});
    assert_eq!(
        fixture
            .job_call(
                "alice",
                "POST",
                &path,
                Some(request),
                Some(&uuid::Uuid::new_v4().to_string())
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, capabilities) = fixture
        .job_call("alice", "GET", "/api/v1/capabilities", None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("summary_body_jobs"))
    );
    assert!(
        !capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("generated_titles"))
    );
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn job_event_replay_is_member_scoped_over_the_authenticated_tcp_channel() {
    let provider = Provider::new(1).await;
    let fixture = Fixture::with_processing(Some(provider.config())).await;
    let (vault, _) = shared(&fixture).await;
    let context = fixture.service.vault(&vault.id).await.unwrap();
    prepare(&context).await;
    let cursor = context.events.publish(Change::SearchReady).cursor;
    let path = format!("/api/v1/vaults/{}/jobs", vault.id);
    let (status, body) = fixture
        .job_call(
            "alice",
            "POST",
            &path,
            Some(serde_json::to_value(input(&context).await).unwrap()),
            Some(&uuid::Uuid::new_v4().to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let job: ProcessingJobSnapshot = serde_json::from_value(body).unwrap();
    terminal(&context, &job.id).await;
    let sentinel = context.events.publish(Change::CatalogChanged).cursor;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/api/v1/vaults/{}/events",
        listener.local_addr().unwrap(),
        vault.id
    );
    let app = fixture.app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    for (subject, visible) in [("alice", true), ("bob", false)] {
        let mut response = reqwest::Client::new()
            .get(&url)
            .bearer_auth(fixture.token(subject))
            .header("x-loofah-api-major", "1")
            .header("last-event-id", &cursor)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let bytes = tokio::time::timeout(Duration::from_secs(5), async {
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.unwrap() {
                bytes.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&bytes).contains(&sentinel) {
                    break;
                }
            }
            bytes
        })
        .await
        .unwrap();
        let body = String::from_utf8(bytes).unwrap();
        assert_eq!(body.contains(&job.id), visible);
        assert!(!body.contains("audience"));
    }
    fixture.service.shutdown().await.unwrap();
    server.abort();
    provider.close().await;
}

pub(super) async fn native_client(
    fixture: &Fixture,
    rtt: u64,
    reply_gate: CancellationToken,
) -> (Arc<RemoteClient>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let app = fixture.app.clone().layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let hold = request.method() == axum::http::Method::POST
                && request.uri().path().ends_with("/jobs")
                && first.swap(false, std::sync::atomic::Ordering::SeqCst);
            let gate = reply_gate.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(rtt / 2)).await;
                let response = next.run(request).await;
                if hold {
                    gate.cancelled().await;
                }
                tokio::time::sleep(Duration::from_millis(rtt / 2)).await;
                response
            }
        },
    ));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = RemoteClient::discover(&origin, Arc::new(EphemeralCredentials), true)
        .await
        .unwrap();
    (client, server)
}

#[tokio::test]
async fn native_sdk_replays_a_lost_submission_reply_at_realistic_latency_without_repeating_generation()
 {
    for rtt in [50, 150, 500] {
        let provider = Provider::new(1).await;
        let fixture = Fixture::with_processing(Some(provider.config())).await;
        let gate = CancellationToken::new();
        let (client, server) = native_client(&fixture, rtt, gate.clone()).await;
        let login = client.begin_login().await.unwrap();
        let url = login.authorization_url().to_string();
        let complete = tokio::spawn(login.complete());
        assert!(reqwest::get(url).await.unwrap().status().is_success());
        complete.await.unwrap().unwrap();
        let RemoteReply::Me(me) = client
            .execute(
                RemoteOperation::Provision,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
        else {
            panic!("native provision response");
        };
        let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
        prepare(&context).await;
        for input in [
            SessionSummaryQuery::Title {
                query: "release".into(),
                limit: 50,
            },
            SessionSummaryQuery::Ids {
                ids: vec!["missing".into(), "note".into(), "note".into()],
            },
        ] {
            let expected = context.session_summaries(input.clone()).await.unwrap();
            let RemoteReply::SessionSummaries(rows) = client
                .execute(
                    RemoteOperation::SessionSummaries {
                        vault_id: me.personal_vault_id.clone(),
                        input,
                    },
                    &uuid::Uuid::new_v4().to_string(),
                    CancellationToken::new(),
                )
                .await
                .unwrap()
            else {
                panic!("native session summary response");
            };
            assert_eq!(rows, expected);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].id, "note");
        }
        let request = input(&context).await;
        context
            .settings
            .set_values(std::collections::HashMap::from([
                ("current_llm_provider".into(), Value::Null),
                ("current_llm_model".into(), Value::Null),
            ]))
            .await
            .unwrap();
        let RemoteReply::ProcessingPolicy(policy) = client
            .execute(
                RemoteOperation::ProcessingPolicy {
                    vault_id: me.personal_vault_id.clone(),
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
        else {
            panic!("native policy response")
        };
        assert_eq!(policy.providers[0].models, vec!["configured"]);
        let RemoteReply::ProcessingSettings(mut settings) = client
            .execute(
                RemoteOperation::ProcessingSettings {
                    vault_id: me.personal_vault_id.clone(),
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
        else {
            panic!("native settings response")
        };
        assert!(settings.settings.summary_model.is_none());
        settings.settings.summary_model = Some(SummaryModelSelection {
            provider_id: "ollama".into(),
            model_id: "configured".into(),
            base_url: String::new(),
        });
        let outcome = client
            .execute(
                RemoteOperation::SaveProcessingSettings {
                    vault_id: me.personal_vault_id.clone(),
                    input: SaveProcessingSettingsRequest {
                        settings: settings.settings,
                        expected_revision: settings.revision,
                    },
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            RemoteReply::ProcessingSettingsSave(ProcessingSettingsSaveOutcome::Saved(_))
        ));
        let device_root = tempfile::tempdir().unwrap();
        let device_path = device_root.path().join("submissions");
        let scope = serde_json::to_string(&(
            "remote",
            client.address(),
            &me.member.id,
            &me.personal_vault_id,
        ))
        .unwrap();
        let device = JobSubmissions::new(device_path.clone()).unwrap();
        let receipt = device.prepare(scope.clone(), request).await.unwrap();
        let request_id = receipt.request_id.clone();
        let operation = RemoteOperation::SubmitSummaryJob {
            vault_id: me.personal_vault_id.clone(),
            input: receipt.input,
        };
        let cancel = CancellationToken::new();
        let pending = {
            let c = client.clone();
            let op = operation.clone();
            let id = request_id.clone();
            let token = cancel.clone();
            tokio::spawn(async move { c.execute(op, &id, token).await })
        };
        let job = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let page = context
                    .list_processing_jobs(me.member.id.clone(), Default::default())
                    .await
                    .unwrap();
                if let Some(job) = page.items.first() {
                    break job.clone();
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        assert_eq!(
            pending.await.unwrap().unwrap_err().mutation_outcome,
            Some(MutationOutcome::Unknown)
        );
        gate.cancel();
        assert_eq!(
            terminal(&context, &job.id).await.state,
            ProcessingJobState::Succeeded
        );
        drop(device);
        let device = JobSubmissions::new(device_path.clone()).unwrap();
        let saved = device
            .list(scope.clone(), None)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let saved = device
            .load(scope.clone(), saved.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.request_id, request_id);
        assert!(saved.job.is_none());
        let operation = RemoteOperation::SubmitSummaryJob {
            vault_id: me.personal_vault_id.clone(),
            input: saved.input,
        };
        let reply = client
            .execute(operation, &saved.request_id, CancellationToken::new())
            .await
            .unwrap();
        assert!(matches!(reply,RemoteReply::ProcessingJob(ref snapshot) if snapshot.id==job.id));
        let RemoteReply::ProcessingJob(snapshot) = reply else {
            unreachable!()
        };
        device
            .observe(scope.clone(), request_id.clone(), snapshot)
            .await
            .unwrap();
        drop(device);
        let device = JobSubmissions::new(device_path).unwrap();
        assert_eq!(
            device.list(scope, None).await.unwrap()[0]
                .job
                .as_ref()
                .unwrap()
                .state,
            ProcessingJobState::Succeeded
        );
        let reply = client
            .execute(
                RemoteOperation::ProcessingJobs {
                    vault_id: me.personal_vault_id.clone(),
                    input: ListProcessingJobsInput {
                        session_id: Some("note".into()),
                        ..Default::default()
                    },
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(
            matches!(reply,RemoteReply::ProcessingJobs(ref page) if page.items.len()==1 && page.items[0].id==job.id)
        );
        let reply = client
            .execute(
                RemoteOperation::ProcessingJobResult {
                    vault_id: me.personal_vault_id.clone(),
                    job_id: job.id.clone(),
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(
            matches!(reply,RemoteReply::ProcessingJobResult(Some(ref result)) if result.job_id==job.id && result.markdown=="# Release\n- Ship Friday.")
        );
        let reply = client
            .execute(
                RemoteOperation::ProcessingJob {
                    vault_id: me.personal_vault_id.clone(),
                    job_id: job.id.clone(),
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(
            matches!(reply,RemoteReply::ProcessingJob(ref snapshot) if snapshot.state==ProcessingJobState::Succeeded)
        );
        let reply = client
            .execute(
                RemoteOperation::CancelProcessingJob {
                    vault_id: me.personal_vault_id.clone(),
                    job_id: job.id.clone(),
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(
            matches!(reply,RemoteReply::ProcessingJob(ref snapshot) if snapshot.state==ProcessingJobState::Succeeded)
        );
        assert_eq!(provider.requests.lock().unwrap().len(), 1);
        client.disconnect();
        fixture.service.shutdown().await.unwrap();
        server.abort();
        provider.close().await;
    }
}

#[tokio::test]
async fn native_sdk_rejects_an_unsupported_job_capability_before_authentication_or_dispatch() {
    let fixture = Fixture::new().await;
    let gate = CancellationToken::new();
    gate.cancel();
    let (client, server) = native_client(&fixture, 0, gate).await;
    let id = uuid::Uuid::new_v4().to_string();
    let result = client
        .execute(
            RemoteOperation::SubmitSummaryJob {
                vault_id: uuid::Uuid::new_v4().to_string(),
                input: SummaryJobRequest {
                    only_if_empty: false,
                    title_policy: Default::default(),
                    session_id: "note".into(),
                    target: DocumentTarget::Summary,
                    expected_revision: "revision".into(),
                },
            },
            &id,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(result.code, ErrorCode::IncompatibleVersion);
    assert_eq!(result.mutation_outcome, Some(MutationOutcome::NotCommitted));
    client.disconnect();
    fixture.service.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn native_sdk_round_trips_maximum_json_escaped_processing_prompt() {
    let provider = Provider::new(0).await;
    let mut config = provider.config();
    config.allow_custom_templates = true;
    let fixture = Fixture::with_processing(Some(config)).await;
    let gate = CancellationToken::new();
    gate.cancel();
    let (client, server) = native_client(&fixture, 0, gate).await;
    let login = client.begin_login().await.unwrap();
    let url = login.authorization_url().to_string();
    let complete = tokio::spawn(login.complete());
    assert!(reqwest::get(url).await.unwrap().status().is_success());
    complete.await.unwrap().unwrap();
    let RemoteReply::Me(me) = client
        .execute(
            RemoteOperation::Provision,
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap()
    else {
        panic!("provision response")
    };
    let vault_id = me.personal_vault_id;
    let RemoteReply::ProcessingSettings(mut before) = client
        .execute(
            RemoteOperation::ProcessingSettings {
                vault_id: vault_id.clone(),
            },
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap()
    else {
        panic!("settings response")
    };
    before.settings.summary_prompt = "\u{1}".repeat(64 * 1024);
    let input = SaveProcessingSettingsRequest {
        settings: before.settings,
        expected_revision: before.revision,
    };
    assert!(serde_json::to_vec(&input).unwrap().len() > 256 * 1024);
    let RemoteReply::ProcessingSettingsSave(ProcessingSettingsSaveOutcome::Saved(saved)) = client
        .execute(
            RemoteOperation::SaveProcessingSettings {
                vault_id: vault_id.clone(),
                input: input.clone(),
            },
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap()
    else {
        panic!("save response")
    };
    assert_eq!(saved.settings, input.settings);
    let RemoteReply::ProcessingSettings(read) = client
        .execute(
            RemoteOperation::ProcessingSettings { vault_id },
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap()
    else {
        panic!("read response")
    };
    assert_eq!(read, saved);
    assert!(provider.requests.lock().unwrap().is_empty());
    client.disconnect();
    fixture.service.shutdown().await.unwrap();
    server.abort();
    provider.close().await;
}

#[tokio::test]
async fn native_title_requests_recover_lost_replies_without_repeating_either_model_call() {
    for rtt in [50, 150, 500] {
        let title = if rtt == 150 {
            "invalid\nmultiline"
        } else {
            "Friday release"
        };
        let provider =
            Provider::with_outputs(2, vec!["# Release\n- Ship Friday.".into(), title.into()]).await;
        let fixture = Fixture::with_processing(Some(provider.config())).await;
        let reply_gate = CancellationToken::new();
        let (client, server) = native_client(&fixture, rtt, reply_gate.clone()).await;
        assert!(
            client
                .capabilities()
                .features
                .iter()
                .any(|value| value == "generated_titles")
        );
        let login = client.begin_login().await.unwrap();
        let url = login.authorization_url().to_string();
        let complete = tokio::spawn(login.complete());
        assert!(reqwest::get(url).await.unwrap().status().is_success());
        complete.await.unwrap().unwrap();
        let RemoteReply::Me(me) = client
            .execute(
                RemoteOperation::Provision,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
        else {
            panic!("provision response")
        };
        let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
        prepare(&context).await;
        let baseline = context.read_metadata("note").await.unwrap();
        assert!(matches!(
            context
                .save_metadata(
                    "note",
                    SaveMetadataRequest {
                        patch: SessionMetaPatch {
                            title: Some(String::new()),
                            ..Default::default()
                        },
                        expected_revision: baseline.revision,
                    }
                )
                .await
                .unwrap(),
            MetadataSaveOutcome::Saved(_)
        ));
        let note_before = context.read_note("note").await.unwrap();
        let mut input = input(&context).await;
        input.title_policy = TitlePolicy::GenerateIfEmpty;
        let operation = RemoteOperation::SubmitSummaryJob {
            vault_id: me.personal_vault_id.clone(),
            input,
        };
        let request_id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let pending = {
            let (client, operation, request_id, cancel) = (
                client.clone(),
                operation.clone(),
                request_id.clone(),
                cancel.clone(),
            );
            tokio::spawn(async move { client.execute(operation, &request_id, cancel).await })
        };
        let job = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let page = context
                    .list_processing_jobs(me.member.id.clone(), Default::default())
                    .await
                    .unwrap();
                if let Some(job) = page.items.first() {
                    break job.clone();
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        assert_eq!(
            pending.await.unwrap().unwrap_err().mutation_outcome,
            Some(MutationOutcome::Unknown)
        );
        reply_gate.cancel();
        assert_eq!(
            terminal(&context, &job.id).await.state,
            ProcessingJobState::Succeeded
        );
        for _ in 0..2 {
            let reply = client
                .execute(operation.clone(), &request_id, CancellationToken::new())
                .await
                .unwrap();
            assert!(
                matches!(reply, RemoteReply::ProcessingJob(ref replay) if replay.id == job.id && replay.state == ProcessingJobState::Succeeded)
            );
        }
        let RemoteReply::ProcessingJobResult(Some(result)) = client
            .execute(
                RemoteOperation::ProcessingJobResult {
                    vault_id: me.personal_vault_id.clone(),
                    job_id: job.id.clone(),
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
        else {
            panic!("private result")
        };
        assert_eq!(result.markdown, "# Release\n- Ship Friday.");
        let saved_body = context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .unwrap()
            .markdown;
        let title_result = result.title.unwrap();
        let current = context.read_note("note").await.unwrap();
        if rtt == 150 {
            assert_eq!(saved_body, result.markdown);
            assert_eq!(current.markdown, note_before.markdown);
            assert_eq!(title_result.status, ProcessingTitleStatus::Failed);
            assert_eq!(
                title_result.failure,
                Some(ProcessingJobFailure::InvalidOutput)
            );
            assert!(current.title.is_empty());
        } else {
            assert_eq!(title_result.status, ProcessingTitleStatus::Applied);
            assert_eq!(current.title, title);
            assert_eq!(saved_body, "# Friday release\n\n# Release\n- Ship Friday.");
            assert_eq!(
                current.markdown.as_deref(),
                Some("# Friday release\n\nShip Friday. Keep this source note.")
            );
        }
        assert_eq!(provider.requests.lock().unwrap().len(), 2);
        client.disconnect();
        fixture.service.shutdown().await.unwrap();
        server.abort();
        provider.close().await;
    }
}

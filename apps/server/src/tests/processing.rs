use super::*;
use axum::{extract::State, response::Response};
use std::{collections::HashMap, sync::Mutex, time::Duration};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

struct Provider {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    finish: Arc<Semaphore>,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
#[derive(Clone)]
struct ProviderState {
    requests: Arc<Mutex<Vec<Value>>>,
    finish: Arc<Semaphore>,
    stop: CancellationToken,
}
impl Provider {
    async fn new(finish: usize) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let state = ProviderState {
            requests: Arc::new(Mutex::new(Vec::new())),
            finish: Arc::new(Semaphore::new(finish)),
            stop: CancellationToken::new(),
        };
        let app = Router::new()
            .route("/v1/chat/completions", axum::routing::post(model))
            .with_state(state.clone());
        let stop = state.stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            base_url,
            requests: state.requests,
            finish: state.finish,
            stop: state.stop,
            task: Some(task),
        }
    }
    fn config(&self) -> crate::processing::ProcessingConfig {
        serde_json::from_value(json!({"providers":[{"id":"ollama","base_url":self.base_url,"models":["configured"],"include_images":false}],"concurrent_jobs":1,"waiting_jobs":2})).unwrap()
    }
    async fn close(mut self) {
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}
async fn model(State(state): State<ProviderState>, Json(body): Json<Value>) -> Response {
    assert_eq!(body["model"], "configured");
    state.requests.lock().unwrap().push(body);
    let text = json!({"choices":[{"index":0,"delta":{"content":"# Release\n- Ship Friday."},"finish_reason":null}]});
    let finish = json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
    Response::builder().header("content-type","text/event-stream").body(Body::from_stream(futures_util::stream::unfold((state,0,text,finish),|(state,step,text,finish)|async move {
        if step==0 {return Some((Ok::<_,std::io::Error>(axum::body::Bytes::from(format!("data: {text}\n\n"))),(state,1,text,finish)));}
        if step==1 {
            tokio::select!{biased;_ = state.stop.cancelled()=>return None, permit=state.finish.acquire()=>permit.unwrap().forget()}
            return Some((Ok(axum::body::Bytes::from(format!("data: {finish}\n\ndata: [DONE]\n\n"))),(state,2,text,finish)));
        }
        None
    }))).unwrap()
}
async fn prepare(context: &hypr_loofah_core::VaultContext) {
    context
        .settings
        .set_values(HashMap::from([
            ("current_llm_provider".into(), json!("ollama")),
            ("current_llm_model".into(), json!("configured")),
        ]))
        .await
        .unwrap();
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Release","created_at":"2026-11-07T10:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Ship Friday. Keep this source note.")
        .await
        .unwrap();
    context.store.ensure_summary("note").await.unwrap();
}
async fn submit(context: &hypr_loofah_core::VaultContext, member: &str) -> ProcessingJobSnapshot {
    let baseline = context
        .read_document("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    context
        .processing_jobs()
        .unwrap()
        .submit(
            member.into(),
            uuid::Uuid::new_v4().to_string(),
            SummaryJobRequest {
                session_id: "note".into(),
                target: DocumentTarget::Summary,
                expected_revision: baseline.revision,
            },
        )
        .await
        .unwrap()
}
async fn terminal(context: &hypr_loofah_core::VaultContext, id: &str) -> ProcessingJobSnapshot {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = context
                .processing_jobs()
                .unwrap()
                .get(id.into())
                .await
                .unwrap()
                .unwrap();
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn configured_headless_service_generates_from_vault_settings_and_rejects_foreign_members() {
    let provider = Provider::new(2).await;
    let fixture = Fixture::with_processing(Some(provider.config())).await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let context = fixture
        .service
        .vault(&alice.personal_vault_id)
        .await
        .unwrap();
    prepare(&context).await;
    let note = context.store.vault_base().join("sessions/note/notes.md");
    let before = std::fs::read(&note).unwrap();
    let job = submit(&context, &alice.member.id).await;
    assert_eq!(
        terminal(&context, &job.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert_eq!(
        context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .unwrap()
            .markdown,
        "# Release\n- Ship Friday."
    );
    assert_eq!(std::fs::read(note).unwrap(), before);
    let foreign = submit(&context, &bob.member.id).await;
    let denied = terminal(&context, &foreign.id).await;
    assert_eq!(denied.failure, Some(ProcessingJobFailure::AccessDenied));
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    fixture.service.shutdown().await.unwrap();
    provider.close().await;
}

#[tokio::test]
async fn membership_downgrade_cancels_owned_provider_work_without_applying_partial_output() {
    let provider = Provider::new(0).await;
    let fixture = Fixture::with_processing(Some(provider.config())).await;
    fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let identity = crate::auth::Identity {
        issuer: fixture.issuer.clone(),
        subject: "alice".into(),
        name: "Alice".into(),
        expires_at: now() + 3600,
    };
    let shared = fixture
        .service
        .catalog
        .create_shared(&identity, &uuid::Uuid::new_v4().to_string(), "Team")
        .await
        .unwrap();
    let added = fixture
        .service
        .catalog
        .set_membership(
            &identity,
            &shared.id,
            &bob.member.id,
            Some(VaultRole::Editor),
            &shared.revision,
        )
        .await
        .unwrap();
    let context = fixture.service.vault(&shared.id).await.unwrap();
    prepare(&context).await;
    let job = submit(&context, &bob.member.id).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while provider.requests.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    fixture
        .service
        .catalog
        .set_membership(
            &identity,
            &shared.id,
            &bob.member.id,
            Some(VaultRole::Reader),
            &added.revision,
        )
        .await
        .unwrap();
    let failed = terminal(&context, &job.id).await;
    assert_eq!(failed.failure, Some(ProcessingJobFailure::AccessDenied));
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
    provider.finish.add_permits(1);
    fixture.service.shutdown().await.unwrap();
    provider.close().await;
}

#[tokio::test]
async fn processing_capacity_is_shared_between_vaults_and_explicit_retry_works_after_drain() {
    let provider = Provider::new(0).await;
    let mut config = provider.config();
    config.waiting_jobs = 0;
    let fixture = Fixture::with_processing(Some(config)).await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let first = fixture
        .service
        .vault(&alice.personal_vault_id)
        .await
        .unwrap();
    let second = fixture.service.vault(&bob.personal_vault_id).await.unwrap();
    prepare(&first).await;
    prepare(&second).await;
    let active = submit(&first, &alice.member.id).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while provider.requests.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let rejected = submit(&second, &bob.member.id).await;
    assert_eq!(
        terminal(&second, &rejected.id).await.failure,
        Some(ProcessingJobFailure::ResourceLimit)
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert_eq!(
        second
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .unwrap()
            .markdown,
        ""
    );
    provider.finish.add_permits(1);
    assert_eq!(
        terminal(&first, &active.id).await.state,
        ProcessingJobState::Succeeded
    );
    let retry = submit(&second, &bob.member.id).await;
    provider.finish.add_permits(1);
    assert_eq!(
        terminal(&second, &retry.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    fixture.service.shutdown().await.unwrap();
    provider.close().await;
}

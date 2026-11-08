use super::*;
use crate::application;
use hypr_loofah_core::{search::SearchService, settings::ConfigState, startup::StartupState};
use hypr_loofah_protocol::{DocumentTarget, ProcessingJobState, SummaryJobRequest};
use serde_json::json;
use tauri::{Listener, Manager};

#[tokio::test]
async fn local_ipc_jobs_use_the_shared_worker_and_anonymous_model_without_login() {
    use axum::{Router, response::IntoResponse, routing::post};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let provider = Router::new().route("/v1/chat/completions", post(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        async {
            ([ ("content-type", "text/event-stream") ], concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"# Local result\\nShip Friday.\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            )).into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server_stop = CancellationToken::new();
    let stop = server_stop.clone();
    let provider = tokio::spawn(async move {
        axum::serve(listener, provider)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
            .unwrap();
    });
    let vault = tempfile::tempdir().unwrap();
    let operations = tempfile::tempdir().unwrap();
    let context = Arc::new(
        VaultContext::new(
            "local:test-vault".into(),
            Arc::new(hypr_vault_write::SessionStore::new(vault.path().into())),
            StartupState::new(Some(vault.path())),
            Arc::new(ConfigState::load_or_default(vault.path())),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(operations.path().into()),
    );
    context
        .store
        .create_note_session(
            &serde_json::from_value(json!({
                "id":"note", "title":"Local note", "created_at":"2026-11-07T10:00:00Z", "tags":[]
            }))
            .unwrap(),
            "Keep my original notes.",
        )
        .await
        .unwrap();
    context.store.ensure_summary("note").await.unwrap();
    let input = SummaryJobRequest {
        title_policy: Default::default(),
        session_id: "note".into(),
        target: DocumentTarget::Summary,
        expected_revision: context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap()
            .revision,
    };
    let app = tauri::test::mock_builder()
        .manage(ProcessingState::default())
        .manage(context.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    tauri_specta::Builder::<tauri::test::MockRuntime>::new()
        .events(tauri_specta::collect_events![
            application::ApplicationChanged
        ])
        .mount_events(app.handle());
    let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    app.listen("application-changed", move |event| {
        let event: application::ApplicationChanged = serde_json::from_str(event.payload()).unwrap();
        let _ = events_tx.send(event.event.change);
    });
    application::forward_changes(app.handle(), &context);
    assert!(!application::application_processing_available(
        app.handle().clone()
    ));
    app.state::<ProcessingState>()
        .start(app.handle().clone(), &context)
        .await
        .unwrap();
    assert!(application::application_processing_available(
        app.handle().clone()
    ));
    let policy = application::application_processing_policy(app.handle().clone()).unwrap();
    assert!(
        policy
            .providers
            .iter()
            .any(|provider| provider.id == "chatgpt_subscription")
    );
    let mut settings = application::application_processing_settings(app.handle().clone())
        .await
        .unwrap();
    assert!(settings.settings.summary_model.is_none());
    settings.settings.summary_model = Some(hypr_loofah_protocol::SummaryModelSelection {
        provider_id: "ollama".into(),
        model_id: "configured".into(),
        base_url,
    });
    let outcome = application::application_save_processing_settings(
        app.handle().clone(),
        hypr_loofah_protocol::SaveProcessingSettingsRequest {
            settings: settings.settings,
            expected_revision: settings.revision,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        outcome,
        hypr_loofah_protocol::ProcessingSettingsSaveOutcome::Saved(_)
    ));
    let id = uuid::Uuid::new_v4().to_string();
    let submitted = application::application_submit_summary_job(
        app.handle().clone(),
        id.clone(),
        input.clone(),
    )
    .await
    .unwrap();
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let job =
                application::application_processing_job(app.handle().clone(), submitted.id.clone())
                    .await
                    .unwrap();
            if job.state.is_terminal() {
                break job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        terminal.state,
        ProcessingJobState::Succeeded,
        "{terminal:?}"
    );
    let replay = application::application_submit_summary_job(app.handle().clone(), id, input)
        .await
        .unwrap();
    assert_eq!(replay.id, submitted.id);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let result =
        application::application_processing_job_result(app.handle().clone(), submitted.id.clone())
            .await
            .unwrap()
            .unwrap();
    assert!(result.markdown.contains("Ship Friday."));
    assert!(
        context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .unwrap()
            .markdown
            .contains("Ship Friday.")
    );
    assert_eq!(
        context.read_note("note").await.unwrap().markdown.as_deref(),
        Some("Keep my original notes.")
    );
    let list = application::application_processing_jobs(app.handle().clone(), Default::default())
        .await
        .unwrap();
    assert_eq!(list.items.len(), 1);
    assert_eq!(
        application::application_cancel_processing_job(app.handle().clone(), submitted.id.clone())
            .await
            .unwrap()
            .state,
        ProcessingJobState::Succeeded
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if matches!(events.recv().await.unwrap(), hypr_loofah_protocol::Change::JobChanged { job_id } if job_id == submitted.id) { break; }
        }
    }).await.unwrap();
    app.state::<ProcessingState>().shutdown().await.unwrap();
    assert!(!application::application_processing_available(
        app.handle().clone()
    ));
    assert!(
        application::application_processing_job_result(app.handle().clone(), submitted.id)
            .await
            .unwrap()
            .is_some()
    );
    server_stop.cancel();
    provider.await.unwrap();
}

#[tokio::test]
async fn only_the_fixed_local_principal_can_authorize_device_jobs() {
    let mut work = JobWork {
        principal: PRINCIPAL.into(),
        request: SummaryJobRequest {
            title_policy: Default::default(),
            session_id: "note".into(),
            target: DocumentTarget::Summary,
            expected_revision: "revision".into(),
        },
    };
    assert!(LocalOwner.authorize(&work).await.is_ok());
    work.principal = "company-member".into();
    assert!(matches!(
        LocalOwner.authorize(&work).await,
        Err(ProcessingJobFailure::AccessDenied)
    ));
}

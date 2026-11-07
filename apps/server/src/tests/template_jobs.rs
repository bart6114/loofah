use super::processing::{Provider, prepare, terminal};
use super::*;
use hypr_loofah_client::{RemoteOperation, RemoteReply};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const PREFIX: &str = "---\n# Keep output metadata\nkind: template_output\ntitle: Existing output\ntemplate_id: retired-template\nsort_order: 7\nfuture: {keep: true}\n---\n\n";

async fn output(context: &hypr_loofah_core::VaultContext) -> DocumentTarget {
    context
        .store
        .write_enhanced_doc(&EnhancedDoc {
            id: "output".into(),
            session_id: "note".into(),
            kind: "template_output".into(),
            title: "Existing output".into(),
            template_id: "retired-template".into(),
            sort_order: 7,
            markdown: "Before".into(),
        })
        .await
        .unwrap();
    std::fs::write(
        context
            .store
            .vault_base()
            .join("sessions/note/enhanced/output.md"),
        format!("{PREFIX}Before"),
    )
    .unwrap();
    DocumentTarget::TemplateOutput {
        document_id: "output".into(),
    }
}

#[tokio::test]
async fn template_jobs_require_editor_access_and_preserve_existing_output_metadata() {
    let provider = Provider::new(1).await;
    let fixture = Fixture::with_processing(Some(provider.config())).await;
    fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Outputs"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
                Some(json!({"role":"reader","expected_revision":shared["revision"]}))
            )
            .await
            .0,
        StatusCode::OK
    );
    let context = fixture.service.vault(vault).await.unwrap();
    prepare(&context).await;
    let target = output(&context).await;
    let source_note =
        std::fs::read(context.store.vault_base().join("sessions/note/notes.md")).unwrap();
    let input = SummaryJobRequest {
        session_id: "note".into(),
        expected_revision: context
            .read_document("note", &target)
            .await
            .unwrap()
            .revision,
        target: target.clone(),
    };
    let path = format!("/api/v1/vaults/{vault}/jobs");
    assert_eq!(
        fixture
            .call(
                "bob",
                "POST",
                &path,
                Some(serde_json::to_value(&input).unwrap())
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, body) = fixture
        .call(
            "alice",
            "POST",
            &path,
            Some(serde_json::to_value(&input).unwrap()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let job: ProcessingJobSnapshot = serde_json::from_value(body).unwrap();
    assert_eq!(
        terminal(&context, &job.id).await.state,
        ProcessingJobState::Succeeded
    );
    let canonical = context.read_document("note", &target).await.unwrap();
    let doc = canonical.document.unwrap();
    assert_eq!(doc.markdown, "# Release\n- Ship Friday.");
    assert_eq!(doc.title, "Existing output");
    assert_eq!(doc.template_id, "retired-template");
    assert_eq!(doc.sort_order, 7);
    assert_eq!(
        std::fs::read_to_string(
            context
                .store
                .vault_base()
                .join("sessions/note/enhanced/output.md")
        )
        .unwrap(),
        format!("{PREFIX}# Release\n- Ship Friday.")
    );
    assert_eq!(
        std::fs::read(context.store.vault_base().join("sessions/note/notes.md")).unwrap(),
        source_note
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert_eq!(
        fixture
            .call("bob", "GET", &format!("{path}/{}/result", job.id), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (_, capabilities) = fixture
        .call("alice", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("template_output_jobs"))
    );
    fixture.service.shutdown().await.unwrap();
    provider.close().await;
}

#[tokio::test]
async fn missing_and_legacy_template_targets_fail_before_model_calls_without_creating_outputs() {
    let provider = Provider::new(1).await;
    let fixture = Fixture::with_processing(Some(provider.config())).await;
    let me = fixture.login("alice").await;
    let vault = &me.personal_vault_id;
    let context = fixture.service.vault(vault).await.unwrap();
    prepare(&context).await;
    let directory = context.store.vault_base().join("sessions/note/enhanced");
    std::fs::create_dir_all(&directory).unwrap();
    let legacy = "---\nkind: summary\ntitle: Legacy\n---\n\nLegacy body";
    std::fs::write(directory.join("legacy.md"), legacy).unwrap();
    let summary =
        std::fs::read(context.store.vault_base().join("sessions/note/summary.md")).unwrap();
    for document_id in ["missing", "legacy"] {
        let target = DocumentTarget::TemplateOutput {
            document_id: document_id.into(),
        };
        let expected_revision = if document_id == "missing" {
            context
                .read_document("note", &target)
                .await
                .unwrap()
                .revision
        } else {
            "legacy-revision".into()
        };
        let input = SummaryJobRequest {
            session_id: "note".into(),
            target,
            expected_revision,
        };
        let (status, body) = fixture
            .call(
                "alice",
                "POST",
                &format!("/api/v1/vaults/{vault}/jobs"),
                Some(serde_json::to_value(input).unwrap()),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let job: ProcessingJobSnapshot = serde_json::from_value(body).unwrap();
        assert_eq!(
            terminal(&context, &job.id).await.failure,
            Some(ProcessingJobFailure::SourceUnavailable)
        );
    }
    assert!(provider.requests.lock().unwrap().is_empty());
    assert!(!directory.join("missing.md").exists());
    assert_eq!(
        std::fs::read_to_string(directory.join("legacy.md")).unwrap(),
        legacy
    );
    assert_eq!(
        std::fs::read(context.store.vault_base().join("sessions/note/summary.md")).unwrap(),
        summary
    );
    fixture.service.shutdown().await.unwrap();
    provider.close().await;
}

#[tokio::test]
async fn native_template_jobs_replay_lost_replies_without_overwriting_newer_output_at_realistic_latency()
 {
    for rtt in [50, 150, 500] {
        let provider = Provider::new(1).await;
        let fixture = Fixture::with_processing(Some(provider.config())).await;
        let gate = CancellationToken::new();
        let (client, server) =
            super::processing_api::native_client(&fixture, rtt, gate.clone()).await;
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
            panic!("native provision response")
        };
        let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
        prepare(&context).await;
        let target = output(&context).await;
        let operation = RemoteOperation::SubmitSummaryJob {
            vault_id: me.personal_vault_id.clone(),
            input: SummaryJobRequest {
                session_id: "note".into(),
                expected_revision: context
                    .read_document("note", &target)
                    .await
                    .unwrap()
                    .revision,
                target: target.clone(),
            },
        };
        let request_id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let pending = tokio::spawn({
            let client = client.clone();
            let operation = operation.clone();
            let request_id = request_id.clone();
            let cancel = cancel.clone();
            async move { client.execute(operation, &request_id, cancel).await }
        });
        let job = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let jobs = context
                    .list_processing_jobs(me.member.id.clone(), ListProcessingJobsInput::default())
                    .await
                    .unwrap();
                if let Some(job) = jobs.items.into_iter().next() {
                    break job;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            terminal(&context, &job.id).await.state,
            ProcessingJobState::Succeeded
        );
        cancel.cancel();
        assert_eq!(
            pending.await.unwrap().unwrap_err().mutation_outcome,
            Some(MutationOutcome::Unknown)
        );
        gate.cancel();
        let path = context
            .store
            .vault_base()
            .join("sessions/note/enhanced/output.md");
        std::fs::write(&path, format!("{PREFIX}Newer output after a lost reply")).unwrap();
        let RemoteReply::ProcessingJob(replayed) = client
            .execute(operation, &request_id, CancellationToken::new())
            .await
            .unwrap()
        else {
            panic!("native template job response")
        };
        assert_eq!(replayed.id, job.id);
        assert_eq!(replayed.state, ProcessingJobState::Succeeded);
        assert_eq!(provider.requests.lock().unwrap().len(), 1);
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            format!("{PREFIX}Newer output after a lost reply")
        );
        fixture.service.shutdown().await.unwrap();
        server.abort();
        provider.close().await;
    }
}

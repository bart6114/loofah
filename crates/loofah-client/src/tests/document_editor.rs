use super::*;
use axum::{body::Body, extract::Path, response::Response};

fn target(params: &HashMap<String, String>) -> DocumentTarget {
    params
        .get("document")
        .map_or(DocumentTarget::Summary, |id| {
            DocumentTarget::TemplateOutput {
                document_id: id.clone(),
            }
        })
}
fn snapshot(session: &str, target: &DocumentTarget) -> Value {
    let (kind, id, source) = match target {
        DocumentTarget::Summary => ("summary", session, "session_summary"),
        DocumentTarget::TemplateOutput { document_id } => {
            ("template_output", document_id.as_str(), "enhanced_note")
        }
    };
    json!({
        "document":{"document":{"id":id,"session_id":session,"kind":kind,"title":"Output label","template_id":"template","sort_order":2,"markdown":"# Session title\nBody"},"revision":"document-revision"},
        "metadata":{"meta":{"id":session,"title":"Session title","created_at":"2026-11-08T10:00:00Z","tags":[],"future":{"keep":true}},"revision":"metadata-revision"},
        "tasks":{"tasks":[{"id":"task","source_type":source,"source_id":id,"source_order":0,"status":"todo","text":"Rich task","body":[{"type":"paragraph","content":[{"type":"text","text":"Rich task"}]}],"due_at":"","assignee":"person","created_at":"2026-11-08T10:00:00Z","updated_at":"2026-11-08T10:00:00Z"}],"revision":"whole-tasks-revision"}
    })
}
fn malformed(mut value: Value, mode: usize) -> Value {
    match mode {
        2 => value["metadata"]["meta"]["id"] = "foreign".into(),
        3 => value["document"]["document"]["session_id"] = "foreign".into(),
        4 => value["document"]["document"]["kind"] = "raw_note".into(),
        5 => value["tasks"]["revision"] = "".into(),
        6 => value["tasks"]["tasks"][0]["source_id"] = "foreign".into(),
        7 => {
            let duplicate = value["tasks"]["tasks"][0].clone();
            value["tasks"]["tasks"]
                .as_array_mut()
                .unwrap()
                .push(duplicate);
        }
        8 => value["document"]["document"] = Value::Null,
        9 => value["document"]["document"]["id"] = "foreign".into(),
        _ => {}
    }
    value
}
fn oversized() -> Response {
    Response::builder()
        .header("content-type", "application/json")
        .body(Body::from(vec![b' '; DOCUMENT_EDITOR_RESPONSE_LIMIT + 1]))
        .unwrap()
}

pub(super) async fn read(
    State(state): State<Arc<Provider>>,
    Path(params): Path<HashMap<String, String>>,
) -> Response {
    let mode = state.document_editor_mode.load(Ordering::SeqCst);
    if mode == 12 {
        return oversized();
    }
    Json(malformed(
        snapshot(&params["session"], &target(&params)),
        mode,
    ))
    .into_response()
}
pub(super) async fn save(
    State(state): State<Arc<Provider>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    Json(input): Json<SaveDocumentEditorRequest>,
) -> Response {
    state.document_editor_calls.lock().await.push(json!({
        "session":params["session"],"target":target(&params),
        "request_id":headers.get("x-request-id").unwrap().to_str().unwrap(),
        "authorization":headers.get("authorization").unwrap().to_str().unwrap(),
        "input":input,
    }));
    let mode = state.document_editor_mode.load(Ordering::SeqCst);
    if mode == 12 {
        return oversized();
    }
    let conflict_status = mode == 1 || mode == 11;
    let outcome = if mode == 1 || mode == 10 {
        "conflict"
    } else {
        "saved"
    };
    let value = malformed(snapshot(&params["session"], &target(&params)), mode);
    (
        if conflict_status {
            StatusCode::CONFLICT
        } else {
            StatusCode::OK
        },
        Json(json!({"kind":outcome,"snapshot":value})),
    )
        .into_response()
}
fn input() -> SaveDocumentEditorRequest {
    SaveDocumentEditorRequest {
        markdown: "# Edited session title\nChecked edit".into(),
        title: Some("Edited session title".into()),
        tasks: vec![],
        expected_revision: "document-revision".into(),
        expected_metadata_revision: "metadata-revision".into(),
        expected_tasks_revision: "whole-tasks-revision".into(),
    }
}
fn operation(target: DocumentTarget, save: bool) -> RemoteOperation {
    if save {
        RemoteOperation::SaveDocumentEditor {
            vault_id: "test".into(),
            session_id: "note".into(),
            target,
            input: input(),
        }
    } else {
        RemoteOperation::DocumentEditor {
            vault_id: "test".into(),
            session_id: "note".into(),
            target,
        }
    }
}
fn support(client: &mut Arc<RemoteClient>) {
    Arc::get_mut(client).unwrap().capabilities.features.extend([
        "document_editor_read".into(),
        "document_editor_write".into(),
    ]);
}

#[tokio::test]
async fn document_editor_capabilities_invalid_routes_and_cancellation_precede_credentials() {
    let fixture = Fixture::new().await;
    let mut client = fixture.client().await;
    for save in [false, true] {
        let error = client
            .execute(
                operation(DocumentTarget::Summary, save),
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::IncompatibleVersion);
        assert_eq!(
            error.mutation_outcome,
            save.then_some(MutationOutcome::NotCommitted)
        );
    }
    support(&mut client);
    for save in [false, true] {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = client
            .execute(
                operation(DocumentTarget::Summary, save),
                &uuid::Uuid::new_v4().to_string(),
                cancel,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Cancelled);
        assert_eq!(
            error.mutation_outcome,
            save.then_some(MutationOutcome::NotCommitted)
        );
        let error = client
            .execute(
                operation(DocumentTarget::Summary, save),
                "not-a-uuid",
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(
            error.mutation_outcome,
            save.then_some(MutationOutcome::NotCommitted)
        );
        let error = client
            .execute(
                operation(
                    DocumentTarget::TemplateOutput {
                        document_id: "../foreign".into(),
                    },
                    save,
                ),
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(
            error.mutation_outcome,
            save.then_some(MutationOutcome::NotCommitted)
        );
    }
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.issued.load(Ordering::SeqCst), 0);
    assert!(fixture.state.document_editor_calls.lock().await.is_empty());
}

#[tokio::test]
async fn document_editor_tls_reads_saves_and_conflicts_preserve_target_and_three_baselines() {
    for millis in [50, 150, 500] {
        let fixture = Fixture::with_tls(Some(Duration::from_millis(millis))).await;
        let mut client = fixture.client().await;
        support(&mut client);
        fixture.login(&client).await;
        for target in [
            DocumentTarget::Summary,
            DocumentTarget::TemplateOutput {
                document_id: "output".into(),
            },
        ] {
            let expected: DocumentEditorSnapshot =
                serde_json::from_value(snapshot("note", &target)).unwrap();
            let reply = client
                .execute(
                    operation(target.clone(), false),
                    &uuid::Uuid::new_v4().to_string(),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            assert!(matches!(reply,RemoteReply::DocumentEditor(value) if value==expected));
            for (mode, conflict) in [(0, false), (1, true)] {
                fixture
                    .state
                    .document_editor_mode
                    .store(mode, Ordering::SeqCst);
                let id = uuid::Uuid::new_v4().to_string();
                let reply = client
                    .execute(
                        operation(target.clone(), true),
                        &id,
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap();
                match reply {
                    RemoteReply::DocumentEditorSave(DocumentEditorSaveOutcome::Saved(value))
                        if !conflict =>
                    {
                        assert_eq!(value, expected)
                    }
                    RemoteReply::DocumentEditorSave(DocumentEditorSaveOutcome::Conflict(value))
                        if conflict =>
                    {
                        assert_eq!(value, expected)
                    }
                    other => panic!("Unexpected response: {other:?}"),
                }
                let calls = fixture.state.document_editor_calls.lock().await;
                let last = calls.last().unwrap();
                assert_eq!(last["request_id"], id);
                assert_eq!(last["target"], serde_json::to_value(&target).unwrap());
                assert_eq!(last["input"], serde_json::to_value(input()).unwrap());
                assert!(
                    last["authorization"]
                        .as_str()
                        .unwrap()
                        .starts_with("Bearer ")
                );
            }
            fixture
                .state
                .document_editor_mode
                .store(0, Ordering::SeqCst);
        }
    }
}

#[tokio::test]
async fn document_editor_rejects_cross_resource_and_malformed_replies_and_retains_uncertain_writes()
{
    let fixture = Fixture::new().await;
    let mut client = fixture.client().await;
    support(&mut client);
    fixture.login(&client).await;
    let target = DocumentTarget::TemplateOutput {
        document_id: "output".into(),
    };
    for mode in 2..=9 {
        fixture
            .state
            .document_editor_mode
            .store(mode, Ordering::SeqCst);
        for save in [false, true] {
            let error = client
                .execute(
                    operation(target.clone(), save),
                    &uuid::Uuid::new_v4().to_string(),
                    CancellationToken::new(),
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::IncompatibleVersion, "mode {mode}");
            assert_eq!(
                error.mutation_outcome,
                save.then_some(MutationOutcome::Unknown),
                "mode {mode}"
            );
        }
    }
    for mode in [10, 11] {
        fixture
            .state
            .document_editor_mode
            .store(mode, Ordering::SeqCst);
        let error = client
            .execute(
                operation(target.clone(), true),
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::IncompatibleVersion);
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
    }
    fixture
        .state
        .document_editor_mode
        .store(8, Ordering::SeqCst);
    assert!(matches!(
        client
            .execute(
                operation(DocumentTarget::Summary, false),
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new()
            )
            .await
            .unwrap(),
        RemoteReply::DocumentEditor(DocumentEditorSnapshot {
            document: DocumentSnapshot { document: None, .. },
            ..
        })
    ));
    fixture
        .state
        .document_editor_mode
        .store(12, Ordering::SeqCst);
    for save in [false, true] {
        let error = client
            .execute(
                operation(target.clone(), save),
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimit);
        assert_eq!(
            error.mutation_outcome,
            save.then_some(MutationOutcome::Unknown)
        );
    }
}

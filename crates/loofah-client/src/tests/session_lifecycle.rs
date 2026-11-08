use super::*;
use axum::Extension;

const VAULT: &str = "40992f67-31f6-45d7-93e5-c3ad20368b85";
struct Lifecycle {
    mode: AtomicUsize,
}
impl Lifecycle {
    fn routes(self: &Arc<Self>) -> Router<Arc<Provider>> {
        Router::new()
            .route(
                &format!("/api/v1/vaults/{VAULT}/sessions/note/lifecycle"),
                get(read),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/session-deletions"),
                post(delete),
            )
            .route(
                &format!("/api/v1/vaults/{VAULT}/session-restorations"),
                post(restore),
            )
            .layer(Extension(self.clone()))
    }
}
fn revision() -> String {
    "a".repeat(64)
}
fn token() -> String {
    "b".repeat(64)
}
fn operations() -> Vec<RemoteOperation> {
    vec![
        RemoteOperation::SessionLifecycle {
            vault_id: VAULT.into(),
            session_id: "note".into(),
        },
        RemoteOperation::DeleteSession {
            vault_id: VAULT.into(),
            input: DeleteSessionRequest {
                session_id: "note".into(),
                expected_revision: revision(),
            },
        },
        RemoteOperation::RestoreSession {
            vault_id: VAULT.into(),
            input: RestoreSessionRequest {
                trash_token: token(),
                expected_revision: revision(),
            },
        },
    ]
}
async fn read(Extension(state): Extension<Arc<Lifecycle>>, headers: HeaderMap) -> Json<Value> {
    assert!(headers.contains_key("authorization"));
    let mut value = json!({"session_id":"note","revision":revision()});
    match state.mode.load(Ordering::SeqCst) {
        1 => value["session_id"] = json!("other"),
        2 => value["revision"] = json!("bad"),
        _ => {}
    }
    Json(value)
}
async fn delete(
    Extension(state): Extension<Arc<Lifecycle>>,
    headers: HeaderMap,
    Json(input): Json<DeleteSessionRequest>,
) -> axum::response::Response {
    assert!(headers.contains_key("authorization"));
    assert!(uuid::Uuid::parse_str(headers["x-request-id"].to_str().unwrap()).is_ok());
    assert_eq!(input.expected_revision, revision());
    let mut value = json!({"kind":"trashed","trash_token":token(),"revision":revision()});
    let status = match state.mode.load(Ordering::SeqCst) {
        3 => {
            value["trash_token"] = json!(".trash/private");
            StatusCode::OK
        }
        4 => {
            value["revision"] = json!("c".repeat(64));
            StatusCode::OK
        }
        5 | 6 => {
            value = json!({"code":"conflict","message":"busy","request_id":null,"retryable":false,"current_revision":null,"mutation_outcome":if state.mode.load(Ordering::SeqCst)==5 {"not_committed"} else {"unknown"}});
            StatusCode::CONFLICT
        }
        9 => {
            value = json!({"kind":"conflict","snapshot":{"session_id":"note","revision":"c".repeat(64)}});
            StatusCode::CONFLICT
        }
        10 => {
            value["trash_token"] = json!("x".repeat(20 * 1024));
            StatusCode::OK
        }
        _ => StatusCode::OK,
    };
    (status, Json(value)).into_response()
}
async fn restore(
    Extension(state): Extension<Arc<Lifecycle>>,
    Json(_input): Json<RestoreSessionRequest>,
) -> Json<Value> {
    let mut value = json!({"kind":"restored","session_id":"note","revision":revision()});
    match state.mode.load(Ordering::SeqCst) {
        7 => value["session_id"] = json!("../private"),
        8 => value["revision"] = json!("c".repeat(64)),
        _ => {}
    }
    Json(value)
}
async fn execute(client: &Arc<RemoteClient>, operation: RemoteOperation) -> Result<RemoteReply> {
    client
        .execute(
            operation,
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
}
#[tokio::test]
async fn lifecycle_sdk_capability_validation_and_cancellation_fail_before_credentials() {
    let state = Arc::new(Lifecycle {
        mode: AtomicUsize::new(0),
    });
    let fixture = Fixture::with_extra_routes(None, state.routes()).await;
    let mut client = fixture.client().await;
    let before = fixture.credentials.loads.load(Ordering::SeqCst);
    for operation in operations() {
        let write = matches!(
            operation,
            RemoteOperation::DeleteSession { .. } | RemoteOperation::RestoreSession { .. }
        );
        let error = execute(&client, operation).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::IncompatibleVersion);
        assert_eq!(
            error.mutation_outcome,
            write.then_some(MutationOutcome::NotCommitted)
        );
    }
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("session_lifecycle".into());
    let invalid = [
        RemoteOperation::SessionLifecycle {
            vault_id: VAULT.into(),
            session_id: "../other".into(),
        },
        RemoteOperation::DeleteSession {
            vault_id: VAULT.into(),
            input: DeleteSessionRequest {
                session_id: "note".into(),
                expected_revision: "bad".into(),
            },
        },
        RemoteOperation::RestoreSession {
            vault_id: VAULT.into(),
            input: RestoreSessionRequest {
                trash_token: "/private".into(),
                expected_revision: revision(),
            },
        },
    ];
    for operation in invalid {
        assert_eq!(
            execute(&client, operation).await.unwrap_err().code,
            ErrorCode::Validation
        );
    }
    for operation in operations() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = client
            .execute(operation.clone(), &uuid::Uuid::new_v4().to_string(), cancel)
            .await
            .unwrap_err();
        assert_eq!(
            error.mutation_outcome,
            matches!(
                operation,
                RemoteOperation::DeleteSession { .. } | RemoteOperation::RestoreSession { .. }
            )
            .then_some(MutationOutcome::NotCommitted)
        );
    }
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), before);
}
#[tokio::test]
async fn lifecycle_sdk_validates_owners_tokens_revisions_budgets_and_typed_busy_over_tls() {
    let state = Arc::new(Lifecycle {
        mode: AtomicUsize::new(0),
    });
    let fixture =
        Fixture::with_extra_routes(Some(Duration::from_millis(150)), state.routes()).await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("session_lifecycle".into());
    fixture.login(&client).await;
    for operation in operations() {
        execute(&client, operation).await.unwrap();
    }
    for (mode, index) in [(1, 0), (2, 0), (3, 1), (4, 1), (7, 2), (8, 2), (10, 1)] {
        state.mode.store(mode, Ordering::SeqCst);
        let error = execute(&client, operations().remove(index))
            .await
            .unwrap_err();
        assert!(matches!(
            error.code,
            ErrorCode::IncompatibleVersion | ErrorCode::ResourceLimit
        ));
        if index > 0 {
            assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
        }
    }
    for (mode, outcome) in [
        (5, MutationOutcome::NotCommitted),
        (6, MutationOutcome::Unknown),
    ] {
        state.mode.store(mode, Ordering::SeqCst);
        let error = execute(&client, operations().remove(1)).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict);
        assert_eq!(error.mutation_outcome, Some(outcome));
    }
    state.mode.store(9, Ordering::SeqCst);
    assert!(matches!(
        execute(&client, operations().remove(1)).await.unwrap(),
        RemoteReply::SessionDelete(DeleteSessionOutcome::Conflict { .. })
    ));
}

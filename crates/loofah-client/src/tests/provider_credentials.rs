use super::*;
use axum::Extension;

const VAULT: &str = "f0c96de7-7264-4414-8934-c428b87b1b71";
const REQUEST: &str = "f0c96de7-7264-4414-8934-c428b87b1b72";
const INITIAL: &str = "f0c96de7-7264-4414-8934-c428b87b1b73";
const SAVED: &str = "f0c96de7-7264-4414-8934-c428b87b1b74";

struct Credentials {
    mode: AtomicUsize,
    calls: AtomicUsize,
    writes: AtomicUsize,
    delay: AtomicUsize,
    present: AtomicBool,
    admitted: tokio::sync::Notify,
}
impl Credentials {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            mode: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
            delay: AtomicUsize::new(0),
            present: AtomicBool::new(false),
            admitted: tokio::sync::Notify::new(),
        })
    }
    fn routes(self: &Arc<Self>) -> Router<Arc<Provider>> {
        Router::new()
            .route(
                &format!("/api/v1/vaults/{VAULT}/processing/providers/openai/credential"),
                get(read).put(save),
            )
            .layer(Extension(self.clone()))
    }
    fn status(&self) -> Value {
        let present = self.present.load(Ordering::SeqCst);
        let mut value = json!({"provider_id":"openai","revision":if present {SAVED} else {INITIAL},"source":if present {"vault"} else {"missing"},"endpoint_matches":true});
        match self.mode.load(Ordering::SeqCst) {
            1 => value["provider_id"] = json!("anthropic"),
            2 => value["revision"] = json!("not-a-revision"),
            3 => {
                value["source"] = json!("vault");
                value["endpoint_matches"] = json!(false);
            }
            4 => value["api_key"] = json!("RESPONSE-CANARY"),
            5 => value["revision"] = json!("x".repeat(32 * 1024)),
            7 => value["revision"] = json!(INITIAL),
            8 => value["source"] = json!("operator"),
            _ => {}
        }
        value
    }
}
fn check(headers: &HeaderMap) {
    assert!(headers.contains_key("authorization"));
    assert_eq!(headers["x-request-id"], REQUEST);
    assert_eq!(headers["x-loofah-api-major"], "1");
}
async fn read(
    Extension(state): Extension<Arc<Credentials>>,
    headers: HeaderMap,
) -> axum::response::Response {
    check(&headers);
    state.calls.fetch_add(1, Ordering::SeqCst);
    Json(state.status()).into_response()
}
async fn save(
    Extension(state): Extension<Arc<Credentials>>,
    headers: HeaderMap,
    Json(input): Json<SaveProviderCredentialRequest>,
) -> axum::response::Response {
    check(&headers);
    state.calls.fetch_add(1, Ordering::SeqCst);
    state.writes.fetch_add(1, Ordering::SeqCst);
    if state.mode.load(Ordering::SeqCst) == 6 {
        let mut error = failure(ErrorCode::Forbidden, "INPUT-CANARY was denied");
        error.request_id = Some("INPUT-CANARY".into());
        error.current_revision = Some("INPUT-CANARY".into());
        return (StatusCode::FORBIDDEN, Json(error)).into_response();
    }
    let stale = state.present.load(Ordering::SeqCst) && input.expected_revision == INITIAL;
    state
        .present
        .store(input.api_key.is_some(), Ordering::SeqCst);
    state.admitted.notify_one();
    tokio::time::sleep(Duration::from_millis(
        state.delay.load(Ordering::SeqCst) as u64
    ))
    .await;
    Json(json!({"kind":if stale {"conflict"} else {"saved"},"status":state.status()}))
        .into_response()
}
fn enable(client: &mut Arc<RemoteClient>) {
    Arc::get_mut(client)
        .unwrap()
        .capabilities
        .features
        .push("provider_credentials".into());
}
fn input() -> SaveProviderCredentialRequest {
    SaveProviderCredentialRequest {
        expected_revision: INITIAL.into(),
        api_key: Some("INPUT-CANARY".into()),
    }
}
fn write() -> RemoteOperation {
    RemoteOperation::SaveProviderCredential {
        vault_id: VAULT.into(),
        provider_id: "openai".into(),
        input: input(),
    }
}
fn read_operation() -> RemoteOperation {
    RemoteOperation::ProviderCredentialStatus {
        vault_id: VAULT.into(),
        provider_id: "openai".into(),
    }
}

#[tokio::test]
async fn credential_sdk_rejects_capability_identity_secret_budgets_and_cancellation_before_credentials()
 {
    let state = Credentials::new();
    let fixture = Fixture::with_extra_routes(None, state.routes()).await;
    let mut client = fixture.client().await;
    let loads = fixture.credentials.loads.load(Ordering::SeqCst);
    for (operation, writing) in [(read_operation(), false), (write(), true)] {
        let error = client
            .execute(operation, REQUEST, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::IncompatibleVersion);
        assert_eq!(
            error.mutation_outcome,
            writing.then_some(MutationOutcome::NotCommitted)
        );
    }
    enable(&mut client);
    for mode in 0..8 {
        let RemoteOperation::SaveProviderCredential {
            mut vault_id,
            mut provider_id,
            mut input,
        } = write()
        else {
            unreachable!()
        };
        match mode {
            0 => vault_id = "../vault".into(),
            1 => vault_id = uuid::Uuid::nil().to_string(),
            2 => provider_id = "unknown".into(),
            3 => input.expected_revision = "hash".into(),
            4 => input.api_key = Some("".into()),
            5 => input.api_key = Some("HEADER\r\nCANARY".into()),
            6 => input.api_key = Some("x".repeat(16 * 1024 + 1)),
            7 => input.api_key = Some(" spaced ".into()),
            _ => unreachable!(),
        }
        let error = client
            .execute(
                RemoteOperation::SaveProviderCredential {
                    vault_id,
                    provider_id,
                    input,
                },
                REQUEST,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
        assert!(!error.message.contains("CANARY"));
    }
    for request in ["invalid", "00000000-0000-0000-0000-000000000000"] {
        let error = client
            .execute(write(), request, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
    }
    let stop = CancellationToken::new();
    stop.cancel();
    let error = client.execute(write(), REQUEST, stop).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert!(!format!("{:?}", write()).contains("INPUT-CANARY"));
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), loads);
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn credential_sdk_validates_bounded_https_status_and_save_replies_without_secret_readback() {
    let state = Credentials::new();
    let fixture = Fixture::with_extra_routes(Some(Duration::from_millis(50)), state.routes()).await;
    let mut client = fixture.client().await;
    enable(&mut client);
    fixture.login(&client).await;
    let status = client
        .provider_credential_status(VAULT, "openai", REQUEST, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(status.source, ProviderCredentialSource::Missing);
    let saved = client
        .save_provider_credential(VAULT, "openai", input(), REQUEST, CancellationToken::new())
        .await
        .unwrap();
    assert!(matches!(saved, ProviderCredentialSaveOutcome::Saved(_)));
    let conflict = client
        .save_provider_credential(VAULT, "openai", input(), REQUEST, CancellationToken::new())
        .await
        .unwrap();
    assert!(matches!(
        conflict,
        ProviderCredentialSaveOutcome::Conflict(_)
    ));
    assert!(!serde_json::to_string(&saved).unwrap().contains("CANARY"));
    for mode in 1..=5 {
        state.mode.store(mode, Ordering::SeqCst);
        for operation in [read_operation(), write()] {
            let writing = matches!(operation, RemoteOperation::SaveProviderCredential { .. });
            let error = client
                .execute(operation, REQUEST, CancellationToken::new())
                .await
                .unwrap_err();
            assert!(matches!(
                error.code,
                ErrorCode::IncompatibleVersion | ErrorCode::ResourceLimit
            ));
            assert_eq!(
                error.mutation_outcome,
                writing.then_some(MutationOutcome::Unknown)
            );
            assert!(!error.message.contains("CANARY"));
        }
    }
    for mode in [7, 8] {
        state.present.store(false, Ordering::SeqCst);
        state.mode.store(mode, Ordering::SeqCst);
        let error = client
            .execute(write(), REQUEST, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
    }
    state.mode.store(6, Ordering::SeqCst);
    let error = client
        .execute(write(), REQUEST, CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(error.request_id.as_deref(), Some(REQUEST));
    assert!(error.current_revision.is_none());
    assert!(!format!("{error:?}").contains("CANARY"));
}

#[tokio::test]
async fn credential_sdk_lost_ack_does_not_repeat_secret_write_and_recovers_status_over_real_tls_latency()
 {
    for millis in [50, 150, 500] {
        let state = Credentials::new();
        let fixture =
            Fixture::with_extra_routes(Some(Duration::from_millis(millis)), state.routes()).await;
        let mut client = fixture.client().await;
        enable(&mut client);
        fixture.login(&client).await;
        state.delay.store(1_000, Ordering::SeqCst);
        let cancel = CancellationToken::new();
        let waiter = tokio::spawn({
            let client = client.clone();
            let cancel = cancel.clone();
            async move {
                client
                    .save_provider_credential(VAULT, "openai", input(), REQUEST, cancel)
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(10), state.admitted.notified())
            .await
            .unwrap();
        cancel.cancel();
        let error = waiter.await.unwrap().unwrap_err();
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
        assert_eq!(state.writes.load(Ordering::SeqCst), 1);
        let status = client
            .provider_credential_status(VAULT, "openai", REQUEST, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(status.revision, SAVED);
        assert_eq!(status.source, ProviderCredentialSource::Vault);
        assert_eq!(state.writes.load(Ordering::SeqCst), 1);
    }
}

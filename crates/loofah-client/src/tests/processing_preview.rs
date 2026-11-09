use super::*;
use axum::Extension;

const VAULT: &str = "3d7e2982-1294-46b8-b1c2-b08ecf532e54";
const JOB: &str = "d1e80884-f71a-4fb3-9b3d-0bf25194605d";
const REQUEST: &str = "a771e021-5dfc-4c71-8e5f-fdd4294e97bf";

fn snapshot() -> ProcessingJobPreview {
    ProcessingJobPreview {
        vault_id: VAULT.into(),
        job: ProcessingJobSnapshot {
            id: JOB.into(),
            session_id: "note".into(),
            target: DocumentTarget::Summary,
            state: ProcessingJobState::Running,
            phase: Some(ProcessingPhase::Generating),
            failure: None,
            created_at: "2026-11-01T00:00:00Z".into(),
            updated_at: "2026-11-01T00:00:01Z".into(),
        },
        preview: Some(ProcessingPreviewSnapshot {
            cursor: format!("{REQUEST}:1"),
            markdown: "PRIVATE-PREVIEW-CANARY".into(),
            truncated: false,
        }),
    }
}
fn operation() -> RemoteOperation {
    RemoteOperation::ProcessingJobPreview {
        vault_id: VAULT.into(),
        job_id: JOB.into(),
    }
}
fn enable(client: &mut Arc<RemoteClient>) {
    Arc::get_mut(client)
        .unwrap()
        .capabilities
        .features
        .push("processing_previews".into());
}
async fn read(
    Extension(value): Extension<Arc<Mutex<ProcessingJobPreview>>>,
    headers: HeaderMap,
) -> Json<ProcessingJobPreview> {
    assert!(headers.contains_key("authorization"));
    Json(value.lock().await.clone())
}

#[tokio::test]
async fn preview_preflight_rejects_capability_invalid_identity_and_cancel_before_credentials() {
    let fixture = Fixture::new().await;
    let mut client = fixture.client().await;
    let loads = fixture.credentials.loads.load(Ordering::SeqCst);
    assert_eq!(
        client
            .execute(operation(), REQUEST, CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::IncompatibleVersion
    );
    enable(&mut client);
    for (vault_id, job_id) in [
        ("../vault", JOB),
        (VAULT, "../job"),
        (VAULT, "00000000-0000-0000-0000-000000000000"),
    ] {
        assert_eq!(
            client
                .execute(
                    RemoteOperation::ProcessingJobPreview {
                        vault_id: vault_id.into(),
                        job_id: job_id.into()
                    },
                    REQUEST,
                    CancellationToken::new()
                )
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        client
            .execute(operation(), REQUEST, cancelled)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), loads);
}

#[tokio::test]
async fn preview_tls_response_binds_original_scope_and_bounds_redacted_partial_text() {
    let value = Arc::new(Mutex::new(snapshot()));
    let routes = Router::new()
        .route(
            &format!("/api/v1/vaults/{VAULT}/jobs/{JOB}/preview"),
            get(read),
        )
        .layer(Extension(value.clone()));
    let fixture = Fixture::with_extra_routes(Some(Duration::from_millis(1)), routes).await;
    let mut client = fixture.client().await;
    enable(&mut client);
    fixture.login(&client).await;
    for absent in [false, true] {
        *value.lock().await = snapshot();
        if absent {
            value.lock().await.preview = None;
        }
        let RemoteReply::ProcessingJobPreview(reply) = client
            .execute(operation(), REQUEST, CancellationToken::new())
            .await
            .unwrap()
        else {
            panic!("wrong reply")
        };
        assert_eq!(reply, *value.lock().await);
        assert!(!format!("{reply:?}").contains("PRIVATE-PREVIEW-CANARY"));
    }
    for mode in 0..6 {
        let mut forged = snapshot();
        match mode {
            0 => forged.vault_id = REQUEST.into(),
            1 => forged.job.id = REQUEST.into(),
            2 => forged.job.session_id = "../note".into(),
            3 => forged.preview.as_mut().unwrap().cursor = "invalid".into(),
            4 => {
                forged.preview.as_mut().unwrap().markdown =
                    "x".repeat(MAX_PROCESSING_PREVIEW_BYTES + 1)
            }
            5 => {
                forged.job.target = DocumentTarget::TemplateOutput {
                    document_id: "../output".into(),
                }
            }
            _ => unreachable!(),
        }
        *value.lock().await = forged;
        let error = client
            .execute(operation(), REQUEST, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::IncompatibleVersion);
        assert!(!format!("{error:?}").contains("PRIVATE-PREVIEW-CANARY"));
    }
}

#[tokio::test]
async fn preview_tls_events_are_negotiated_without_advancing_resume_cursor() {
    async fn stream(headers: HeaderMap) -> axum::response::Response {
        assert_eq!(headers["x-loofah-job-previews"], "1");
        assert_eq!(headers["last-event-id"], "original:1");
        let event = ChangeEvent {
            vault_id: VAULT.into(),
            cursor: "different:999".into(),
            change: Change::JobPreviewChanged { job_id: JOB.into() },
        };
        Sse::new(futures_util::stream::iter([Ok::<
            _,
            std::convert::Infallible,
        >(
            Event::default()
                .event("processing_preview")
                .data(serde_json::to_string(&event).unwrap()),
        )]))
        .into_response()
    }
    let fixture = Fixture::with_extra_routes(
        Some(Duration::from_millis(1)),
        Router::new().route("/preview-events", get(stream)),
    )
    .await;
    let mut client = fixture.client().await;
    enable(&mut client);
    fixture.login(&client).await;
    let mut cursor = Some("original:1".into());
    let mut failures = 0;
    let seen = AtomicUsize::new(0);
    client
        .event_connection(
            Url::parse(&format!("{}/preview-events", fixture.state.origin)).unwrap(),
            VAULT,
            &mut cursor,
            &|event| {
                if matches!(event, RemoteEvent::Change { .. }) {
                    seen.fetch_add(1, Ordering::SeqCst);
                }
                true
            },
            &mut failures,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    assert_eq!(cursor.as_deref(), Some("original:1"));
}

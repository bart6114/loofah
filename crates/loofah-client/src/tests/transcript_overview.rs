use super::*;

pub(super) async fn overview(State(state): State<Arc<Provider>>) -> Json<Value> {
    let labels = match state.overview_mode.load(Ordering::SeqCst) {
        1 => vec!["duplicate".to_string(); 2],
        2 => vec![String::new()],
        3 => (0..=TRANSCRIPT_OVERVIEW_LABEL_LIMIT)
            .map(|index| index.to_string())
            .collect(),
        4 => vec!["\n".repeat(TRANSCRIPT_OVERVIEW_RESPONSE_LIMIT / 2)],
        _ => vec!["second".into(), "first".into()],
    };
    Json(json!({"has_words":true,"speaker_labels":labels,"started_at":10.0,"ended_at":1.0}))
}
fn operation() -> RemoteOperation {
    RemoteOperation::TranscriptOverview {
        vault_id: "test".into(),
        session_id: "note".into(),
    }
}
#[tokio::test]
async fn capability_and_cancel_admission_happen_before_credentials() {
    let fixture = Fixture::new().await;
    let mut client = fixture.client().await;
    assert_eq!(
        client
            .execute(
                operation(),
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::IncompatibleVersion
    );
    assert_eq!(fixture.state.issued.load(Ordering::SeqCst), 0);
    assert!(fixture.credentials.values.lock().await.is_empty());
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("transcript_overview".into());
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        client
            .execute(operation(), &uuid::Uuid::new_v4().to_string(), cancel)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(
        client
            .execute(
                RemoteOperation::TranscriptOverview {
                    vault_id: "test".into(),
                    session_id: "../private".into()
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert!(fixture.credentials.values.lock().await.is_empty());
}
#[tokio::test]
async fn sdk_roundtrip_preserves_order_and_explicit_times_and_rejects_invalid_or_oversize_replies()
{
    let fixture = Fixture::new().await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("transcript_overview".into());
    fixture.login(&client).await;
    let reply = client
        .execute(
            operation(),
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let RemoteReply::TranscriptOverview(overview) = reply else {
        panic!("Expected transcript overview");
    };
    assert_eq!(
        overview,
        TranscriptOverview {
            has_words: true,
            speaker_labels: vec!["second".into(), "first".into()],
            started_at: Some(10.0),
            ended_at: Some(1.0)
        }
    );
    for mode in 1..=4 {
        fixture.state.overview_mode.store(mode, Ordering::SeqCst);
        assert!(
            client
                .execute(
                    operation(),
                    &uuid::Uuid::new_v4().to_string(),
                    CancellationToken::new()
                )
                .await
                .is_err(),
            "mode {mode}"
        );
    }
}

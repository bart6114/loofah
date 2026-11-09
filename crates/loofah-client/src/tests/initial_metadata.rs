use super::*;
#[tokio::test]
async fn initial_metadata_requires_capability_and_valid_fields_before_credentials() {
    let fixture = Fixture::new().await;
    let mut client = fixture.client().await;
    let before = fixture.credentials.loads.load(Ordering::SeqCst);
    let request = uuid::Uuid::new_v4().to_string();
    let operation = RemoteOperation::CreateSessionWithMetadata {
        vault_id: "vault".into(),
        input: CreateSessionWithMetadataRequest {
            title: "Title".into(),
            markdown: "Original note".into(),
            metadata: LocalInitialSessionMetadata::default(),
        },
    };
    let cause = client
        .execute(operation.clone(), &request, CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(cause.code, ErrorCode::IncompatibleVersion);
    assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("session_initial_metadata".into());
    let RemoteOperation::CreateSessionWithMetadata {
        vault_id,
        mut input,
    } = operation
    else {
        unreachable!()
    };
    input.metadata.skill = Some("requires-author".into());
    let cause = client
        .execute(
            RemoteOperation::CreateSessionWithMetadata { vault_id, input },
            &request,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(cause.code, ErrorCode::Validation);
    assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), before);
}

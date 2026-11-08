use super::*;

fn operations() -> Vec<RemoteOperation> {
    vec![
        RemoteOperation::TagSuggestions {
            vault_id: "test".into(),
            input: TagSuggestionsRequest {
                query: "".into(),
                excluded: vec![],
                limit: 8,
            },
        },
        RemoteOperation::EnsureTag {
            vault_id: "test".into(),
            input: EnsureTagRequest {
                name: "import/manual".into(),
            },
        },
        RemoteOperation::AcceptTagSuggestion {
            vault_id: "test".into(),
            session_id: "note".into(),
            input: TagSuggestionActionRequest {
                name: "other".into(),
                expected_revision: "revision".into(),
            },
        },
        RemoteOperation::DismissTagSuggestion {
            vault_id: "test".into(),
            session_id: "note".into(),
            input: TagSuggestionActionRequest {
                name: "other".into(),
                expected_revision: "revision".into(),
            },
        },
    ]
}

#[tokio::test]
async fn complete_tag_capability_is_required_before_authentication_and_write_dispatch() {
    let fixture = Fixture::new().await;
    let mut client = fixture.client().await;
    for (index, operation) in operations().into_iter().enumerate() {
        let request_id = uuid::Uuid::new_v4().to_string();
        let failure = client
            .execute(operation, &request_id, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(failure.code, ErrorCode::IncompatibleVersion);
        assert_eq!(failure.request_id.as_deref(), Some(request_id.as_str()));
        assert_eq!(
            failure.mutation_outcome,
            if index == 0 {
                None
            } else {
                Some(MutationOutcome::NotCommitted)
            }
        );
    }
    assert_eq!(fixture.state.issued.load(Ordering::SeqCst), 0);
    assert!(fixture.credentials.values.lock().await.is_empty());
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("tag_editing".into());
    for operation in operations().into_iter().skip(1) {
        let failure = client
            .execute(
                operation,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.code, ErrorCode::Unauthenticated);
        assert_eq!(
            failure.mutation_outcome,
            Some(MutationOutcome::NotCommitted)
        );
    }
}

#[tokio::test]
async fn tag_validation_and_predispatch_cancellation_do_not_admit_writes() {
    let fixture = Fixture::new().await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("tag_editing".into());
    for operation in operations().into_iter().skip(1) {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let failure = client
            .execute(operation.clone(), &uuid::Uuid::new_v4().to_string(), cancel)
            .await
            .unwrap_err();
        assert_eq!(failure.code, ErrorCode::Cancelled);
        assert_eq!(
            failure.mutation_outcome,
            Some(MutationOutcome::NotCommitted)
        );
        let failure = client
            .execute(operation, "not-a-uuid", CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(failure.code, ErrorCode::Validation);
        assert_eq!(
            failure.mutation_outcome,
            Some(MutationOutcome::NotCommitted)
        );
    }
    for operation in [
        RemoteOperation::EnsureTag {
            vault_id: "test".into(),
            input: EnsureTagRequest { name: " ".into() },
        },
        RemoteOperation::AcceptTagSuggestion {
            vault_id: "test".into(),
            session_id: "../escape".into(),
            input: TagSuggestionActionRequest {
                name: "other".into(),
                expected_revision: "revision".into(),
            },
        },
        RemoteOperation::DismissTagSuggestion {
            vault_id: "test".into(),
            session_id: "note".into(),
            input: TagSuggestionActionRequest {
                name: "other".into(),
                expected_revision: "".into(),
            },
        },
    ] {
        let failure = client
            .execute(
                operation,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.code, ErrorCode::Validation);
        assert_eq!(
            failure.mutation_outcome,
            Some(MutationOutcome::NotCommitted)
        );
    }
    assert_eq!(fixture.state.issued.load(Ordering::SeqCst), 0);
}

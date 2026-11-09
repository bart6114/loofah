use super::*;

#[tokio::test]
async fn checked_speaker_capability_and_invalid_input_precede_credentials() {
    let fixture = Fixture::new().await;
    let mut client = fixture.client().await;
    let operations = vec![
        RemoteOperation::TranscriptEditor {
            vault_id: "vault".into(),
            session_id: "note".into(),
        },
        RemoteOperation::AssignTranscriptSpeakers {
            vault_id: "vault".into(),
            session_id: "note".into(),
            input: AssignTranscriptSpeakersRequest {
                expected_revision: "a".repeat(64),
                assignments: vec![TranscriptSpeakerAssignment {
                    transcript_id: "t".into(),
                    channel: 0,
                    speaker_index: None,
                    speaker_label: "alice".into(),
                    anchor_word_id: "w".into(),
                }],
            },
        },
        RemoteOperation::EnsurePerson {
            vault_id: "vault".into(),
            input: EnsurePersonRequest {
                name: "Alice".into(),
            },
        },
    ];
    for operation in &operations {
        let error = client
            .execute(
                operation.clone(),
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::IncompatibleVersion);
        assert_eq!(
            error.mutation_outcome,
            (!matches!(operation, RemoteOperation::TranscriptEditor { .. }))
                .then_some(MutationOutcome::NotCommitted)
        );
    }
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("transcript_editor".into());
    for operation in operations {
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert_eq!(
            client
                .execute(
                    operation.clone(),
                    &uuid::Uuid::new_v4().to_string(),
                    cancelled
                )
                .await
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
        assert_eq!(
            client
                .execute(operation, "invalid", CancellationToken::new())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }
    let error = client
        .execute(
            RemoteOperation::EnsurePerson {
                vault_id: "vault".into(),
                input: EnsurePersonRequest { name: "\n".into() },
            },
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Validation);
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), 0);
}

pub(super) async fn read(
    axum::extract::Path(params): axum::extract::Path<HashMap<String, String>>,
) -> Json<Value> {
    Json(
        json!({"session_id":params["session"],"session_created_at":"2026-11-08T10:00:00Z","revision":"a".repeat(64),"transcripts":[]}),
    )
}
pub(super) async fn assign(
    axum::extract::Path(params): axum::extract::Path<HashMap<String, String>>,
    headers: HeaderMap,
    Json(input): Json<AssignTranscriptSpeakersRequest>,
) -> axum::response::Response {
    assert!(
        headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Bearer ")
    );
    assert!(uuid::Uuid::parse_str(headers["x-request-id"].to_str().unwrap()).is_ok());
    assert_eq!(input.assignments.len(), 2);
    let conflict = input.expected_revision == "b".repeat(64);
    (if conflict {StatusCode::CONFLICT}else{StatusCode::OK},Json(json!({"kind":if conflict {"conflict"}else{"saved"},"snapshot":{"session_id":params["session"],"session_created_at":"2026-11-08T10:00:00Z","revision":"b".repeat(64),"transcripts":[]}}))).into_response()
}
pub(super) async fn ensure(Json(input): Json<EnsurePersonRequest>) -> Json<Value> {
    Json(json!({"id":"alice","name":input.name.trim()}))
}

#[tokio::test]
async fn checked_speaker_tls_reads_atomic_batches_conflicts_and_person_names() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("transcript_editor".into());
    fixture.login(&client).await;
    let reply = client
        .execute(
            RemoteOperation::TranscriptEditor {
                vault_id: "vault".into(),
                session_id: "note".into(),
            },
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(reply,RemoteReply::TranscriptEditor(value) if value.session_id=="note"));
    for (revision, conflict) in [("a", false), ("b", true)] {
        let input = AssignTranscriptSpeakersRequest {
            expected_revision: revision.repeat(64),
            assignments: (0..2)
                .map(|index| TranscriptSpeakerAssignment {
                    transcript_id: "t".into(),
                    channel: 1,
                    speaker_index: Some(index),
                    speaker_label: "alice".into(),
                    anchor_word_id: format!("word-{index}"),
                })
                .collect(),
        };
        let reply = client
            .execute(
                RemoteOperation::AssignTranscriptSpeakers {
                    vault_id: "vault".into(),
                    session_id: "note".into(),
                    input,
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let RemoteReply::TranscriptEditorSave(value) = reply else {
            panic!("unexpected reply")
        };
        assert_eq!(
            matches!(value, TranscriptEditorSaveOutcome::Conflict(_)),
            conflict
        );
    }
    let reply = client
        .execute(
            RemoteOperation::EnsurePerson {
                vault_id: "vault".into(),
                input: EnsurePersonRequest {
                    name: " Alice ".into(),
                },
            },
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(reply,RemoteReply::Person(person) if person.name=="Alice"));
}

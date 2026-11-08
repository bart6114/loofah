use super::*;

async fn note(context: &Arc<hypr_loofah_core::VaultContext>, labels: &[String]) {
    let meta = serde_json::from_value(
        json!({"id":"same","title":"Title","created_at":"2026-11-08T00:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "PRIVATE NOTE BODY")
        .await
        .unwrap();
    let hints: Vec<_> = labels
        .iter()
        .map(|label| json!({"word_id":"w","type":"speaker_label","value":label}))
        .collect();
    let value = json!({"transcripts":[{"id":"t","started_at":10,"ended_at":50,"words":[{"id":"w","text":"PRIVATE WORDS","start_ms":0,"end_ms":100,"channel":0}],"speaker_hints":hints}]});
    let path = context
        .store
        .vault_base()
        .join("sessions/same/transcript.json");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    context.store.refresh_session("same").await.unwrap();
}
#[tokio::test]
async fn overview_is_reader_accessible_scoped_metadata_only_and_missing_sessions_are_not_found() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Transcript metadata"})),
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
    note(&context, &["Bob".into(), "Alice".into(), "Bob".into()]).await;
    note(
        &fixture
            .service
            .vault(&alice.personal_vault_id)
            .await
            .unwrap(),
        &["PRIVATE OTHER VAULT".into()],
    )
    .await;
    let path = format!("/api/v1/vaults/{vault}/sessions/same/transcript-overview");
    let (status, result) = fixture.call("bob", "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        result,
        json!({"has_words":true,"speaker_labels":["Bob","Alice"],"started_at":10.0,"ended_at":50.0})
    );
    assert!(!result.to_string().contains("PRIVATE"));
    assert_eq!(
        fixture.call("charlie", "GET", &path, None).await.0,
        StatusCode::NOT_FOUND
    );
    let private = path.replacen(vault, &alice.personal_vault_id, 1);
    assert_eq!(
        fixture.call("bob", "GET", &private, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .call(
                "bob",
                "GET",
                &path.replace("sessions/same", "sessions/missing"),
                None
            )
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let meta = serde_json::from_value(
        json!({"id":"empty","title":"","created_at":"2026-11-08T00:00:00Z","tags":[]}),
    )
    .unwrap();
    context.store.create_note_session(&meta, "").await.unwrap();
    assert_eq!(
        fixture
            .call(
                "bob",
                "GET",
                &path.replace("sessions/same", "sessions/empty"),
                None
            )
            .await
            .1,
        json!({"has_words":false,"speaker_labels":[],"started_at":null,"ended_at":null})
    );
    let (_, caps) = fixture
        .call("bob", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        caps["features"]
            .as_array()
            .unwrap()
            .contains(&json!("transcript_overview"))
    );
    fixture.service.shutdown().await.unwrap();
}
#[tokio::test]
async fn overview_fails_count_raw_and_encoded_budgets_without_truncation_or_local_capacity_limits()
{
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    for labels in [
        (0..=TRANSCRIPT_OVERVIEW_LABEL_LIMIT)
            .map(|i| i.to_string())
            .collect::<Vec<_>>(),
        vec!["é".repeat(TRANSCRIPT_OVERVIEW_RESPONSE_LIMIT / 2 + 1)],
        vec!["\n".repeat(TRANSCRIPT_OVERVIEW_RESPONSE_LIMIT / 2)],
    ] {
        if context.store.session_ids().is_empty() {
            note(&context, &labels).await;
        } else {
            let value = json!({"transcripts":[{"id":"t","speaker_hints":labels.iter().map(|label| json!({"word_id":"w","type":"speaker_label","value":label})).collect::<Vec<_>>()}]});
            std::fs::write(
                context
                    .store
                    .vault_base()
                    .join("sessions/same/transcript.json"),
                serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
            context.store.refresh_session("same").await.unwrap();
        }
        assert_eq!(
            context
                .transcript_overview("same", None)
                .await
                .unwrap()
                .speaker_labels,
            labels
        );
        let (status, error) = fixture
            .call(
                "alice",
                "GET",
                &format!(
                    "/api/v1/vaults/{}/sessions/same/transcript-overview",
                    me.personal_vault_id
                ),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(error["code"], "resource_limit");
    }
    fixture.service.shutdown().await.unwrap();
}

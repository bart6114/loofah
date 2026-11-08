use super::*;

#[tokio::test]
async fn speaker_edits_are_atomic_revision_checked_reader_protected_and_keep_unknown_fields() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Speakers"})),
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
    let base = format!("/api/v1/vaults/{vault}/sessions");
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            &base,
            Some(json!({"title":"Speakers","markdown":"Notes"})),
        )
        .await;
    let session = created["meta"]["id"].as_str().unwrap();
    let context = fixture.service.vault(vault).await.unwrap();
    let path = context
        .store
        .vault_base()
        .join("sessions")
        .join(session)
        .join("transcript.json");
    std::fs::write(&path,serde_json::to_vec(&json!({"future":42,"transcripts":[{"id":"t","session_id":session,"future_entry":true,"words":[{"id":"w","text":"Hello","channel":0,"start_ms":0,"end_ms":1,"future_word":9}],"speaker_hints":[]}]})).unwrap()).unwrap();
    let read = format!("{base}/{session}/transcript/editor");
    let write = format!("{base}/{session}/transcript/speakers");
    let (status, original) = fixture.call("bob", "GET", &read, None).await;
    assert_eq!(status, StatusCode::OK);
    let input = json!({"expected_revision":original["revision"],"assignments":[{"transcript_id":"t","channel":0,"speaker_index":null,"speaker_label":"alice","anchor_word_id":"w"}]});
    assert_eq!(
        fixture
            .call("bob", "POST", &write, Some(input.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture.call("charlie", "GET", &read, None).await.0,
        StatusCode::NOT_FOUND
    );
    let private = format!(
        "/api/v1/vaults/{}/sessions/{session}/transcript/editor",
        alice.personal_vault_id
    );
    assert_eq!(
        fixture.call("bob", "GET", &private, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, saved) = fixture
        .call("alice", "POST", &write, Some(input.clone()))
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["kind"], "saved");
    let (status, conflict) = fixture.call("alice", "POST", &write, Some(input)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["snapshot"], saved["snapshot"]);
    let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(raw["future"], 42);
    assert_eq!(raw["transcripts"][0]["future_entry"], true);
    assert_eq!(raw["transcripts"][0]["words"][0]["future_word"], 9);
    let people = format!("/api/v1/vaults/{vault}/people/ensure");
    assert_eq!(
        fixture
            .call("bob", "POST", &people, Some(json!({"name":"Alice"})))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, person) = fixture
        .call("alice", "POST", &people, Some(json!({"name":"Alice"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(person["name"], "Alice");
    let registry = context.store.vault_base().join("people.json");
    std::fs::write(&registry, "broken").unwrap();
    assert_ne!(
        fixture
            .call("alice", "POST", &people, Some(json!({"name":"Bob"})))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(std::fs::read_to_string(registry).unwrap(), "broken");
}

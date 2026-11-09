use super::*;

#[tokio::test]
async fn summary_queries_are_reader_accessible_bounded_and_vault_isolated() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Headers"})),
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
    for (id, title) in [
        (vault, "Shared plan"),
        (alice.personal_vault_id.as_str(), "PRIVATE PLAN"),
    ] {
        let context = fixture.service.vault(id).await.unwrap();
        let meta=serde_json::from_value(json!({"id":"same","title":title,"created_at":"2026-11-05T10:00:00Z","tags":[],"future":"PRIVATE META"})).unwrap();
        context
            .store
            .create_note_session(&meta, "PRIVATE BODY")
            .await
            .unwrap();
    }
    let path = format!("/api/v1/vaults/{vault}/session-summaries/query");
    for input in [
        json!({"kind":"list","limit":55,"order":"newest_first"}),
        json!({"kind":"title","query":" PLAN ","limit":50}),
        json!({"kind":"ids","ids":["missing","same","same"]}),
    ] {
        let (status, rows) = fixture
            .call("bob", "POST", &path, Some(input.clone()))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["title"], "Shared plan");
        assert_eq!(rows[0].as_object().unwrap().len(), 3);
        assert!(!rows.to_string().contains("PRIVATE"));
        assert_eq!(
            fixture
                .call("charlie", "POST", &path, Some(input.clone()))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            fixture
                .call(
                    "bob",
                    "POST",
                    &format!(
                        "/api/v1/vaults/{}/session-summaries/query",
                        alice.personal_vault_id
                    ),
                    Some(input)
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    for input in [
        json!({"kind":"list","limit":201,"order":"newest_first"}),
        json!({"kind":"ids","ids":["../escape"]}),
        json!({"kind":"ids","ids":vec!["same";101]}),
        json!({"kind":"title","query":" ","limit":1}),
        json!({"kind":"ids","ids":[],"path":"/private/vault"}),
    ] {
        assert_eq!(
            fixture.call("bob", "POST", &path, Some(input)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let context = fixture.service.vault(vault).await.unwrap();
    std::fs::remove_file(context.store.vault_base().join("sessions/same/_meta.json")).unwrap();
    let (status, rows) = fixture
        .call(
            "bob",
            "POST",
            &path,
            Some(json!({"kind":"ids","ids":["same"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows, json!([]));
    fixture.service.shutdown().await.unwrap();
}

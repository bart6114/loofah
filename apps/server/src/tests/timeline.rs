use super::*;

#[tokio::test]
async fn timeline_pages_and_facets_are_reader_accessible_complete_revision_bound_and_vault_isolated()
 {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Timeline"})),
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
    for i in 0..205 {
        let meta=serde_json::from_value(json!({"id":format!("id-{i:03}"),"title":"same","created_at":"2026-01-01T00:00:00Z","tags":["a/b","a/c"],"author":" Raw Author ","future_secret":"PRIVATE"})).unwrap();
        context.store.write_meta(&meta).await.unwrap();
    }
    let private = fixture
        .service
        .vault(&alice.personal_vault_id)
        .await
        .unwrap();
    private.store.write_meta(&serde_json::from_value(json!({"id":"same","title":"PRIVATE TITLE","created_at":"2026-01-01","tags":["private/tag"]})).unwrap()).await.unwrap();
    let path = format!("/api/v1/vaults/{vault}/timeline/headers");
    let input = json!({"view":{"kind":"tag","path":"a/b"},"limit":200});
    let (status, first) = fixture
        .call("bob", "POST", &path, Some(input.clone()))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["items"].as_array().unwrap().len(), 200);
    assert_eq!(first["total_count"], 205);
    assert!(!first.to_string().contains("PRIVATE"));
    assert_eq!(first["items"][0]["header"]["author"], " Raw Author ");
    let (status,second)=fixture.call("bob","POST",&path,Some(json!({"view":{"kind":"tag","path":"a/b"},"limit":200,"cursor":first["next_cursor"],"expected_revision":first["revision"]}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["items"].as_array().unwrap().len(), 5);
    assert!(second["next_cursor"].is_null());
    let facets_path = format!("/api/v1/vaults/{vault}/timeline/facets");
    let (status, facets) = fixture
        .call(
            "bob",
            "POST",
            &facets_path,
            Some(json!({"expected_revision":first["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(facets["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(facets["nodes"][0]["subtree_count"], 205);
    assert_eq!(facets["nodes"][0]["direct_count"], 0);
    for subject in ["bob", "charlie"] {
        let foreign = if subject == "bob" {
            alice.personal_vault_id.as_str()
        } else {
            vault
        };
        assert_eq!(
            fixture
                .call(
                    subject,
                    "POST",
                    &format!("/api/v1/vaults/{foreign}/timeline/headers"),
                    Some(input.clone())
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            fixture
                .call(
                    subject,
                    "POST",
                    &format!("/api/v1/vaults/{foreign}/timeline/facets"),
                    Some(json!({}))
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    for input in [
        json!({"view":{"kind":"tag","path":"a//b"},"limit":200}),
        json!({"view":{"kind":"untagged","path":"a"},"limit":200}),
        json!({"view":{"kind":"undated"},"limit":201}),
        json!({"view":{"kind":"undated"},"limit":2,"server_path":"/private"}),
    ] {
        assert_eq!(
            fixture.call("bob", "POST", &path, Some(input)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut changed = context.store.session_get("id-000").unwrap().meta;
    changed.title = "new".into();
    context.store.write_meta(&changed).await.unwrap();
    let (status,error)=fixture.call("bob","POST",&path,Some(json!({"view":{"kind":"tag","path":"a/b"},"limit":200,"cursor":first["next_cursor"]}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["retryable"], true);
    let (status, error) = fixture
        .call(
            "bob",
            "POST",
            &facets_path,
            Some(json!({"expected_revision":first["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["retryable"], true);
    let (_, capabilities) = fixture
        .call("bob", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("timeline_read"))
    );
    fixture.service.shutdown().await.unwrap();
}

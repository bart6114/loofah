use super::*;

#[tokio::test]
async fn provisioned_and_initialized_empty_vault_can_adopt_without_rebinding_catalog_access() {
    let mut fixture = Fixture::new().await;
    let root = std::mem::replace(&mut fixture._root, tempfile::tempdir().unwrap());
    let issuer_task = std::mem::replace(&mut fixture.issuer_task, tokio::spawn(async {}));
    let owner = fixture.login("alice").await;
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Fresh adoption target"})),
        )
        .await;
    let id = created["id"].as_str().unwrap();
    let context = fixture.service.vault(id).await.unwrap();
    assert!(context.startup.is_ready());
    drop(context);
    fixture.service.shutdown().await.unwrap();
    let original_config = fixture.service.config.clone();
    drop(fixture);
    let source = root.path().join("local-source");
    let session = source.join("sessions/adopted-note");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("_meta.json"),serde_json::to_vec(&json!({"id":"adopted-note","title":"Local original","created_at":"2026-11-08T12:00:00Z","tags":[]})).unwrap()).unwrap();
    std::fs::write(session.join("notes.md"), b"Exact Local note body\n").unwrap();
    std::fs::write(session.join("unknown-user-file"), b"preserve unknown bytes").unwrap();
    let output = root.path().join("adopted-server");
    crate::maintenance::adopt_vault(&original_config.storage_root, id, &source, &output)
        .await
        .unwrap();
    let mut config = original_config;
    config.storage_root = output.clone();
    config.cache_root = root.path().join("fresh-adoption-cache");
    let reopened = Service::open(config).await.unwrap();
    let adopted = reopened.vault(id).await.unwrap();
    assert!(adopted.startup.is_ready());
    assert_eq!(
        adopted
            .store
            .read_note("adopted-note")
            .await
            .unwrap()
            .as_deref(),
        Some("Exact Local note body\n")
    );
    let access = reopened
        .catalog
        .access_member(&owner.member.id, id, true)
        .await
        .unwrap();
    assert_eq!(access.info.id, id);
    drop(access);
    drop(adopted);
    reopened.shutdown().await.unwrap();
    drop(reopened);
    issuer_task.abort();
    assert_eq!(
        std::fs::read(source.join("sessions/adopted-note/notes.md")).unwrap(),
        b"Exact Local note body\n"
    );
    assert_eq!(
        std::fs::read(
            output
                .join("vaults")
                .join(id)
                .join("sessions/adopted-note/unknown-user-file")
        )
        .unwrap(),
        b"preserve unknown bytes"
    );
}

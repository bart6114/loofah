use super::*;
use std::{collections::BTreeMap, path::Path};

fn storage_hashes(root: &Path) -> BTreeMap<std::path::PathBuf, String> {
    fn visit(root: &Path, current: &Path, hashes: &mut BTreeMap<std::path::PathBuf, String>) {
        for entry in std::fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            assert!(!kind.is_symlink());
            if kind.is_dir() {
                visit(root, &entry.path(), hashes);
            } else {
                assert!(kind.is_file());
                hashes.insert(
                    entry.path().strip_prefix(root).unwrap().to_owned(),
                    capture::hash(std::fs::read(entry.path()).unwrap()),
                );
            }
        }
    }
    let mut hashes = BTreeMap::new();
    visit(root, root, &mut hashes);
    hashes
}

fn copy_stopped_storage(source: &Path, target: &Path) {
    std::fs::create_dir(target).unwrap();
    std::fs::set_permissions(target, source.metadata().unwrap().permissions()).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink());
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            copy_stopped_storage(&entry.path(), &destination);
        } else {
            assert!(kind.is_file());
            std::fs::copy(entry.path(), destination).unwrap();
        }
    }
}

#[tokio::test]
async fn stopped_storage_restore_preserves_identity_access_receipts_capture_bytes_and_canonical_files()
 {
    let mut fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Restore"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap().to_owned();
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
    let create_id = uuid::Uuid::new_v4().to_string();
    let create_path = format!("/api/v1/vaults/{vault}/sessions");
    let create_input = json!({"title":"Before backup","markdown":"Original create body"});
    let (status, created) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &create_path,
            Some(create_input.clone()),
            Some(&create_id),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let session = created["meta"]["id"].as_str().unwrap();
    let note_path = format!("{create_path}/{session}/note");
    let (status, _) = fixture.call("alice", "PUT", &note_path,
        Some(json!({"markdown":"Canonical text after creation","title":"After creation","expected_revision":created["note"]["revision"]}))).await;
    assert_eq!(status, StatusCode::OK);
    let context = fixture.service.vault(&vault).await.unwrap();
    let directory = context.store.vault_base().join("sessions").join(session);
    std::fs::write(
        directory.join("unknown-user-attachment.pdf"),
        b"USER ATTACHMENT CANARY\0\xff",
    )
    .unwrap();
    std::fs::write(
        directory.join(".user-hidden-file"),
        b"PRIVATE HIDDEN CANARY",
    )
    .unwrap();
    let note_before = fixture.call("bob", "GET", &note_path, None).await;
    assert_eq!(note_before.0, StatusCode::OK);
    let mut input = capture::input(&fixture, &alice);
    input.identity.scope.vault_id = vault.clone();
    input.identity.target = CaptureTarget::ExistingSession {
        session_id: session.to_owned(),
        expected_audio_sha256: None,
    };
    assert_eq!(
        capture::begin(&fixture, "alice", &input).await.0,
        StatusCode::OK
    );
    let chunk = capture::descriptor(&input, 0, 160, None);
    assert_eq!(
        capture::append(&fixture, "alice", &input, &chunk, capture::pcm(160))
            .await
            .0,
        StatusCode::OK
    );
    let seal = CaptureSeal {
        capture_fingerprint: chunk.capture_fingerprint.clone(),
        chunk_count: 1,
        frames: 160,
        pcm_bytes: u64::from(chunk.bytes),
        chain_digest: chunk.digest.clone(),
        source_sha256: capture::hash(capture::pcm(160)),
    };
    let capture_path = format!(
        "/api/v1/vaults/{vault}/captures/{}",
        input.identity.capture_id
    );
    let (status, sealed) = fixture
        .call(
            "alice",
            "POST",
            &format!("{capture_path}/seal"),
            Some(serde_json::to_value(&seal).unwrap()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(sealed["publication"].is_null());
    drop(context);
    fixture.service.shutdown().await.unwrap();

    let restored = tempfile::tempdir().unwrap();
    let original_storage = fixture.service.config.storage_root.clone();
    let original_hashes = storage_hashes(&original_storage);
    let restored_storage = restored.path().join("storage");
    copy_stopped_storage(&original_storage, &restored_storage);
    assert_eq!(storage_hashes(&restored_storage), original_hashes);
    let mut config = fixture.service.config.clone();
    config.storage_root = restored_storage.clone();
    config.cache_root = restored.path().join("empty-cache");
    assert!(!config.cache_root.exists());
    let service = Service::open(config).await.unwrap();
    fixture.app = routes::router(service.clone());
    fixture.service = service;
    assert_eq!(fixture.login("alice").await, alice);
    assert_eq!(fixture.login("bob").await, bob);
    assert_eq!(
        fixture.call("bob", "GET", &note_path, None).await,
        note_before
    );
    assert_eq!(
        fixture
            .call(
                "bob",
                "PUT",
                &note_path,
                Some(
                    json!({"markdown":"Unauthorized","expected_revision":note_before.1["revision"]})
                )
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let private_path = format!("/api/v1/vaults/{}/sessions", alice.personal_vault_id);
    assert_eq!(
        fixture.call("bob", "GET", &private_path, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, replayed) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &create_path,
            Some(create_input),
            Some(&create_id),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed["meta"]["id"], session);
    assert_eq!(replayed["note"], note_before.1);
    assert_eq!(
        fixture.call("bob", "GET", &note_path, None).await,
        note_before
    );
    assert_eq!(
        fixture.call("alice", "GET", &capture_path, None).await,
        (StatusCode::OK, sealed)
    );
    let (status, published) = fixture
        .call(
            "alice",
            "POST",
            &format!("{capture_path}/publish"),
            Some(serde_json::to_value(&seal).unwrap()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(published["publication"]["session_id"], session);
    let restored_context = fixture.service.vault(&vault).await.unwrap();
    let restored_directory = restored_context
        .store
        .vault_base()
        .join("sessions")
        .join(session);
    assert_eq!(
        std::fs::read(restored_directory.join("unknown-user-attachment.pdf")).unwrap(),
        b"USER ATTACHMENT CANARY\0\xff"
    );
    assert_eq!(
        std::fs::read(restored_directory.join(".user-hidden-file")).unwrap(),
        b"PRIVATE HIDDEN CANARY"
    );
    wait_agent_projection(&restored_context).await;
    let exported = restored_context.agent_export(session).await.unwrap();
    let exported = serde_json::to_string(&exported).unwrap();
    assert!(!exported.contains("CANARY"));
    assert!(exported.contains("Canonical text after creation"));
    assert_eq!(storage_hashes(&original_storage), original_hashes);
    drop(restored_context);
    fixture.service.shutdown().await.unwrap();
}

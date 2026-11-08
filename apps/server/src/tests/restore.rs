use super::*;
use std::{collections::BTreeMap, path::Path};

fn storage_hashes(root: &Path) -> BTreeMap<std::path::PathBuf, String> {
    fn visit(root: &Path, current: &Path, hashes: &mut BTreeMap<std::path::PathBuf, String>) {
        for entry in std::fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            if entry.path() == root.join(".loofah-server.lock") {
                continue;
            }
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

struct StoppedFixture {
    root: tempfile::TempDir,
    issuer: String,
    issuer_task: tokio::task::JoinHandle<()>,
    config: Config,
}

async fn stop(mut fixture: Fixture) -> StoppedFixture {
    fixture.service.shutdown().await.unwrap();
    fixture.service.wait_for_shutdown().await.unwrap();
    let stopped = StoppedFixture {
        root: std::mem::replace(&mut fixture._root, tempfile::tempdir().unwrap()),
        issuer: std::mem::take(&mut fixture.issuer),
        issuer_task: std::mem::replace(&mut fixture.issuer_task, tokio::spawn(async {})),
        config: fixture.service.config.clone(),
    };
    drop(fixture);
    stopped
}

async fn restart(stopped: StoppedFixture, config: Config) -> Fixture {
    let service = Service::open(config).await.unwrap();
    Fixture {
        _root: stopped.root,
        issuer: stopped.issuer,
        issuer_task: stopped.issuer_task,
        app: routes::router(service.clone()),
        service,
    }
}

#[tokio::test]
async fn stopped_storage_restore_preserves_identity_access_receipts_capture_bytes_and_canonical_files()
 {
    let processing =
        serde_json::from_value(json!({"providers":[{"id":"openai","models":["approved"]}]}))
            .unwrap();
    let fixture = Fixture::with_processing(Some(processing)).await;
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
    let credential_path = format!("/api/v1/vaults/{vault}/processing/providers/openai/credential");
    let (_, credential_status) = fixture.call("alice", "GET", &credential_path, None).await;
    let (status, saved_credential) = fixture.call("alice", "PUT", &credential_path, Some(json!({
        "expected_revision":credential_status["revision"], "api_key":"RESTORE-PRIVATE-CANARY"
    }))).await;
    assert_eq!(status, StatusCode::OK);
    drop(context);
    let stopped = stop(fixture).await;

    let restored = tempfile::tempdir().unwrap();
    let original_storage = stopped.config.storage_root.clone();
    let original_hashes = storage_hashes(&original_storage);
    let snapshot = restored.path().join("snapshot");
    let secrets = restored.path().join("private-credentials");
    let report = crate::maintenance::backup(&original_storage, &snapshot, &secrets)
        .await
        .unwrap();
    assert!(report.secrets_required);
    assert!(crate::maintenance::verify(&snapshot, None).await.is_err());
    let private_path = format!("runtime/{vault}/creates/provider-credentials/credentials.json");
    assert!(!snapshot.join("data").join(&private_path).exists());
    assert!(secrets.join("data").join(&private_path).exists());
    let restored_storage = restored.path().join("storage");
    crate::maintenance::restore(&snapshot, Some(&secrets), &restored_storage)
        .await
        .unwrap();
    assert_eq!(storage_hashes(&restored_storage), original_hashes);
    let mut config = stopped.config.clone();
    config.storage_root = restored_storage.clone();
    config.cache_root = restored.path().join("empty-cache");
    assert!(!config.cache_root.exists());
    let fixture = restart(stopped, config).await;
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
    assert_eq!(
        fixture.call("alice", "GET", &credential_path, None).await,
        (StatusCode::OK, saved_credential["status"].clone())
    );
    assert_eq!(
        restored_context
            .provider_credentials()
            .unwrap()
            .resolve("openai".into())
            .await
            .unwrap()
            .as_deref(),
        Some("RESTORE-PRIVATE-CANARY")
    );
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

#[tokio::test]
async fn restored_interrupted_legacy_document_write_recovers_before_reads_and_future_intent_fails_closed()
 {
    for version in [1, 999] {
        let fixture = Fixture::new().await;
        let alice = fixture.login("alice").await;
        let vault = &alice.personal_vault_id;
        let (_, created) = fixture
            .call(
                "alice",
                "POST",
                &format!("/api/v1/vaults/{vault}/sessions"),
                Some(json!({"title":"Upgrade recovery","markdown":"Personal note"})),
            )
            .await;
        let session = created["meta"]["id"].as_str().unwrap().to_owned();
        let context = fixture.service.vault(vault).await.unwrap();
        context.store.ensure_summary(&session).await.unwrap();
        context
            .store
            .update_generated_summary(&session, "- [ ] Call Alice", Some(""))
            .await
            .unwrap();
        let target = DocumentTarget::Summary;
        let before = context
            .store
            .read_document_snapshot(&session, &target)
            .await
            .unwrap()
            .unwrap();
        let mut tasks = context.store.session_tasks(&session).await.unwrap();
        tasks[0].status = "done".into();
        let vault_path = context.store.vault_base().to_path_buf();
        let directory = vault_path.join("sessions").join(&session);
        let mut task_bytes = vec![1];
        task_bytes.extend(std::fs::read(directory.join("tasks.json")).unwrap());
        drop(context);
        let stopped = stop(fixture).await;
        // A killed previous executable committed Markdown but not the paired task state.
        std::fs::write(directory.join("summary.md"), "- [x] Call Alice").unwrap();
        let store = hypr_vault_write::SessionStore::new(vault_path);
        let after = store
            .read_document_snapshot(&session, &target)
            .await
            .unwrap()
            .unwrap();
        drop(store);
        let pending_path = stopped
            .config
            .storage_root
            .join("runtime")
            .join(vault)
            .join("creates/document-saves")
            .join(format!(
                "{}.json",
                capture::hash(session.as_bytes().to_vec())
            ));
        let intent = json!({"schema_version":version,"session_id":session,"target":target,"before_revision":before.revision,"after_revision":after.revision,"before_tasks_hash":capture::hash(task_bytes),"tasks":tasks});
        hypr_storage::fs::atomic_write(&pending_path, &intent.to_string()).unwrap();
        let original = storage_hashes(&stopped.config.storage_root);
        let root = tempfile::tempdir().unwrap();
        let snapshot = root.path().join("backup");
        let secrets = root.path().join("private");
        crate::maintenance::backup(&stopped.config.storage_root, &snapshot, &secrets)
            .await
            .unwrap();
        let destination = root.path().join("restored");
        crate::maintenance::restore(&snapshot, None, &destination)
            .await
            .unwrap();
        let mut config = stopped.config.clone();
        let original_storage = config.storage_root.clone();
        config.storage_root = destination.clone();
        config.cache_root = root.path().join("regenerated-cache");
        let fixture = restart(stopped, config).await;
        let path = format!("/api/v1/vaults/{vault}/sessions/{session}/documents");
        let (status, documents) = fixture.call("alice", "GET", &path, None).await;
        if version == 1 {
            assert_eq!(status, StatusCode::OK, "{documents}");
            assert!(documents.to_string().contains("- [x] Call Alice"));
            let context = fixture.service.vault(vault).await.unwrap();
            assert_eq!(context.store.session_tasks(&session).await.unwrap(), tasks);
            assert!(
                !destination
                    .join(pending_path.strip_prefix(&original_storage).unwrap())
                    .exists()
            );
            assert!(fixture.service.config.cache_root.exists());
        } else {
            assert!(!status.is_success());
            assert_eq!(
                std::fs::read(
                    destination.join(pending_path.strip_prefix(&original_storage).unwrap())
                )
                .unwrap(),
                intent.to_string().as_bytes()
            );
            assert_eq!(
                std::fs::read(
                    destination.join(format!("vaults/{vault}/sessions/{session}/tasks.json"))
                )
                .unwrap(),
                std::fs::read(directory.join("tasks.json")).unwrap()
            );
        }
        assert_eq!(storage_hashes(&original_storage), original);
        fixture.service.shutdown().await.unwrap();
    }
}

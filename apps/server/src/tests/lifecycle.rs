use super::*;
use std::{fs::File, path::Path, time::Duration};

fn unlocked(root: &Path, id: &str) -> bool {
    let file = File::options()
        .read(true)
        .write(true)
        .open(root.join("vaults").join(id).join(".loofah-vault.lock"))
        .unwrap();
    file.try_lock().is_ok()
}

#[tokio::test]
async fn shutdown_deadline_retains_service_vault_ownership_until_admitted_operations_exit() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    let startup = context.startup.clone();
    drop(context);
    let read = fixture
        .service
        .content_reads
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    let outcome = fixture
        .service
        .shutdown_with_deadline(Duration::from_millis(20))
        .await;
    assert!(matches!(outcome, Err(ref cause) if cause.0.code == ErrorCode::Unavailable));
    assert!(fixture.service.stop.is_cancelled());
    assert!(!unlocked(
        &fixture.service.config.storage_root,
        &me.personal_vault_id
    ));
    assert!(
        matches!(fixture.service.vault(&me.personal_vault_id).await, Err(ref cause) if cause.0.code == ErrorCode::Unavailable)
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            fixture.service.wait_for_shutdown()
        )
        .await
        .is_err()
    );
    drop(read);
    tokio::time::timeout(Duration::from_secs(10), fixture.service.wait_for_shutdown())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        startup.snapshot().phase,
        hypr_loofah_core::startup::StartupPhase::Stopped
    );
    assert!(unlocked(
        &fixture.service.config.storage_root,
        &me.personal_vault_id
    ));
}

#[tokio::test]
async fn cancelled_shutdown_waiter_does_not_cancel_or_duplicate_the_owned_service_drain() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    let startup = context.startup.clone();
    drop(context);
    let read = fixture
        .service
        .content_reads
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    let service = fixture.service.clone();
    let waiter = tokio::spawn(async move { service.shutdown().await });
    fixture.service.stop.cancelled().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(!unlocked(
        &fixture.service.config.storage_root,
        &me.personal_vault_id
    ));
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            fixture.service.wait_for_shutdown()
        )
        .await
        .is_err()
    );
    drop(read);
    tokio::time::timeout(Duration::from_secs(10), fixture.service.wait_for_shutdown())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        startup.snapshot().phase,
        hypr_loofah_core::startup::StartupPhase::Stopped
    );
    assert!(unlocked(
        &fixture.service.config.storage_root,
        &me.personal_vault_id
    ));
}

#[tokio::test]
async fn server_deadline_waits_for_the_detached_media_workers_actual_release_guard() {
    struct Admission {
        _permit: tokio::sync::OwnedSemaphorePermit,
        entered: Option<tokio::sync::oneshot::Sender<()>>,
        release: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    }
    impl Drop for Admission {
        fn drop(&mut self) {
            if let Some(entered) = self.entered.take() {
                let _ = entered.send(());
            }
            let mut released = self.release.0.lock().unwrap();
            while !*released {
                released = self.release.1.wait(released).unwrap();
            }
        }
    }
    struct Release(Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Media","created_at":"2026-11-08T10:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Preserve note")
        .await
        .unwrap();
    std::fs::write(
        context.store.vault_base().join("sessions/note/audio.wav"),
        b"0123456789",
    )
    .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    fixture
        .service
        .media_leases
        .open(context.clone(), "owner", &id, "note")
        .await
        .unwrap()
        .unwrap();
    let reader = fixture
        .service
        .media_leases
        .reader("owner", &me.personal_vault_id, &id)
        .unwrap();
    drop(context);
    let (entered, started) = tokio::sync::oneshot::channel();
    let release = Release(Arc::new((
        std::sync::Mutex::new(false),
        std::sync::Condvar::new(),
    )));
    let admission = Admission {
        _permit: fixture
            .service
            .media_reads
            .clone()
            .acquire_owned()
            .await
            .unwrap(),
        entered: Some(entered),
        release: release.0.clone(),
    };
    let call = tokio::spawn(async move { reader.read_chunk_with_guard(0, 1, admission).await });
    tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .unwrap()
        .unwrap();
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    assert!(
        fixture
            .service
            .shutdown_with_deadline(Duration::from_millis(20))
            .await
            .is_err()
    );
    assert!(!unlocked(
        &fixture.service.config.storage_root,
        &me.personal_vault_id
    ));
    drop(release);
    tokio::time::timeout(Duration::from_secs(10), fixture.service.wait_for_shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(unlocked(
        &fixture.service.config.storage_root,
        &me.personal_vault_id
    ));
}

#[tokio::test]
async fn cancelled_catalogue_request_keeps_shutdown_pending_until_actual_commit_finishes() {
    struct Release(Option<Arc<std::sync::Barrier>>);
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(barrier) = self.0.take() {
                std::thread::spawn(move || barrier.wait());
            }
        }
    }
    let fixture = Fixture::new().await;
    let (entered, receiver) = std::sync::mpsc::channel();
    let resume = Arc::new(std::sync::Barrier::new(2));
    let release = Release(Some(resume.clone()));
    *fixture.service.catalog.pause.lock().unwrap() = Some((entered, resume));
    let app = fixture.app.clone();
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/me")
        .header(
            "authorization",
            format!("Bearer {}", fixture.token("alice")),
        )
        .header("x-loofah-api-major", "1")
        .body(Body::empty())
        .unwrap();
    let call = tokio::spawn(async move { app.oneshot(request).await });
    tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap()
        .unwrap();
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    assert!(fixture.service.writes.available_permits() < 32);
    assert!(
        matches!(fixture.service.shutdown_with_deadline(Duration::from_millis(20)).await,
            Err(ref cause) if cause.0.code == ErrorCode::Unavailable)
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            fixture.service.wait_for_shutdown()
        )
        .await
        .is_err()
    );
    drop(release);
    tokio::time::timeout(Duration::from_secs(10), fixture.service.wait_for_shutdown())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fixture.service.writes.available_permits(), 32);
    let stored: Value = serde_json::from_slice(
        &std::fs::read(fixture.service.config.storage_root.join("catalog.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(stored["members"].as_object().unwrap().len(), 1);
}

#[tokio::test]
async fn failed_post_open_preparation_drains_before_publishing_an_empty_retryable_runtime_cell() {
    let provider = super::processing::Provider::new(0).await;
    let fixture = Fixture::with_processing(Some(provider.config())).await;
    let me = fixture.login("alice").await;
    let jobs = fixture
        .service
        .config
        .storage_root
        .join("runtime")
        .join(&me.personal_vault_id)
        .join("creates/jobs");
    std::fs::create_dir_all(jobs.parent().unwrap()).unwrap();
    std::fs::write(&jobs, b"Preserve invalid private storage").unwrap();
    assert!(fixture.service.vault(&me.personal_vault_id).await.is_err());
    assert!(unlocked(
        &fixture.service.config.storage_root,
        &me.personal_vault_id
    ));
    assert_eq!(
        std::fs::read(&jobs).unwrap(),
        b"Preserve invalid private storage"
    );
    std::fs::remove_file(jobs).unwrap();
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    drop(context);
    fixture.service.shutdown().await.unwrap();
    assert!(unlocked(
        &fixture.service.config.storage_root,
        &me.personal_vault_id
    ));
    provider.close().await;
}

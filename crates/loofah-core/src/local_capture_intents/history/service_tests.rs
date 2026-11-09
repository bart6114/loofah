use super::*;
use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
use std::future::Future;

async fn context(root: &Path) -> Arc<VaultContext> {
    let vault = root.join("vault");
    fs::create_dir_all(&vault).unwrap();
    let hypr_storage::vault::LeaseAttempt::Acquired(lease) =
        hypr_storage::vault::VaultLease::try_acquire_existing(&vault).unwrap()
    else {
        panic!("fixture lease")
    };
    let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
    if !vault.join("sessions/note/_meta.json").exists() {
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Original","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "Original note")
            .await
            .unwrap();
    }
    Arc::new(
        VaultContext::new(
            "original-vault".into(),
            store,
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.join("operations"))
        .with_vault_lease(lease)
        .unwrap(),
    )
}
fn declaration() -> LocalCaptureDeclaration {
    LocalCaptureDeclaration {
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id: "note".into(),
        append: false,
        batch_requested: false,
        transcription: None,
        summary: None,
    }
}
fn root() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

#[tokio::test]
async fn more_than_4096_real_local_admissions_preserve_uuid_history_without_active_growth() {
    let root = root();
    let context = context(root.path()).await;
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let first = declaration();
    let token = service.declare(first.clone()).await.unwrap();
    token.not_started().await.unwrap();
    for _ in 0..4096 {
        let token = service.declare(declaration()).await.unwrap();
        token.not_started().await.unwrap();
        assert_eq!(service.state.lock().await.len(), 1);
    }
    assert_eq!(service.history.head().unwrap().total, 4096);
    let historical = service.status(&first.request_id).await.unwrap();
    assert_eq!(historical.declaration, first);
    assert_eq!(historical.state, LocalCaptureState::NotStarted);
    assert!(
        service
            .control_status(&first.request_id)
            .await
            .unwrap()
            .actions
            .is_empty()
    );
    token.not_started().await.unwrap();
    assert!(token.physical_finished(Ok(())).await.is_err());
    let mut changed = first.clone();
    changed.session_id = "nonexistent-session".into();
    assert_eq!(
        service.declare(changed).await.err().unwrap().code,
        ErrorCode::Conflict
    );
    assert_eq!(
        service.declare(first.clone()).await.err().unwrap().code,
        ErrorCode::Conflict
    );
    assert_eq!(
        service.recover(first.request_id.clone()).await.unwrap(),
        historical
    );
    let page = service.history(Some("note"), None, 25).await.unwrap();
    assert_eq!(page.items.len(), 25);
    assert!(page.next_cursor.is_some());
    assert!(page.items.iter().all(|item| item.actions.is_empty()));
    service.shutdown().await;
    drop(token);
    drop(service);
    drop(context);
    let reopened_context = self::context(root.path()).await;
    let reopened = LocalCaptureIntents::open(reopened_context).await.unwrap();
    assert!(reopened.state.lock().await.is_empty());
    assert_eq!(reopened.history.head().unwrap().total, 4097);
    assert_eq!(
        reopened.status(&first.request_id).await.unwrap(),
        historical
    );
    let fresh = reopened.declare(declaration()).await.unwrap();
    fresh.not_started().await.unwrap();
    reopened.shutdown().await;
}

#[test]
fn full_legacy_active_inventory_does_not_count_the_reserved_history_directory() {
    let root = root();
    let active = root.path().join("local-capture-intents");
    let history = History::open(&active, "original-vault").unwrap();
    history.initialize().unwrap();
    for _ in 0..4096 {
        let mut record = tests::record();
        record.version = 1;
        let directory = active.join(&record.status.declaration.request_id);
        fs::create_dir(&directory).unwrap();
        fs::write(
            directory.join("intent.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(
        journal::load(&active, "original-vault").unwrap().len(),
        4096
    );
    assert_eq!(history.head().unwrap().total, 0);
}

#[tokio::test]
async fn archive_waits_for_original_record_reader_and_preserves_abandoned_io_ownership() {
    let root = root();
    let context = context(root.path()).await;
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let input = declaration();
    service
        .declare(input.clone())
        .await
        .unwrap()
        .not_started()
        .await
        .unwrap();
    let original = service.read_original(&input.request_id).await.unwrap();
    let original_directory = original.directory.clone();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let pinned = context.clone();
    let caller = tokio::spawn(async move {
        tokio::task::spawn_blocking(move || {
            let (_context, _read) = (pinned, original);
            entered.send(()).unwrap();
            released.recv().unwrap();
        })
        .await
        .unwrap();
    });
    waiting.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let mut archive = Box::pin(service.archive_completed());
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(archive.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    assert!(original_directory.join("intent.json").exists());
    release.send(()).unwrap();
    archive.await.unwrap();
    assert!(!original_directory.exists());
    assert!(
        service
            .read_original(&input.request_id)
            .await
            .unwrap()
            .archived
    );
    service.shutdown().await;
}

#[tokio::test]
async fn shutdown_waits_for_abandoned_archive_io_and_closes_new_history_admission() {
    let root = root();
    let context = context(root.path()).await;
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let input = declaration();
    service
        .declare(input.clone())
        .await
        .unwrap()
        .not_started()
        .await
        .unwrap();
    let (entered, release) = tests::pause(&service.history.directory, "move");
    let archive_service = service.clone();
    let caller = tokio::spawn(async move { archive_service.archive_completed().await });
    entered.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let mut stopping = Box::pin(service.shutdown());
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(stopping.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    assert!(service.history(None, None, 10).await.is_err());
    // Already-held in-memory completion remains inspectable without new I/O.
    assert_eq!(
        service.status(&input.request_id).await.unwrap().state,
        LocalCaptureState::NotStarted
    );
    assert!(!service.state.lock().await.is_empty());
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), stopping)
        .await
        .unwrap();
    assert!(service.state.lock().await.is_empty());
    assert_eq!(service.history.head().unwrap().total, 1);
}

#[tokio::test]
async fn shutdown_waits_for_abandoned_history_reader_actual_end() {
    let root = root();
    let context = context(root.path()).await;
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let input = declaration();
    service
        .declare(input.clone())
        .await
        .unwrap()
        .not_started()
        .await
        .unwrap();
    service.archive_completed().await.unwrap();
    let original = service
        .read_public_original(&input.request_id)
        .await
        .unwrap();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let pinned = context.clone();
    let caller = tokio::spawn(async move {
        tokio::task::spawn_blocking(move || {
            let (_context, _read) = (pinned, original);
            entered.send(()).unwrap();
            released.recv().unwrap();
        })
        .await
        .unwrap();
    });
    waiting.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let mut stopping = Box::pin(service.shutdown());
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(stopping.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    assert!(service.history(None, None, 10).await.is_err());
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), stopping)
        .await
        .unwrap();
}

#[tokio::test]
async fn archived_review_resolution_replays_original_decision_without_current_session_reads() {
    let root = root();
    let context = context(root.path()).await;
    let mut record = tests::record();
    record.version = 3;
    record.status.declaration = declaration();
    record.status.state = LocalCaptureState::Closed;
    record.processing_drained = true;
    record.physical = Some(Ok(()));
    record.persistence = Some(Ok(LocalCapturePersistence {
        live_transcription_active: false,
        needs_batch_repair: false,
        new_transcript_id: None,
    }));
    let resolution = LocalCaptureResolution {
        resolution_id: uuid::Uuid::new_v4().to_string(),
        expected_revision: "a".repeat(64),
        decision: LocalCaptureResolutionDecision::KeepCurrentContent,
    };
    record.resolution = Some(resolution.clone());
    let request = record.status.declaration.request_id.clone();
    let active = root.path().join("operations/local-capture-intents");
    journal::persist(&active, &record, true).unwrap();
    let original = fs::read(active.join(&request).join("intent.json")).unwrap();
    fs::write(
        active.join(&request).join("unknown-private-copy"),
        b"retained",
    )
    .unwrap();
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    assert!(service.state.lock().await.is_empty());
    fs::remove_file(root.path().join("vault/sessions/note/_meta.json")).unwrap();
    let snapshot = service.control_status(&request).await.unwrap();
    assert_eq!(snapshot.status.state, LocalCaptureState::Closed);
    assert!(snapshot.actions.is_empty());
    assert_eq!(
        service
            .resolve(request.clone(), resolution.clone())
            .await
            .unwrap(),
        snapshot
    );
    let mut changed = resolution;
    changed.resolution_id = uuid::Uuid::new_v4().to_string();
    assert!(service.resolve(request.clone(), changed).await.is_err());
    let retained = service.read_public_original(&request).await.unwrap();
    assert!(retained.archived);
    assert_eq!(
        fs::read(retained.directory.join("intent.json")).unwrap(),
        original
    );
    assert_eq!(
        fs::read(retained.directory.join("unknown-private-copy")).unwrap(),
        b"retained"
    );
    drop(retained);
    service.shutdown().await;
}

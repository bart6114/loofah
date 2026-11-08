use super::*;
use std::time::Duration;
fn input(markdown: &str) -> ExportInput {
    ExportInput {
        enhanced_md: markdown.into(),
        note_md: None,
        transcript: None,
        metadata: None,
        attachments: vec![],
    }
}
#[test]
fn export_images_only_resolve_explicit_portable_references() {
    let images=files::image_references(&input("![one](attachments/a%20b.png)\n![same](attachments/a%20b.png)\n![external](https://example.com/secret.png)\n`![code](attachments/not-an-image.png)`")).unwrap();
    assert_eq!(
        images,
        vec![("attachments/a%20b.png".into(), "a b.png".into())]
    );
    for value in [
        "attachments/..%2Fsecret.png",
        "attachments/.secret.png",
        "attachments/a%5Cb.png",
        "attachments/%FF.png",
    ] {
        assert!(files::image_references(&input(&format!("![x]({value})"))).is_err());
    }
}
#[test]
fn export_publication_never_overwrites_existing_or_follows_replaced_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap().join("Loofah");
    let one =
        files::publish(files::stage(root.clone(), "Meeting", "txt", b"one".to_vec()).unwrap())
            .unwrap();
    let two =
        files::publish(files::stage(root.clone(), "Meeting", "txt", b"two".to_vec()).unwrap())
            .unwrap();
    assert_ne!(one, two);
    assert_eq!(std::fs::read(&one).unwrap(), b"one");
    assert_eq!(std::fs::read(&two).unwrap(), b"two");
    let staged = files::stage(root.clone(), "Meeting", "txt", b"three".to_vec()).unwrap();
    std::fs::rename(&root, root.with_file_name("retired")).unwrap();
    std::fs::create_dir(&root).unwrap();
    assert!(files::publish(staged).is_err());
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
}
#[cfg(unix)]
#[test]
fn export_refuses_symlinked_download_directory() {
    let temp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(temp.path()).unwrap();
    let real = base.join("real");
    std::fs::create_dir(&real).unwrap();
    let root = base.join("Loofah");
    std::os::unix::fs::symlink(real, &root).unwrap();
    assert!(files::stage(root, "meeting", "pdf", vec![]).is_err());
}
#[tokio::test]
async fn cancelled_exports_keep_shutdown_admission_until_physical_work_returns() {
    let state = SessionExportState::default();
    let request = uuid::Uuid::new_v4().to_string();
    let epoch = state.owner("original").unwrap();
    let work = state
        .admit("original", &epoch, &request, CancellationToken::new())
        .unwrap();
    state.cancel("other", &epoch, &request);
    assert!(work.check().is_ok());
    state.release_window("original");
    assert!(work.check().is_err());
    assert!(state.pending());
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let task = tokio::task::spawn_blocking(move || {
        entered.send(()).unwrap();
        blocked.recv().unwrap();
        drop(work);
    });
    waiting.await.unwrap();
    state.request_shutdown();
    assert!(!state.drain(Duration::from_millis(10)).await);
    assert!(state.owner("new").is_err());
    release.send(()).unwrap();
    task.await.unwrap();
    assert!(state.drain(Duration::from_secs(1)).await);
}

#[test]
fn export_pre_admission_cancellation_is_window_scoped_and_cannot_be_evicted() {
    let state = SessionExportState::default();
    let request = uuid::Uuid::new_v4().to_string();
    let epoch = state.owner("first").unwrap();
    state.cancel("first", &epoch, &request);
    assert_eq!(
        state
            .admit("first", &epoch, &request, CancellationToken::new())
            .err()
            .unwrap()
            .code,
        ErrorCode::Cancelled
    );
    let other_epoch = state.owner("other").unwrap();
    let other = state
        .admit("other", &other_epoch, &request, CancellationToken::new())
        .unwrap();
    assert!(other.check().is_ok());
    drop(other);
    for _ in 0..130 {
        state.cancel("first", &epoch, &uuid::Uuid::new_v4().to_string());
    }
    assert!(
        state
            .admit(
                "first",
                &epoch,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new()
            )
            .is_err()
    );
    assert!(
        state
            .admit("first", &epoch, &request, CancellationToken::new())
            .is_err()
    );
    state.release_window("first");
    let next = state.owner("first").unwrap();
    assert_ne!(epoch, next);
    assert!(
        state
            .admit("first", &epoch, &request, CancellationToken::new())
            .is_err()
    );
    assert!(
        state
            .admit(
                "first",
                &next,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new()
            )
            .is_ok()
    );
}
#[test]
fn retired_export_window_cannot_rebind_or_cancel_the_replacement_generation() {
    let state = SessionExportState::default();
    let old = state.owner("window").unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let active = state
        .admit("window", &old, &request, CancellationToken::new())
        .unwrap();
    state.release_window("window");
    let current = state.owner("window").unwrap();
    assert_ne!(old, current);
    assert!(
        state
            .admit(
                "window",
                &old,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new()
            )
            .is_err()
    );
    let mut published = false;
    assert!(
        active
            .publish(|| {
                published = true;
                Ok(())
            })
            .is_err()
    );
    assert!(!published);
    drop(active);
    let replacement = state
        .admit("window", &current, &request, CancellationToken::new())
        .unwrap();
    state.cancel("window", &old, &request);
    replacement
        .publish(|| {
            published = true;
            Ok(())
        })
        .unwrap();
    assert!(published);
}

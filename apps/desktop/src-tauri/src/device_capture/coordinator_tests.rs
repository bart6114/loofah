use super::*;
use crate::device_capture::remote::RemoteCaptureCoordinator;
use hypr_loofah_client::{ConnectionProfiles, ProfileLease};

async fn profile(root: &std::path::Path) -> Arc<ProfileLease> {
    let profiles = ConnectionProfiles::new(root.join("profiles")).unwrap();
    let lease = profiles
        .create("fixture", "https://capture.example/")
        .await
        .unwrap();
    let mut stored = lease.profile().await;
    stored.member_id = Some("member".into());
    stored.vault_id = Some("remote".into());
    lease.save(stored).await.unwrap();
    lease
}

#[tokio::test]
async fn offline_coordinator_pins_original_profile_scope_and_joins_real_source_before_shutdown() {
    let _capture_test_guard = super::super::super::CAPTURE_TEST_LOCK.lock().await;
    let (temp, spool) = spool().await;
    let profile = profile(temp.path()).await;
    let profile_id = profile.profile().await.id;
    let observer = Arc::new(Observer::default());
    let (host, input, gate, root, handle) = root_host(
        spool.clone(),
        observer.clone(),
        Config {
            resource_gate: Some(Arc::new(Semaphore::new(0))),
            ..Default::default()
        },
    )
    .await;
    let coordinator =
        RemoteCaptureCoordinator::new(temp.path().join("captures"), observer).unwrap();
    coordinator.install_test_host(profile_id.clone(), spool.clone(), host.clone());
    let capture = coordinator
        .start(
            profile.clone(),
            None,
            CaptureTarget::ExistingSession {
                session_id: "note".into(),
                expected_audio_sha256: None,
            },
            CaptureFormat::stereo(),
        )
        .await
        .unwrap();
    let original = capture.identity.clone();
    let mut ticket = capture.device.clone();
    for _ in 0..5 {
        input.send(frame()).await.unwrap();
    }
    until(&mut ticket, |state| {
        state.completion.first_admitted_audio_observed_at.is_some()
    })
    .await;
    let mut changed = profile.profile().await;
    changed.member_id = Some("other-member".into());
    changed.vault_id = Some("other-vault".into());
    profile.save(changed).await.unwrap();
    let mut wrong = original.scope.clone();
    wrong.member_id = "other-member".into();
    assert!(
        coordinator
            .request_stop(&profile_id, &wrong, &ticket.key)
            .is_err()
    );
    assert!(
        coordinator
            .request_stop(&profile_id, &original.scope, &ticket.key)
            .unwrap()
    );
    let pending = coordinator.wait_shutdown(Duration::from_millis(10)).await;
    assert!(!pending.safe_to_exit);
    assert_eq!(pending.devices[0].pending.len(), 1);
    drop(capture);
    drop(ticket);
    gate.add_permits(1);
    let complete = coordinator.wait_shutdown(Duration::from_secs(5)).await;
    assert!(complete.safe_to_exit);
    let snapshot = spool
        .inventory(original.scope.clone())
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(snapshot.identity, original);
    assert_eq!(snapshot.frames, 5 * 1920);
    assert!(snapshot.seal.is_some());
    assert!(snapshot.acknowledged.is_none());
    assert!(snapshot.publication.is_none());
    root.stop(None);
    handle.await.unwrap();
}

#[tokio::test]
async fn dropping_coordinator_requests_real_device_stop_without_dropping_admitted_frames() {
    let _capture_test_guard = super::super::super::CAPTURE_TEST_LOCK.lock().await;
    let (temp, spool) = spool().await;
    let profile = profile(temp.path()).await;
    let profile_id = profile.profile().await.id;
    let observer = Arc::new(Observer::default());
    let (host, input, gate, root, handle) = root_host(
        spool.clone(),
        observer.clone(),
        Config {
            resource_gate: Some(Arc::new(Semaphore::new(0))),
            ..Default::default()
        },
    )
    .await;
    let coordinator =
        RemoteCaptureCoordinator::new(temp.path().join("captures"), observer).unwrap();
    coordinator.install_test_host(profile_id, spool.clone(), host.clone());
    let capture = coordinator
        .start(
            profile,
            None,
            CaptureTarget::PendingSession {
                create_request_id: uuid::Uuid::new_v4().to_string(),
            },
            CaptureFormat::stereo(),
        )
        .await
        .unwrap();
    let identity = capture.identity.clone();
    let mut ticket = capture.device;
    input.send(frame()).await.unwrap();
    until(&mut ticket, |state| {
        state.completion.first_admitted_audio_observed_at.is_some()
    })
    .await;
    drop(coordinator);
    let pending = host.wait_shutdown(Duration::from_millis(10)).await.unwrap();
    assert!(!pending.safe_to_exit);
    gate.add_permits(1);
    assert!(
        host.wait_shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .safe_to_exit
    );
    let snapshot = spool
        .inventory(identity.scope)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(snapshot.frames, 1920);
    assert!(matches!(
        snapshot.identity.target,
        CaptureTarget::PendingSession { .. }
    ));
    assert!(snapshot.session_id.is_none());
    assert!(snapshot.seal.is_some());
    root.stop(None);
    handle.await.unwrap();
}

#[tokio::test]
async fn append_is_refused_before_dispatch_or_spool_creation() {
    let _capture_test_guard = super::super::super::CAPTURE_TEST_LOCK.lock().await;
    let (temp, spool) = spool().await;
    let profile = profile(temp.path()).await;
    let profile_id = profile.profile().await.id;
    let observer = Arc::new(Observer::default());
    let mock = driver(true, StartMode::NeverAcknowledged);
    let host = DeviceCaptureHost::with_driver(spool.clone(), observer.clone(), mock.clone());
    let coordinator =
        RemoteCaptureCoordinator::new(temp.path().join("captures"), observer).unwrap();
    coordinator.install_test_host(profile_id, spool.clone(), host);
    let result = coordinator
        .start(
            profile.clone(),
            None,
            CaptureTarget::ExistingSession {
                session_id: "note".into(),
                expected_audio_sha256: Some("a".repeat(64)),
            },
            CaptureFormat::stereo(),
        )
        .await;
    assert!(matches!(result,Err(error) if error.code==ErrorCode::Validation));
    assert_eq!(mock.starts.load(Ordering::SeqCst), 0);
    assert!(coordinator.inventory(profile).await.unwrap().is_empty());
    assert!(
        coordinator
            .wait_shutdown(Duration::from_secs(1))
            .await
            .safe_to_exit
    );
}

#[cfg(unix)]
#[tokio::test]
async fn trusted_capture_parent_alias_resolves_once_but_owned_profile_symlink_is_rejected() {
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let lease = profile(temp.path()).await;
    let id = lease.profile().await.id;
    let real = temp.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let observer = Arc::new(Observer::default());
    let coordinator = RemoteCaptureCoordinator::new(alias, observer.clone()).unwrap();
    assert!(
        coordinator
            .inventory(lease.clone())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(real.join(&id).is_dir());
    assert!(
        coordinator
            .wait_shutdown(Duration::from_secs(1))
            .await
            .safe_to_exit
    );
    drop(coordinator);
    let root = temp.path().join("unsafe");
    std::fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink(real.join(&id), root.join(&id)).unwrap();
    let coordinator = RemoteCaptureCoordinator::new(root, observer).unwrap();
    assert!(
        matches!(coordinator.inventory(lease).await,Err(error) if error.code==ErrorCode::Internal)
    );
    assert!(
        coordinator
            .wait_shutdown(Duration::from_secs(1))
            .await
            .safe_to_exit
    );
}

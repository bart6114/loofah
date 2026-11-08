use super::*;
use crate::device_capture::remote::RemoteCaptureCoordinator;
use hypr_loofah_client::{ConnectionProfiles, ProfileLease};
use hypr_loofah_protocol::ErrorCode;

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
            uuid::Uuid::new_v4().to_string(),
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
            uuid::Uuid::new_v4().to_string(),
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
            uuid::Uuid::new_v4().to_string(),
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

#[tokio::test]
async fn concurrent_start_replay_reuses_one_capture_and_changed_declarations_never_retarget_it() {
    let _capture_test_guard = super::super::super::CAPTURE_TEST_LOCK.lock().await;
    let (temp, spool) = spool().await;
    let profile = profile(temp.path()).await;
    let original = profile.profile().await;
    let observer = Arc::new(Observer::default());
    let (host, input, _gate, root, handle) =
        root_host(spool.clone(), observer.clone(), Config::default()).await;
    let coordinator =
        RemoteCaptureCoordinator::new(temp.path().join("captures"), observer.clone()).unwrap();
    coordinator.install_test_host(original.id.clone(), spool.clone(), host.clone());
    let request = uuid::Uuid::new_v4().to_string();
    let target = CaptureTarget::ExistingSession {
        session_id: "note".into(),
        expected_audio_sha256: None,
    };
    let (left, right) = tokio::join!(
        coordinator.start(
            profile.clone(),
            None,
            request.clone(),
            target.clone(),
            CaptureFormat::stereo()
        ),
        coordinator.start(
            profile.clone(),
            None,
            request.clone(),
            target.clone(),
            CaptureFormat::stereo()
        )
    );
    let mut left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.identity, right.identity);
    assert_eq!(left.device.key, right.device.key);
    assert_eq!(left.identity.request_id, request);
    input.send(frame()).await.unwrap();
    until(&mut left.device, |state| {
        state.completion.first_admitted_audio_observed_at.is_some()
    })
    .await;
    let mut mono = CaptureFormat::stereo();
    mono.channels = vec![hypr_loofah_client::capture_spool::CaptureChannel::Microphone];
    for (target, format) in [
        (
            CaptureTarget::ExistingSession {
                session_id: "different".into(),
                expected_audio_sha256: None,
            },
            CaptureFormat::stereo(),
        ),
        (target.clone(), mono),
    ] {
        assert!(
            matches!(coordinator.start(profile.clone(),None,request.clone(),target,format).await,Err(error) if error.code==ErrorCode::Conflict)
        );
    }
    for member_changed in [false, true] {
        let mut changed = original.clone();
        if member_changed {
            changed.member_id = Some("different-member".into());
        } else {
            changed.vault_id = Some("different-vault".into());
        }
        profile.save(changed).await.unwrap();
        assert!(
            matches!(coordinator.start(profile.clone(),None,request.clone(),target.clone(),CaptureFormat::stereo()).await,Err(error) if error.code==ErrorCode::Conflict)
        );
        let replay = coordinator
            .start_scoped(
                profile.clone(),
                None,
                crate::device_capture::remote::DeviceCaptureStartRequest {
                    request_id: request.clone(),
                    scope: left.identity.scope.clone(),
                    target: target.clone(),
                    format: CaptureFormat::stereo(),
                },
            )
            .await
            .unwrap();
        assert_eq!(replay.identity, left.identity);
        assert_eq!(replay.device.key, left.device.key);
    }
    for invalid in ["invalid".to_owned(), uuid::Uuid::nil().to_string()] {
        assert!(
            matches!(coordinator.start(profile.clone(),None,invalid,target.clone(),CaptureFormat::stereo()).await,Err(error) if error.code==ErrorCode::Validation)
        );
    }
    assert!(
        coordinator
            .request_stop(&original.id, &left.identity.scope, &left.device.key)
            .unwrap()
    );
    assert!(
        coordinator
            .wait_shutdown(Duration::from_secs(5))
            .await
            .safe_to_exit
    );
    assert_eq!(host.state.lock().unwrap().history.len(), 1);
    let stored = spool.inventory(left.identity.scope.clone()).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].identity, left.identity);
    assert_eq!(stored[0].format, CaptureFormat::stereo());
    assert_eq!(stored[0].frames, 1920);
    assert_eq!(
        observer
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event.event, DeviceCaptureEvent::Preparing))
            .count(),
        1
    );
    root.stop(None);
    handle.await.unwrap();
}

#[tokio::test]
async fn abandoned_start_waiter_after_real_dispatch_cannot_lose_mapping_or_start_twice() {
    let _capture_test_guard = super::super::super::CAPTURE_TEST_LOCK.lock().await;
    let (temp, spool) = spool().await;
    let profile = profile(temp.path()).await;
    let stored_profile = profile.profile().await;
    let observer = Arc::new(Observer::default());
    let (host, input, _gate, root, handle) =
        root_host(spool.clone(), observer.clone(), Config::default()).await;
    let coordinator =
        RemoteCaptureCoordinator::new(temp.path().join("captures"), observer).unwrap();
    coordinator.install_test_host(stored_profile.id.clone(), spool.clone(), host.clone());
    let request = uuid::Uuid::new_v4().to_string();
    let target = CaptureTarget::ExistingSession {
        session_id: "note".into(),
        expected_audio_sha256: None,
    };
    let (entered, paused) = tokio::sync::oneshot::channel();
    let release = Arc::new(Semaphore::new(0));
    coordinator.pause_mapping(entered, release.clone());
    let first = tokio::spawn({
        let coordinator = coordinator.clone();
        let profile = profile.clone();
        let request = request.clone();
        let target = target.clone();
        async move {
            coordinator
                .start(profile, None, request, target, CaptureFormat::stereo())
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), paused)
        .await
        .unwrap()
        .unwrap();
    input.send(frame()).await.unwrap();
    let actual = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let current = host
                .state
                .lock()
                .unwrap()
                .entries
                .values()
                .next()
                .map(|entry| entry.status.borrow().clone());
            if let Some(current) = current
                && current
                    .completion
                    .first_admitted_audio_observed_at
                    .is_some()
            {
                break current;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(actual.identity.request_id, request);
    assert!(
        coordinator
            .capture_tickets(&stored_profile.id, &actual.identity.scope)
            .is_empty()
    );
    first.abort();
    assert!(first.await.is_err());
    let second = tokio::spawn({
        let coordinator = coordinator.clone();
        let profile = profile.clone();
        let request = request.clone();
        async move {
            coordinator
                .start(profile, None, request, target, CaptureFormat::stereo())
                .await
        }
    });
    let admitted = tokio::time::timeout(Duration::from_secs(2), async {
        while coordinator.pending_starts() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let blocked = !second.is_finished();
    let before = coordinator.wait_shutdown(Duration::from_millis(10)).await;
    release.add_permits(1);
    admitted.unwrap();
    assert!(blocked);
    assert!(!before.safe_to_exit);
    let replay = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(replay.identity, actual.identity);
    assert_eq!(replay.device.key, actual.key);
    assert!(
        coordinator
            .wait_shutdown(Duration::from_secs(5))
            .await
            .safe_to_exit
    );
    assert_eq!(
        coordinator
            .capture_tickets(&stored_profile.id, &actual.identity.scope)
            .len(),
        1
    );
    assert_eq!(host.state.lock().unwrap().history.len(), 1);
    let snapshot = spool
        .inventory(actual.identity.scope)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(snapshot.identity.request_id, request);
    assert_eq!(snapshot.frames, 1920);
    assert!(snapshot.seal.is_some());
    root.stop(None);
    handle.await.unwrap();
}

#[tokio::test]
async fn existing_spool_request_after_restart_is_recovery_only_even_after_scope_selection_changes()
{
    let _capture_test_guard = super::super::super::CAPTURE_TEST_LOCK.lock().await;
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let profile = profile(temp.path()).await;
    let original = profile.profile().await;
    let root = temp.path().join("captures");
    let spool = CaptureSpool::open(root.join(&original.id), CaptureSpoolLimits::default())
        .await
        .unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let identity = CaptureIdentity {
        scope: CaptureScope {
            server: original.server.clone(),
            member_id: original.member_id.clone().unwrap(),
            vault_id: original.vault_id.clone().unwrap(),
        },
        capture_id: uuid::Uuid::new_v4().to_string(),
        request_id: request.clone(),
        target: CaptureTarget::ExistingSession {
            session_id: "note".into(),
            expected_audio_sha256: None,
        },
    };
    let writer = spool
        .begin(identity.clone(), CaptureFormat::stereo())
        .await
        .unwrap();
    let before = writer.snapshot().await.unwrap();
    drop(writer);
    drop(spool);
    let observer = Arc::new(Observer::default());
    let coordinator = RemoteCaptureCoordinator::new(root, observer.clone()).unwrap();
    for changed_scope in [false, true] {
        if changed_scope {
            let mut changed = original.clone();
            changed.member_id = Some("another-member".into());
            changed.vault_id = Some("another-vault".into());
            profile.save(changed).await.unwrap();
        }
        assert!(
            matches!(coordinator.start(profile.clone(),None,request.clone(),identity.target.clone(),CaptureFormat::stereo()).await,Err(error) if error.code==ErrorCode::Conflict)
        );
    }
    let mut signed_out = original.clone();
    signed_out.member_id = None;
    signed_out.vault_id = None;
    profile.save(signed_out).await.unwrap();
    let recovered = coordinator
        .recovery_inventory(profile.clone())
        .await
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0], before);
    profile.save(original).await.unwrap();
    assert_eq!(coordinator.inventory(profile).await.unwrap(), vec![before]);
    assert!(observer.0.lock().unwrap().is_empty());
    assert!(
        coordinator
            .wait_shutdown(Duration::from_secs(1))
            .await
            .safe_to_exit
    );
}

#[tokio::test]
async fn explicit_start_scope_cannot_follow_a_later_saved_destination() {
    let (temp, _) = spool().await;
    let profile = profile(temp.path()).await;
    let (_, scope) = crate::device_capture::remote::profile_scope(&profile)
        .await
        .unwrap();
    let coordinator = RemoteCaptureCoordinator::new(
        temp.path().join("must-not-create"),
        Arc::new(Observer::default()),
    )
    .unwrap();
    let mut changed = profile.profile().await;
    changed.vault_id = Some("changed-vault".into());
    profile.save(changed).await.unwrap();
    let result = coordinator
        .start_scoped(
            profile,
            None,
            crate::device_capture::remote::DeviceCaptureStartRequest {
                request_id: uuid::Uuid::new_v4().to_string(),
                scope,
                target: CaptureTarget::ExistingSession {
                    session_id: "note".into(),
                    expected_audio_sha256: None,
                },
                format: CaptureFormat::stereo(),
            },
        )
        .await;
    assert!(matches!(result, Err(error) if error.code == ErrorCode::Conflict));
    assert!(!temp.path().join("must-not-create").exists());
    assert!(
        coordinator
            .wait_shutdown(Duration::from_secs(1))
            .await
            .safe_to_exit
    );
}

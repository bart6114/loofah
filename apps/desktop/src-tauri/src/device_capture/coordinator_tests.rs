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

fn recovery_identity() -> CaptureIdentity {
    CaptureIdentity {
        scope: CaptureScope {
            server: "https://capture.example/".into(),
            member_id: "member".into(),
            vault_id: "remote".into(),
        },
        capture_id: uuid::Uuid::new_v4().to_string(),
        request_id: uuid::Uuid::new_v4().to_string(),
        target: CaptureTarget::PendingSession {
            create_request_id: uuid::Uuid::new_v4().to_string(),
        },
    }
}

#[tokio::test]
async fn signed_out_recovery_finishes_and_exports_original_audio_without_creating_a_vault_note() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let profile = profile(&root).await;
    let original = profile.profile().await;
    let capture_root = root.join("captures");
    let spool = CaptureSpool::open(
        capture_root.join(&original.id),
        CaptureSpoolLimits::default(),
    )
    .await
    .unwrap();
    let identity = recovery_identity();
    let writer = spool
        .begin_with_observation(identity.clone(), CaptureFormat::stereo(), 1_794_000_000_001)
        .await
        .unwrap();
    let pcm = [0.25_f32.to_le_bytes(), (-0.5_f32).to_le_bytes()].concat();
    writer.append_chunk(0, 1, pcm.clone()).await.unwrap();
    let before = writer.snapshot().await.unwrap();
    drop(writer);
    drop(spool);
    let mut signed_out = original.clone();
    signed_out.member_id = None;
    signed_out.vault_id = None;
    profile.save(signed_out).await.unwrap();
    let coordinator =
        RemoteCaptureCoordinator::new(capture_root, Arc::new(Observer::default())).unwrap();
    let finished = coordinator
        .finish_recovery(profile.clone(), identity.clone())
        .await
        .unwrap();
    assert_eq!(finished.identity, before.identity);
    assert_eq!(
        finished.first_admitted_audio_observed_at_ms,
        before.first_admitted_audio_observed_at_ms
    );
    assert_eq!(finished.pcm_bytes, before.pcm_bytes);
    assert!(finished.seal.is_some());
    assert!(finished.session_id.is_none());
    assert!(finished.publication.is_none());
    let destination = root.join("recovered.wav");
    let exported = coordinator
        .export_recovery(profile.clone(), identity.clone(), destination.clone())
        .await
        .unwrap();
    assert!(exported.sealed);
    let bytes = std::fs::read(&destination).unwrap();
    assert_eq!(&bytes[bytes.len() - pcm.len()..], pcm);
    assert_eq!(
        coordinator
            .finish_recovery(profile.clone(), identity)
            .await
            .unwrap(),
        finished
    );
    assert_eq!(
        coordinator
            .recovery_inventory(profile.clone())
            .await
            .unwrap(),
        vec![finished]
    );
    assert!(!root.join("sessions").exists());
    assert!(profile.profile().await.member_id.is_none());
    assert!(
        coordinator
            .wait_shutdown(Duration::from_secs(5))
            .await
            .safe_to_exit
    );
}

#[tokio::test]
async fn untimed_saved_prefix_can_export_without_inventing_a_recording_timestamp_or_seal() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let profile = profile(&root).await;
    let profile_id = profile.profile().await.id;
    let capture_root = root.join("captures");
    let spool = CaptureSpool::open(
        capture_root.join(&profile_id),
        CaptureSpoolLimits::default(),
    )
    .await
    .unwrap();
    let identity = recovery_identity();
    let writer = spool
        .begin(identity.clone(), CaptureFormat::stereo())
        .await
        .unwrap();
    writer.append_chunk(0, 1, vec![0; 8]).await.unwrap();
    let before = writer.snapshot().await.unwrap();
    drop(writer);
    drop(spool);
    let coordinator =
        RemoteCaptureCoordinator::new(capture_root, Arc::new(Observer::default())).unwrap();
    assert!(
        matches!(coordinator.finish_recovery(profile.clone(),identity.clone()).await,Err(error) if error.code==ErrorCode::Conflict)
    );
    let receipt = coordinator
        .export_recovery(profile.clone(), identity, root.join("untimed.wav"))
        .await
        .unwrap();
    assert!(!receipt.sealed);
    assert_eq!(
        coordinator.recovery_inventory(profile).await.unwrap(),
        vec![before]
    );
    assert!(
        coordinator
            .wait_shutdown(Duration::from_secs(5))
            .await
            .safe_to_exit
    );
}

#[tokio::test]
async fn recovery_refuses_active_capture_without_sealing_or_exporting_a_live_prefix() {
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
    coordinator.install_test_host(profile_id.clone(), spool.clone(), host);
    let capture = coordinator
        .start(
            profile.clone(),
            None,
            uuid::Uuid::new_v4().to_string(),
            CaptureTarget::PendingSession {
                create_request_id: uuid::Uuid::new_v4().to_string(),
            },
            CaptureFormat::stereo(),
        )
        .await
        .unwrap();
    input.send(frame()).await.unwrap();
    let mut ticket = capture.device.clone();
    until(&mut ticket, |state| {
        state.completion.first_admitted_audio_observed_at.is_some()
    })
    .await;
    assert!(
        matches!(coordinator.finish_recovery(profile.clone(),capture.identity.clone()).await,Err(error) if error.code==ErrorCode::Conflict)
    );
    let output = temp.path().join("active.wav");
    assert!(
        matches!(coordinator.export_recovery(profile,capture.identity.clone(),output.clone()).await,Err(error) if error.code==ErrorCode::Conflict)
    );
    assert!(!output.exists());
    assert!(
        spool
            .resume(capture.identity.clone())
            .await
            .unwrap()
            .snapshot()
            .await
            .unwrap()
            .seal
            .is_none()
    );
    coordinator
        .request_stop(&profile_id, &capture.identity.scope, &ticket.key)
        .unwrap();
    gate.add_permits(1);
    assert!(
        coordinator
            .wait_shutdown(Duration::from_secs(5))
            .await
            .safe_to_exit
    );
    root.stop(None);
    handle.await.unwrap();
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
                    processing: None,
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
                processing: None,
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

#[tokio::test]
async fn explicit_processing_replay_cannot_change_options_and_offline_cancel_preserves_identity() {
    let _capture_test_guard = super::super::super::CAPTURE_TEST_LOCK.lock().await;
    let (temp, spool) = spool().await;
    let profile = profile(temp.path()).await;
    let saved_profile = profile.profile().await;
    let observer = Arc::new(Observer::default());
    let host = DeviceCaptureHost::with_driver(
        spool.clone(),
        observer.clone(),
        driver(true, StartMode::Reject),
    );
    let coordinator =
        RemoteCaptureCoordinator::new(temp.path().join("captures"), observer).unwrap();
    coordinator.install_test_host(saved_profile.id.clone(), spool.clone(), host);
    let scope = CaptureScope {
        server: saved_profile.server.clone(),
        member_id: saved_profile.member_id.clone().unwrap(),
        vault_id: saved_profile.vault_id.clone().unwrap(),
    };
    let request = crate::device_capture::remote::DeviceCaptureStartRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        scope,
        target: CaptureTarget::PendingSession {
            create_request_id: uuid::Uuid::new_v4().to_string(),
        },
        format: CaptureFormat::stereo(),
        processing: Some(
            hypr_capture_store::capture_spool::CaptureProcessingOptions {
                model: None,
                languages: vec!["en".into()],
                keywords: vec![],
                summary: None,
            },
        ),
    };
    let mut started = coordinator
        .start_scoped(profile.clone(), None, request.clone())
        .await
        .unwrap();
    until(&mut started.device, |state| {
        state.phase == DeviceHostPhase::NeverStarted
    })
    .await;
    let before = spool
        .resume(started.identity.clone())
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert_eq!(before.identity.request_id, request.request_id);
    assert!(before.processing.is_some());
    let replay = coordinator
        .start_scoped(profile.clone(), None, request.clone())
        .await
        .unwrap();
    assert_eq!(replay.identity, started.identity);
    let mut changed = request.clone();
    changed.processing.as_mut().unwrap().languages = vec!["nl".into()];
    assert!(
        matches!(coordinator.start_scoped(profile.clone(), None, changed).await, Err(cause) if cause.code == ErrorCode::Conflict)
    );
    let mut raw = request;
    raw.processing = None;
    assert!(
        matches!(coordinator.start_scoped(profile.clone(), None, raw).await, Err(cause) if cause.code == ErrorCode::Conflict)
    );
    let mut signed_out = saved_profile;
    signed_out.member_id = None;
    signed_out.vault_id = None;
    profile.save(signed_out).await.unwrap();
    let cancelled = coordinator
        .cancel_processing(profile.clone(), started.identity.clone())
        .await
        .unwrap();
    assert_eq!(cancelled.identity, before.identity);
    assert_eq!(
        cancelled.processing.as_ref().unwrap().request,
        before.processing.unwrap().request
    );
    assert!(cancelled.processing.unwrap().cancel_requested);
    assert!(
        coordinator
            .upload_ticket(&started.profile_id, &started.identity)
            .unwrap()
            .is_none()
    );
    assert!(
        coordinator
            .wait_shutdown(Duration::from_secs(2))
            .await
            .safe_to_exit
    );
}

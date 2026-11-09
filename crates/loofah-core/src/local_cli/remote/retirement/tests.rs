use super::*;
use crate::local_cli::remote::tests::{Fixture, SUBTITLE, done};

fn decision(
    status: &MediaRetirementStatus,
    disposition: MediaSourceDisposition,
) -> MediaRetirementRequest {
    MediaRetirementRequest {
        retirement_id: uuid::Uuid::new_v4().to_string(),
        expected_revision: status.revision.clone(),
        expected_declaration_sha256: status.declaration_sha256.clone(),
        expected_source: status.source.clone(),
        disposition,
    }
}
async fn drained(media: &MediaDispatcher, request: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while media.ensure_retirement_drained(request).await.is_err() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
async fn successful(fixture: &Fixture) -> MediaDeclaration {
    let declaration = fixture.declaration(SUBTITLE).await;
    let admitted = fixture.media.declare(declaration.clone()).await.unwrap();
    fixture
        .media
        .receive(declaration.request_id.clone(), admitted.revision, SUBTITLE)
        .await
        .unwrap();
    assert_eq!(
        done(&fixture.media, &declaration.request_id).await.phase,
        MediaPhase::Succeeded
    );
    drained(&fixture.media, &declaration.request_id).await;
    declaration
}
async fn planned(fixture: &Fixture, request: &str, input: MediaRetirementRequest) -> Admission {
    let mut record = fixture.media.load(request).await.unwrap().unwrap();
    fixture.media.retirement_eligibility(&record).await.unwrap();
    let (present, files) = source_inventory(
        &fixture.media.root.join(format!("{request}.source")),
        source(&record).as_ref(),
        input.disposition,
    )
    .unwrap();
    if record.dispatch == Some(journal::DispatchEvidence::NeverDispatched) {
        record.phase = MediaPhase::Cancelled;
    }
    record.retirement = Some(Intent {
        request: input,
        files,
        source_directory_present: present,
        completed: false,
    });
    fixture.media.save(record.clone(), false).await.unwrap();
    record
}

#[tokio::test]
async fn media_retirement_releases_both_quotas_but_never_reuses_original_identity() {
    let mut fixture = Fixture::new().await;
    fixture.media = MediaDispatcher::new(
        fixture.runtime.context.clone(),
        "member-a".into(),
        fixture.authority.clone(),
        MediaLimits {
            requests: 1,
            retained_bytes: SUBTITLE.len() as u64,
            source_bytes: SUBTITLE.len() as u64,
        },
    )
    .unwrap();
    let declaration = fixture.declaration(SUBTITLE).await;
    let admitted = fixture.media.declare(declaration.clone()).await.unwrap();
    let next = fixture.declaration(SUBTITLE).await;
    assert!(fixture.media.declare(next.clone()).await.is_err());
    let status = fixture
        .media
        .retirement_status(&declaration.request_id)
        .await
        .unwrap();
    assert_eq!(status.state, MediaRetirementState::Eligible);
    assert_eq!(
        status.allowed_dispositions,
        vec![MediaSourceDisposition::Discard]
    );
    let input = decision(&status, MediaSourceDisposition::Discard);
    let retired = fixture
        .media
        .retire(declaration.request_id.clone(), input.clone())
        .await
        .unwrap();
    assert!(retired.quota_released);
    assert_eq!(retired.state, MediaRetirementState::Retired);
    assert!(fixture.media.inventory().await.unwrap().is_empty());
    let replay = fixture.media.declare(declaration.clone()).await.unwrap();
    assert_eq!(replay.revision, admitted.revision);
    assert_eq!(replay.status.phase, MediaPhase::Cancelled);
    assert!(
        fixture
            .media
            .receive(declaration.request_id.clone(), replay.revision, SUBTITLE)
            .await
            .is_err()
    );
    assert!(
        fixture
            .media
            .recover(&declaration.request_id)
            .await
            .is_err()
    );
    assert!(fixture.media.cancel(&declaration.request_id).await.is_err());
    let mut changed = input.clone();
    changed.retirement_id = uuid::Uuid::new_v4().to_string();
    assert!(
        fixture
            .media
            .retire(declaration.request_id.clone(), changed)
            .await
            .is_err()
    );
    assert_eq!(
        fixture
            .media
            .retire(declaration.request_id.clone(), input)
            .await
            .unwrap(),
        retired
    );
    fixture.media.declare(next).await.unwrap();
    let reopened = MediaDispatcher::new(
        fixture.runtime.context.clone(),
        "member-a".into(),
        fixture.authority.clone(),
        MediaLimits::default(),
    )
    .unwrap();
    assert!(
        reopened
            .retirement_status(&declaration.request_id)
            .await
            .unwrap()
            .quota_released
    );
    assert_eq!(
        reopened.declare(declaration).await.unwrap().revision,
        admitted.revision
    );
    reopened.shutdown().await;
    fixture.stop().await;
}

#[tokio::test]
async fn media_retirement_waits_for_source_reader_and_preserves_canonical_and_result() {
    let fixture = Fixture::new().await;
    let declaration = successful(&fixture).await;
    let request = &declaration.request_id;
    let canonical = fixture
        .root
        .path()
        .join("vault/sessions")
        .join(&fixture.session)
        .join("transcript.json");
    let before = std::fs::read(&canonical).unwrap();
    let result = fixture.media.result(request).await.unwrap();
    assert!(result.is_some());
    let status = fixture.media.retirement_status(request).await.unwrap();
    let input = decision(&status, MediaSourceDisposition::Exported);
    let active_session = fixture
        .runtime
        .context
        .admit_session_work(&fixture.session)
        .await
        .unwrap();
    assert!(
        fixture
            .media
            .retire(request.clone(), input.clone())
            .await
            .is_err()
    );
    drop(active_session);
    let source = fixture
        .media
        .open_source(request, &status.revision)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .media
            .retirement_status(request)
            .await
            .unwrap()
            .state,
        MediaRetirementState::Blocked
    );
    assert!(
        fixture
            .media
            .retire(request.clone(), input.clone())
            .await
            .is_err()
    );
    drop(source);
    let retired = fixture.media.retire(request.clone(), input).await.unwrap();
    assert!(retired.quota_released);
    assert_eq!(std::fs::read(&canonical).unwrap(), before);
    assert_eq!(
        serde_json::to_value(fixture.media.result(request).await.unwrap()).unwrap(),
        serde_json::to_value(&result).unwrap()
    );
    assert_eq!(
        fixture.media.status(request).await.unwrap().phase,
        MediaPhase::Succeeded
    );
    assert!(
        fixture
            .media
            .open_source(request, &status.revision)
            .await
            .is_err()
    );
    assert!(!fixture.media.tasks.lock().await.contains_key(request));
    assert!(!fixture.media.receivers.lock().await.contains_key(request));
    assert!(
        fixture
            .media
            .root
            .join(format!("{request}.source"))
            .is_dir()
    );
    assert_eq!(
        std::fs::read_dir(fixture.media.root.join(format!("{request}.source")))
            .unwrap()
            .count(),
        0
    );
    std::fs::remove_file(&canonical).unwrap();
    assert_eq!(
        serde_json::to_value(fixture.media.result(request).await.unwrap()).unwrap(),
        serde_json::to_value(&result).unwrap()
    );
    assert!(!canonical.exists());
    fixture.stop().await;
}

#[tokio::test]
async fn media_retirement_refuses_ambiguous_legacy_and_dispatched_terminal_states() {
    let fixture = Fixture::new().await;
    let declaration = fixture.declaration(SUBTITLE).await;
    fixture.media.declare(declaration.clone()).await.unwrap();
    let original = fixture
        .media
        .load(&declaration.request_id)
        .await
        .unwrap()
        .unwrap();
    for phase in [
        MediaPhase::Review,
        MediaPhase::Failed,
        MediaPhase::Cancelled,
        MediaPhase::Running,
    ] {
        let mut record = original.clone();
        record.phase = phase;
        record.dispatch = Some(journal::DispatchEvidence::Dispatched);
        fixture.media.save(record, false).await.unwrap();
        let status = fixture
            .media
            .retirement_status(&declaration.request_id)
            .await
            .unwrap();
        assert_eq!(status.state, MediaRetirementState::Blocked);
        assert!(
            fixture
                .media
                .retire(
                    declaration.request_id.clone(),
                    decision(&status, MediaSourceDisposition::Discard)
                )
                .await
                .is_err()
        );
    }
    let mut legacy = original;
    legacy.version = 2;
    legacy.dispatch = None;
    fixture.media.save(legacy, false).await.unwrap();
    assert_eq!(
        fixture
            .media
            .retirement_status(&declaration.request_id)
            .await
            .unwrap()
            .state,
        MediaRetirementState::Blocked
    );
    assert_eq!(fixture.media.inventory().await.unwrap().len(), 1);
    fixture.stop().await;
}

#[tokio::test]
async fn media_retirement_crash_after_each_destructive_boundary_keeps_quota_and_exact_retry() {
    for boundary in [
        CleanupCheckpoint::SourceRemoved,
        CleanupCheckpoint::ReceiptSaved,
    ] {
        let fixture = Fixture::new().await;
        let declaration = fixture.declaration(SUBTITLE).await;
        fixture.media.declare(declaration.clone()).await.unwrap();
        let request = &declaration.request_id;
        let directory = fixture.media.root.join(format!("{request}.source"));
        ledger::write_new(&directory.join("partial"), &SUBTITLE[..8]).unwrap();
        let status = fixture.media.retirement_status(request).await.unwrap();
        let input = decision(&status, MediaSourceDisposition::Discard);
        let record = planned(&fixture, request, input.clone()).await;
        assert!(
            finish_with(&fixture.media.root, record, |stage| if stage == boundary {
                Err(unknown("Injected stopped cleanup"))
            } else {
                Ok(())
            })
            .is_err()
        );
        let pending = fixture.media.retirement_status(request).await.unwrap();
        assert_eq!(pending.state, MediaRetirementState::Retiring);
        assert!(!pending.quota_released);
        assert_eq!(fixture.media.inventory().await.unwrap().len(), 1);
        assert!(fixture.media.recover(request).await.is_err());
        let reopened = MediaDispatcher::new(
            fixture.runtime.context.clone(),
            "member-a".into(),
            fixture.authority.clone(),
            MediaLimits::default(),
        )
        .unwrap();
        let retired = reopened.retire(request.clone(), input).await.unwrap();
        assert!(retired.quota_released);
        assert!(reopened.inventory().await.unwrap().is_empty());
        reopened.shutdown().await;
        fixture.stop().await;
    }
}

#[tokio::test]
async fn media_retirement_changed_or_unknown_source_preserves_files_and_quota() {
    let fixture = Fixture::new().await;
    let declaration = fixture.declaration(SUBTITLE).await;
    fixture.media.declare(declaration.clone()).await.unwrap();
    let request = &declaration.request_id;
    let directory = fixture.media.root.join(format!("{request}.source"));
    ledger::write_new(&directory.join("private-unknown"), b"keep me").unwrap();
    let status = fixture.media.retirement_status(request).await.unwrap();
    let input = decision(&status, MediaSourceDisposition::Discard);
    assert!(
        fixture
            .media
            .retire(request.clone(), input.clone())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(directory.join("private-unknown")).unwrap(),
        b"keep me"
    );
    std::fs::remove_file(directory.join("private-unknown")).unwrap();
    ledger::write_new(&directory.join("partial"), b"original").unwrap();
    let _record = planned(&fixture, request, input.clone()).await;
    let retained = directory.join("original-kept");
    std::fs::rename(directory.join("partial"), &retained).unwrap();
    ledger::write_new(&directory.join("partial"), b"original").unwrap();
    std::fs::rename(
        &retained,
        fixture.root.path().join("original-private-source"),
    )
    .unwrap();
    assert!(fixture.media.retire(request.clone(), input).await.is_err());
    assert_eq!(
        std::fs::read(directory.join("partial")).unwrap(),
        b"original"
    );
    assert_eq!(fixture.media.inventory().await.unwrap().len(), 1);
    fixture.stop().await;
}

#[tokio::test]
async fn media_retirement_refuses_hardlinked_and_oversized_sources_before_intent() {
    let fixture = Fixture::new().await;
    let declaration = fixture.declaration(SUBTITLE).await;
    fixture.media.declare(declaration.clone()).await.unwrap();
    let request = &declaration.request_id;
    let directory = fixture.media.root.join(format!("{request}.source"));
    ledger::write_new(&directory.join("partial"), b"original").unwrap();
    std::fs::hard_link(
        directory.join("partial"),
        fixture.root.path().join("other-link"),
    )
    .unwrap();
    let status = fixture.media.retirement_status(request).await.unwrap();
    let input = decision(&status, MediaSourceDisposition::Discard);
    assert!(
        fixture
            .media
            .retire(request.clone(), input.clone())
            .await
            .is_err()
    );
    assert!(
        fixture
            .media
            .load(request)
            .await
            .unwrap()
            .unwrap()
            .retirement
            .is_none()
    );
    std::fs::remove_file(fixture.root.path().join("other-link")).unwrap();
    File::options()
        .write(true)
        .open(directory.join("partial"))
        .unwrap()
        .set_len(SUBTITLE.len() as u64 + 1)
        .unwrap();
    assert!(fixture.media.retire(request.clone(), input).await.is_err());
    assert!(
        fixture
            .media
            .load(request)
            .await
            .unwrap()
            .unwrap()
            .retirement
            .is_none()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn media_retirement_revocation_and_member_isolation_preserve_source() {
    let fixture = Fixture::new().await;
    let declaration = fixture.declaration(SUBTITLE).await;
    fixture.media.declare(declaration.clone()).await.unwrap();
    let status = fixture
        .media
        .retirement_status(&declaration.request_id)
        .await
        .unwrap();
    let input = decision(&status, MediaSourceDisposition::Discard);
    let other = MediaDispatcher::new(
        fixture.runtime.context.clone(),
        "member-b".into(),
        fixture.authority.clone(),
        MediaLimits::default(),
    )
    .unwrap();
    assert_eq!(
        other
            .retirement_status(&declaration.request_id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert!(
        other
            .retire(declaration.request_id.clone(), input.clone())
            .await
            .is_err()
    );
    fixture.authority.revoked.cancel();
    assert_eq!(
        fixture
            .media
            .retire(declaration.request_id.clone(), input)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    assert!(
        fixture
            .media
            .load(&declaration.request_id)
            .await
            .unwrap()
            .unwrap()
            .retirement
            .is_none()
    );
    other.shutdown().await;
    fixture.stop().await;
}

#[tokio::test]
async fn media_retirement_abandoned_source_read_still_blocks_actual_cleanup() {
    let fixture = Fixture::new().await;
    let declaration = successful(&fixture).await;
    let request = &declaration.request_id;
    let status = fixture.media.retirement_status(request).await.unwrap();
    let input = decision(&status, MediaSourceDisposition::Exported);
    let source = fixture
        .media
        .open_source(request, &status.revision)
        .await
        .unwrap();
    let (entered, reached) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    *source.read_pause.lock().unwrap() = Some((entered, wait));
    let reader = tokio::spawn(async move { source.read_chunk_with_guard(0, 8, ()).await });
    reached.await.unwrap();
    reader.abort();
    let _ = reader.await;
    assert_eq!(
        fixture
            .media
            .retirement_status(request)
            .await
            .unwrap()
            .state,
        MediaRetirementState::Blocked
    );
    assert!(
        fixture
            .media
            .retire(request.clone(), input.clone())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while fixture
            .media
            .retirement_status(request)
            .await
            .unwrap()
            .state
            != MediaRetirementState::Eligible
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        fixture
            .media
            .retire(request.clone(), input)
            .await
            .unwrap()
            .quota_released
    );
    fixture.stop().await;
}

#[tokio::test]
async fn media_retirement_abandoned_caller_retains_shutdown_and_session_until_io_finishes() {
    let fixture = Fixture::new().await;
    let declaration = fixture.declaration(SUBTITLE).await;
    fixture.media.declare(declaration.clone()).await.unwrap();
    let request = declaration.request_id;
    let status = fixture.media.retirement_status(&request).await.unwrap();
    let input = decision(&status, MediaSourceDisposition::Discard);
    let (entered, reached) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    *fixture.media.retirement_pause.lock().unwrap() = Some((entered, wait));
    let owner = fixture.media.clone();
    let original = request.clone();
    let caller = tokio::spawn(async move { owner.retire(original, input).await });
    reached.await.unwrap();
    caller.abort();
    let _ = caller.await;
    assert!(
        fixture
            .runtime
            .context
            .try_session_lifecycle_change(&fixture.session)
            .is_err()
    );
    assert!(
        fixture
            .media
            .root
            .join(format!("{request}.admission.json"))
            .is_file()
    );
    let owner = fixture.media.clone();
    let shutdown = tokio::spawn(async move { owner.shutdown().await });
    tokio::task::yield_now().await;
    assert!(!shutdown.is_finished());
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !fixture
            .media
            .root
            .join(format!("{request}.admission.json"))
            .exists()
    );
    assert!(retired_path(&fixture.media.root, &request).is_file());
    assert!(
        fixture
            .runtime
            .context
            .try_session_lifecycle_change(&fixture.session)
            .is_ok()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn media_retirement_archive_budget_and_future_receipts_fail_closed() {
    let fixture = Fixture::new().await;
    let declaration = fixture.declaration(SUBTITLE).await;
    fixture.media.declare(declaration.clone()).await.unwrap();
    let request = &declaration.request_id;
    let status = fixture.media.retirement_status(request).await.unwrap();
    let input = decision(&status, MediaSourceDisposition::Discard);
    let oversized = retired_path(&fixture.media.root, &uuid::Uuid::new_v4().to_string());
    ledger::write_new(&oversized, b"{}").unwrap();
    File::options()
        .write(true)
        .open(&oversized)
        .unwrap()
        .set_len(MAX_TOMBSTONE_BYTES)
        .unwrap();
    assert!(
        fixture
            .media
            .retire(request.clone(), input.clone())
            .await
            .is_err()
    );
    assert!(
        fixture
            .media
            .load(request)
            .await
            .unwrap()
            .unwrap()
            .retirement
            .is_none()
    );
    std::fs::remove_file(oversized).unwrap();
    fixture.media.retire(request.clone(), input).await.unwrap();
    let archive = retired_path(&fixture.media.root, request);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&archive).unwrap()).unwrap();
    value["version"] = serde_json::json!(99);
    std::fs::write(&archive, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(fixture.media.declare(declaration.clone()).await.is_err());
    assert!(fixture.media.retirement_status(request).await.is_err());
    assert!(fixture.media.recover(request).await.is_err());
    assert!(
        !fixture
            .media
            .root
            .join(format!("{request}.admission.json"))
            .exists()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn media_retirement_completed_replay_ignores_later_work_and_preserves_reappeared_bytes() {
    let fixture = Fixture::new().await;
    let declaration = fixture.declaration(SUBTITLE).await;
    fixture.media.declare(declaration.clone()).await.unwrap();
    let request = &declaration.request_id;
    let status = fixture.media.retirement_status(request).await.unwrap();
    let input = decision(&status, MediaSourceDisposition::Discard);
    let retired = fixture
        .media
        .retire(request.clone(), input.clone())
        .await
        .unwrap();
    let later = fixture
        .media
        .root
        .join(format!("{request}.source"))
        .join("source");
    ledger::write_new(&later, b"Later operator-retained content; never touch").unwrap();
    let bytes = std::fs::read(&later).unwrap();
    let session = fixture
        .runtime
        .context
        .admit_session_work(&fixture.session)
        .await
        .unwrap();
    let source = fixture.media.source_gate(request).await.read_owned().await;
    let reader_capacity = fixture.media.source_reads.acquire_many(4).await.unwrap();
    let replay = fixture.media.retire(request.clone(), input).await.unwrap();
    assert_eq!(replay, retired);
    assert_eq!(std::fs::read(&later).unwrap(), bytes);
    assert!(fixture.media.inventory().await.unwrap().is_empty());
    drop(source);
    drop(session);
    drop(reader_capacity);
    fixture.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn media_retirement_refuses_symlinked_source_without_touching_target() {
    let fixture = Fixture::new().await;
    let declaration = fixture.declaration(SUBTITLE).await;
    fixture.media.declare(declaration.clone()).await.unwrap();
    let request = &declaration.request_id;
    let directory = fixture.media.root.join(format!("{request}.source"));
    ledger::ensure_directory(&directory).unwrap();
    let target = fixture.root.path().join("operator-file");
    std::fs::write(&target, b"preserve").unwrap();
    std::os::unix::fs::symlink(&target, directory.join("partial")).unwrap();
    let status = fixture.media.retirement_status(request).await.unwrap();
    assert!(
        fixture
            .media
            .retire(
                request.clone(),
                decision(&status, MediaSourceDisposition::Discard)
            )
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"preserve");
    assert!(
        std::fs::symlink_metadata(directory.join("partial"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        fixture
            .media
            .load(request)
            .await
            .unwrap()
            .unwrap()
            .retirement
            .is_none()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn media_retirement_rechecks_owned_source_identity_after_hash_before_unlink() {
    let fixture = Fixture::new().await;
    let declaration = fixture.declaration(SUBTITLE).await;
    fixture.media.declare(declaration.clone()).await.unwrap();
    let request = &declaration.request_id;
    let directory = fixture.media.root.join(format!("{request}.source"));
    ledger::write_new(&directory.join("partial"), b"original").unwrap();
    let status = fixture.media.retirement_status(request).await.unwrap();
    let input = decision(&status, MediaSourceDisposition::Discard);
    let record = planned(&fixture, request, input).await;
    let prepared = prepare_cleanup(&fixture.media.root, &record).unwrap();
    std::fs::rename(
        directory.join("partial"),
        fixture.root.path().join("retained-original"),
    )
    .unwrap();
    ledger::write_new(&directory.join("partial"), b"original").unwrap();
    assert!(finish_prepared(&fixture.media.root, record, prepared, |_| Ok(())).is_err());
    assert_eq!(
        std::fs::read(directory.join("partial")).unwrap(),
        b"original"
    );
    assert!(!retired_path(&fixture.media.root, request).exists());
    assert_eq!(fixture.media.inventory().await.unwrap().len(), 1);
    fixture.stop().await;
}

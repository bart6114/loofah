use super::*;

#[tokio::test]
async fn startup_recovery_finishes_only_admitted_intents_without_opening_the_managed_store() {
    use hypr_storage::vault::{LeaseAttempt, VaultLease};
    for boundary in [1, 4] {
        let f = Fixture::new().await;
        let lease = match VaultLease::try_acquire_existing(f.store.vault_base()).unwrap() {
            LeaseAttempt::Acquired(lease) => lease,
            _ => panic!("unexpected owner"),
        };
        f.store.pin_vault_lease(&lease).unwrap();
        interrupted(&f, boundary).await;
        f.store.set_startup_pending(true);
        assert!(f.store.recording_publication_recovery().is_err());
        assert!(f.store.ensure_ready().is_err());
        let recovery = f
            .store
            .recording_publication_startup_recovery(&lease.pin())
            .unwrap();
        let mut inventory = recovery
            .discover(&f.operations, "vault", Default::default())
            .await
            .unwrap();
        assert!(inventory.issues.is_empty());
        assert_eq!(inventory.pending.len(), 1);
        let plan = match recovery
            .prepare(&f.operations, "vault", inventory.pending.remove(0))
            .await
            .unwrap()
        {
            RecordingPublicationRecoveryPreparation::Prepared(plan) => plan,
            _ => panic!("missing admitted intent"),
        };
        assert_eq!(
            (*plan).recheck().await.unwrap().commit().await.unwrap(),
            f.request.published()
        );
        assert_eq!(
            std::fs::read(f.directory.join("audio.wav")).unwrap(),
            f.bytes
        );
        assert_eq!(
            std::fs::read(f.directory.join("notes.md")).unwrap(),
            b"User note"
        );
        assert!(f.store.ensure_ready().is_err());
        drop(recovery);
        assert!(f.store.ensure_ready().is_err());
    }
}

#[tokio::test]
async fn startup_recovery_rejects_unpinned_and_different_owner_descriptors() {
    use hypr_storage::vault::{LeaseAttempt, VaultLease};
    let f = Fixture::new().await;
    let lease = match VaultLease::try_acquire_existing(f.store.vault_base()).unwrap() {
        LeaseAttempt::Acquired(lease) => lease,
        _ => panic!("unexpected owner"),
    };
    assert!(
        f.store
            .recording_publication_startup_recovery(&lease.pin())
            .is_err()
    );
    f.store.pin_vault_lease(&lease).unwrap();
    let other = Arc::new(std::fs::File::open(f.directory.join("notes.md")).unwrap());
    assert!(
        f.store
            .recording_publication_startup_recovery(&other)
            .is_err()
    );
    assert!(
        f.store
            .recording_publication_startup_recovery(&lease.pin())
            .is_ok()
    );
}

async fn interrupted(f: &Fixture, boundary: u8) {
    assert!(
        f.prepare()
            .await
            .commit_with(move |at| if at == boundary {
                Err(StoreError::Io("injected interrupted publication".into()))
            } else {
                Ok(())
            })
            .await
            .is_err()
    );
}
async fn inventory(f: &Fixture) -> RecordingPublicationInventory {
    f.store
        .discover_recording_publications(
            &f.operations,
            "vault",
            RecordingPublicationDiscoveryLimits::default(),
        )
        .await
        .unwrap()
}
async fn pending(f: &Fixture) -> PendingRecordingPublication {
    let mut found = inventory(f).await;
    assert!(found.issues.is_empty());
    assert_eq!(found.pending.len(), 1);
    found.pending.remove(0)
}
async fn recover(
    f: &Fixture,
    entry: PendingRecordingPublication,
) -> RecordingPublicationRecoveryPreparation {
    f.store
        .prepare_recording_publication_recovery(&f.operations, "vault", entry)
        .await
        .unwrap()
}
async fn commit_recovery(f: &Fixture, entry: PendingRecordingPublication) -> RecordingPublication {
    match recover(f, entry).await {
        RecordingPublicationRecoveryPreparation::Prepared(plan) => {
            (*plan).recheck().await.unwrap().commit().await.unwrap()
        }
        _ => panic!("expected admitted recovery plan"),
    }
}
fn receipt_path(f: &Fixture) -> PathBuf {
    io::receipt_path(&f.operations, &f.request)
}
fn write_receipt(f: &Fixture, mutate: impl FnOnce(&mut io::Receipt)) {
    let path = receipt_path(f);
    let mut receipt = io::read(&path).unwrap().unwrap();
    mutate(&mut receipt);
    io::persist(&path, &receipt).unwrap();
}

#[tokio::test]
async fn empty_inventory_is_complete_and_does_not_create_private_directories() {
    let f = Fixture::new().await;
    let found = inventory(&f).await;
    assert!(found.pending.is_empty() && found.issues.is_empty());
    assert_eq!(found.scanned_entries, 0);
    assert!(!f.operations.exists());
    std::fs::create_dir_all(f.operations.join("unrelated")).unwrap();
    std::fs::write(
        f.operations
            .join("unrelated/recording-publication-bad.json"),
        b"bad",
    )
    .unwrap();
    std::fs::write(f.operations.join("other.json"), b"bad").unwrap();
    let found = inventory(&f).await;
    assert_eq!(found.scanned_entries, 2);
    assert_eq!(found.scanned_receipts, 0);
    assert!(found.pending.is_empty() && found.issues.is_empty());
}

#[tokio::test]
async fn both_admitted_incomplete_phases_discover_and_recover_without_raw_source() {
    for boundary in [1, 4] {
        let f = Fixture::new().await;
        interrupted(&f, boundary).await;
        let bytes = std::fs::read(receipt_path(&f)).unwrap();
        let entry = pending(&f).await;
        assert_eq!(entry.request(), &f.request);
        assert_eq!(
            entry.phase(),
            if boundary == 1 {
                RecordingPublicationPhase::Pending
            } else {
                RecordingPublicationPhase::MetadataWritten
            }
        );
        assert_eq!(std::fs::read(receipt_path(&f)).unwrap(), bytes);
        assert_eq!(commit_recovery(&f, entry).await, f.request.published());
        assert_eq!(
            std::fs::read(f.directory.join("audio.wav")).unwrap(),
            f.bytes
        );
        let done = inventory(&f).await;
        assert!(done.pending.is_empty() && done.issues.is_empty());
        assert_eq!(done.scanned_receipts, 1);
    }
}

#[tokio::test]
async fn completed_receipts_never_read_or_recreate_deleted_or_replaced_recordings() {
    for delete in [false, true] {
        let f = Fixture::new().await;
        f.prepare().await.commit().await.unwrap();
        if delete {
            f.store.delete_session("note").await.unwrap();
        } else {
            std::fs::write(f.directory.join("audio.wav"), b"replacement").unwrap();
        }
        let found = inventory(&f).await;
        assert!(found.pending.is_empty() && found.issues.is_empty());
        if delete {
            assert!(!f.directory.exists());
            assert!(f.store.session_get("note").is_none());
        } else {
            assert_eq!(
                std::fs::read(f.directory.join("audio.wav")).unwrap(),
                b"replacement"
            );
        }
    }
}

#[tokio::test]
async fn discovery_reports_uncertain_with_trusted_session_and_invalid_without_one() {
    let f = Fixture::new().await;
    interrupted(&f, 1).await;
    write_receipt(&f, |receipt| receipt.state = io::State::Uncertain);
    let found = inventory(&f).await;
    assert!(found.pending.is_empty());
    assert_eq!(found.issues.len(), 1);
    assert_eq!(found.issues[0].session_id.as_deref(), Some("note"));
    assert_eq!(
        found.issues[0].kind,
        RecordingPublicationReviewKind::Uncertain
    );
    std::fs::write(receipt_path(&f), b"{").unwrap();
    let found = inventory(&f).await;
    assert!(found.pending.is_empty());
    assert_eq!(found.issues[0].session_id, None);
    assert_eq!(
        found.issues[0].kind,
        RecordingPublicationReviewKind::InvalidReceipt
    );
}

#[tokio::test]
async fn wrong_vault_filename_and_stage_bindings_fail_closed_without_file_changes() {
    for changed in 0..3 {
        let f = Fixture::new().await;
        interrupted(&f, 1).await;
        let expected = match changed {
            0 => {
                write_receipt(&f, |receipt| receipt.request.vault_id = "other".into());
                RecordingPublicationReviewKind::WrongVault
            }
            1 => {
                let path = receipt_path(&f);
                std::fs::rename(
                    &path,
                    f.operations
                        .join(format!("recording-publication-{}.json", "0".repeat(64))),
                )
                .unwrap();
                RecordingPublicationReviewKind::FilenameBinding
            }
            _ => {
                write_receipt(&f, |receipt| receipt.stage_name = "../../notes.md".into());
                RecordingPublicationReviewKind::InvalidReceipt
            }
        };
        let metadata = std::fs::read(f.directory.join("_meta.json")).unwrap();
        let found = inventory(&f).await;
        assert!(found.pending.is_empty());
        assert_eq!(found.issues.len(), 1);
        assert_eq!(found.issues[0].kind, expected);
        assert_eq!(found.issues[0].session_id, None);
        assert!(!f.directory.join("audio.wav").exists());
        assert_eq!(
            std::fs::read(f.directory.join("_meta.json")).unwrap(),
            metadata
        );
    }
}

#[tokio::test]
async fn duplicate_future_truncated_and_oversized_receipts_are_retained_for_review() {
    for corrupt in 0..5 {
        let f = Fixture::new().await;
        interrupted(&f, 1).await;
        let path = receipt_path(&f);
        let original = std::fs::read_to_string(&path).unwrap();
        let bytes = match corrupt {
            0 => original.replacen('{', "{\"schema_version\":1,", 1),
            1 => original.replace(
                "\"capture_ended_at_ms\":1799995210999",
                "\"capture_ended_at_ms\":null,\"capture_ended_at_ms\":null",
            ),
            2 => original.replace("\"schema_version\":1", "\"schema_version\":2"),
            3 => "{".into(),
            _ => "x".repeat(64 * 1024 + 1),
        };
        assert_ne!(bytes, original);
        std::fs::write(&path, &bytes).unwrap();
        let found = inventory(&f).await;
        assert!(found.pending.is_empty());
        assert_eq!(found.issues.len(), 1);
        assert_eq!(
            found.issues[0].kind,
            if corrupt == 4 {
                RecordingPublicationReviewKind::OversizedReceipt
            } else {
                RecordingPublicationReviewKind::InvalidReceipt
            }
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn malformed_candidate_names_are_review_issues_not_silently_ignored() {
    let f = Fixture::new().await;
    std::fs::create_dir_all(&f.operations).unwrap();
    for name in [
        "recording-publication-nope.json",
        "RECORDING-PUBLICATION-AAAAAAAA.json",
        "recording-publication-abc.json.bak",
    ] {
        std::fs::write(f.operations.join(name), b"unknown").unwrap();
    }
    let found = inventory(&f).await;
    assert_eq!(found.issues.len(), 3);
    assert!(found.pending.is_empty());
    assert!(found.issues.iter().all(|issue| issue.kind
        == RecordingPublicationReviewKind::InvalidFilename
        && issue.session_id.is_none()));
}

#[tokio::test]
async fn scan_entry_byte_and_result_limits_never_return_a_partial_success() {
    let f = Fixture::new().await;
    interrupted(&f, 1).await;
    std::fs::write(f.operations.join("unrelated"), b"x").unwrap();
    std::fs::write(f.operations.join("recording-publication-bad.json"), b"bad").unwrap();
    let before = std::fs::read(receipt_path(&f)).unwrap();
    for kind in 0..3 {
        let mut limits = RecordingPublicationDiscoveryLimits::default();
        match kind {
            0 => limits.max_directory_entries = 1,
            1 => limits.max_total_receipt_bytes = 1,
            _ => limits.max_results = 1,
        };
        assert!(matches!(
            f.store
                .discover_recording_publications(&f.operations, "vault", limits)
                .await,
            Err(StoreError::ResourceLimit(_))
        ));
    }
    assert_eq!(std::fs::read(receipt_path(&f)).unwrap(), before);
    assert!(!f.directory.join("audio.wav").exists());
}

#[tokio::test]
async fn prepared_recovery_pins_current_leases_and_core_controls_recheck_and_commit() {
    let f = Fixture::new().await;
    interrupted(&f, 1).await;
    let entry = pending(&f).await;
    let plan = match recover(&f, entry.clone()).await {
        RecordingPublicationRecoveryPreparation::Prepared(plan) => plan,
        _ => panic!("expected plan"),
    };
    assert!(f.store.lock_session_audio("note").await.is_err());
    assert!(matches!(
        recover(&f, entry).await,
        RecordingPublicationRecoveryPreparation::Busy
    ));
    assert_eq!(
        (*plan).recheck().await.unwrap().commit().await.unwrap(),
        f.request.published()
    );
}

#[tokio::test]
async fn current_same_intent_completion_is_not_replayed_even_after_session_deletion() {
    let f = Fixture::new().await;
    interrupted(&f, 1).await;
    let entry = pending(&f).await;
    f.status().await.commit().await.unwrap();
    f.store.delete_session("note").await.unwrap();
    assert!(matches!(
        recover(&f, entry).await,
        RecordingPublicationRecoveryPreparation::NoLongerPending
    ));
    assert!(!f.directory.exists());
    assert!(f.store.session_get("note").is_none());
}

#[tokio::test]
async fn receipt_disappearance_immutable_change_and_phase_regression_require_review() {
    for change in 0..4 {
        let f = Fixture::new().await;
        interrupted(&f, if change == 3 { 4 } else { 1 }).await;
        let entry = pending(&f).await;
        match change {
            0 => std::fs::remove_file(receipt_path(&f)).unwrap(),
            1 => write_receipt(&f, |r| {
                r.stage_name = format!(".recording-publication-{}.tmp", uuid::Uuid::new_v4())
            }),
            2 => write_receipt(&f, |r| r.request.source_binding.push('x')),
            _ => write_receipt(&f, |r| r.state = io::State::Pending),
        };
        let raw = std::fs::read(f.directory.join("_meta.json")).unwrap();
        let audio = std::fs::read(f.directory.join("audio.wav")).ok();
        assert!(matches!(
            recover(&f, entry).await,
            RecordingPublicationRecoveryPreparation::RequiresReview(_)
        ));
        assert_eq!(std::fs::read(f.directory.join("_meta.json")).unwrap(), raw);
        assert_eq!(std::fs::read(f.directory.join("audio.wav")).ok(), audio);
    }
}

#[tokio::test]
async fn pending_handle_can_advance_to_metadata_written_but_not_cross_store_root_or_vault() {
    let f = Fixture::new().await;
    interrupted(&f, 1).await;
    let entry = pending(&f).await;
    let other_store = SessionStore::new(f.root.path().join("vault"));
    assert!(matches!(
        other_store
            .prepare_recording_publication_recovery(&f.operations, "vault", entry.clone())
            .await
            .unwrap(),
        RecordingPublicationRecoveryPreparation::RequiresReview(
            RecordingPublicationReviewKind::WrongVault
        )
    ));
    assert!(matches!(
        f.store
            .prepare_recording_publication_recovery(&f.operations, "other", entry.clone())
            .await
            .unwrap(),
        RecordingPublicationRecoveryPreparation::RequiresReview(
            RecordingPublicationReviewKind::WrongVault
        )
    ));
    let other = f.root.path().join("other-operations");
    std::fs::create_dir(&other).unwrap();
    assert!(matches!(
        f.store
            .prepare_recording_publication_recovery(&other, "vault", entry.clone())
            .await
            .unwrap(),
        RecordingPublicationRecoveryPreparation::RequiresReview(
            RecordingPublicationReviewKind::ReceiptChanged
        )
    ));
    assert!(
        f.status()
            .await
            .commit_with(|at| if at == 4 {
                Err(StoreError::Io("pause".into()))
            } else {
                Ok(())
            })
            .await
            .is_err()
    );
    assert_eq!(commit_recovery(&f, entry).await, f.request.published());
}

#[tokio::test]
async fn deleted_replaced_or_corrupt_pending_data_never_gets_recreated() {
    for change in 0..3 {
        let f = Fixture::new().await;
        interrupted(&f, 2).await;
        let entry = pending(&f).await;
        match change {
            0 => {
                f.store.delete_session("note").await.unwrap();
            }
            1 => std::fs::write(f.directory.join("audio.wav"), b"replacement").unwrap(),
            _ => {
                let mut raw = f.raw();
                raw["title"] = "changed after intent".into();
                std::fs::write(
                    f.directory.join("_meta.json"),
                    serde_json::to_vec(&raw).unwrap(),
                )
                .unwrap();
            }
        };
        let before = std::fs::read(f.directory.join("audio.wav")).ok();
        match recover(&f, entry).await {
            RecordingPublicationRecoveryPreparation::RequiresReview(_) => {}
            RecordingPublicationRecoveryPreparation::Prepared(plan) => assert_eq!(
                (*plan).recheck().await.unwrap().commit().await.unwrap(),
                RecordingPublication::Uncertain
            ),
            _ => panic!("unsafe recovery outcome"),
        }
        assert_eq!(std::fs::read(f.directory.join("audio.wav")).ok(), before);
    }
}

#[tokio::test]
async fn unclaimed_hidden_files_are_not_discovered_or_cleaned() {
    let f = Fixture::new().await;
    let unknown = f.directory.join(format!(
        ".recording-publication-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&unknown, &f.bytes).unwrap();
    let found = inventory(&f).await;
    assert!(found.pending.is_empty() && found.issues.is_empty());
    assert_eq!(std::fs::read(unknown).unwrap(), f.bytes);
}

#[tokio::test]
async fn transcript_presence_refuses_new_and_retained_publication_but_committed_stt_is_allowed() {
    for boundary in [0, 1, 4, 5] {
        let f = Fixture::new().await;
        if boundary != 0 {
            interrupted(&f, boundary).await;
        }
        std::fs::write(
            f.directory.join("transcript.json"),
            b"user or STT transcript",
        )
        .unwrap();
        let before = std::fs::read(f.directory.join("_meta.json")).unwrap();
        if boundary == 0 {
            assert_eq!(
                f.prepare().await.commit().await.unwrap(),
                RecordingPublication::Changed
            );
        } else if boundary == 5 {
            assert_eq!(
                f.status().await.commit().await.unwrap(),
                f.request.published()
            );
            assert!(inventory(&f).await.pending.is_empty());
        } else {
            let entry = pending(&f).await;
            assert!(matches!(
                recover(&f, entry).await,
                RecordingPublicationRecoveryPreparation::RequiresReview(_)
            ));
        }
        assert_eq!(
            std::fs::read(f.directory.join("transcript.json")).unwrap(),
            b"user or STT transcript"
        );
        assert_eq!(
            std::fs::read(f.directory.join("_meta.json")).unwrap(),
            before
        );
    }
}

#[tokio::test]
async fn transcript_appearing_during_staging_or_after_intent_is_preserved() {
    for after_intent in [false, true] {
        let f = Fixture::new().await;
        let plan = f.prepare().await;
        let path = f.directory.join("transcript.json");
        let outcome = if after_intent {
            plan.commit_with(move |at| {
                if at == 2 {
                    std::fs::write(&path, b"external transcript").map_err(io::io_error)?;
                }
                Ok(())
            })
            .await
            .unwrap()
        } else {
            std::fs::write(path, b"external transcript").unwrap();
            plan.commit().await.unwrap()
        };
        assert_eq!(
            outcome,
            if after_intent {
                RecordingPublication::Uncertain
            } else {
                RecordingPublication::Changed
            }
        );
        assert_eq!(
            std::fs::read(f.directory.join("transcript.json")).unwrap(),
            b"external transcript"
        );
    }
}

#[tokio::test]
async fn inherited_oversize_end_cannot_admit_an_unreadable_receipt() {
    let mut f = Fixture::new().await;
    f.request.capture_ended_at_ms = None;
    let mut raw = f.raw();
    raw["ended_at"] = "x".repeat(129).into();
    let bytes = serde_json::to_vec_pretty(&raw).unwrap();
    std::fs::write(f.directory.join("_meta.json"), &bytes).unwrap();
    assert!(f.prepare().await.commit().await.is_err());
    assert!(!f.operations.exists());
    assert!(!f.directory.join("audio.wav").exists());
    assert_eq!(
        std::fs::read(f.directory.join("_meta.json")).unwrap(),
        bytes
    );
}

#[cfg(unix)]
#[tokio::test]
async fn unsafe_root_receipt_and_dangling_transcript_are_never_followed() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new().await;
    std::fs::create_dir_all(&f.operations).unwrap();
    let outside = f.root.path().join("outside");
    std::fs::write(&outside, b"private").unwrap();
    let receipt = f
        .operations
        .join(format!("recording-publication-{}.json", "a".repeat(64)));
    symlink(&outside, &receipt).unwrap();
    let found = inventory(&f).await;
    assert_eq!(
        found.issues[0].kind,
        RecordingPublicationReviewKind::UnsafeReceipt
    );
    assert_eq!(std::fs::read(&outside).unwrap(), b"private");
    let linked = f.root.path().join("linked-operations");
    symlink(&f.operations, &linked).unwrap();
    assert!(
        f.store
            .discover_recording_publications(
                &linked,
                "vault",
                RecordingPublicationDiscoveryLimits::default()
            )
            .await
            .is_err()
    );
    symlink(
        f.root.path().join("missing"),
        f.directory.join("transcript.json"),
    )
    .unwrap();
    assert_eq!(
        f.prepare().await.commit().await.unwrap(),
        RecordingPublication::Changed
    );
    assert!(
        std::fs::symlink_metadata(f.directory.join("transcript.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

use std::{
    collections::BTreeMap,
    path::Path,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::*;

struct TestDirectory {
    _temporary: tempfile::TempDir,
    path: PathBuf,
}

impl TestDirectory {
    fn path(&self) -> &Path {
        &self.path
    }
}

fn test_directory() -> TestDirectory {
    let temporary = tempfile::tempdir().unwrap();
    // macOS CI temp paths can traverse the system /var symlink; the host passes
    // a physical app-private parent rather than a symlinked storage namespace.
    let path = temporary.path().canonicalize().unwrap();
    TestDirectory {
        _temporary: temporary,
        path,
    }
}

fn scope() -> CaptureScope {
    CaptureScope {
        server: "https://company.example/".into(),
        member_id: "member".into(),
        vault_id: "vault".into(),
    }
}
fn identity() -> CaptureIdentity {
    CaptureIdentity {
        scope: scope(),
        capture_id: uuid::Uuid::from_u128(1).to_string(),
        request_id: uuid::Uuid::from_u128(2).to_string(),
        target: CaptureTarget::ExistingSession {
            session_id: "note".into(),
            expected_audio_sha256: None,
        },
    }
}
fn pcm(frames: u32) -> Vec<u8> {
    (0..frames)
        .flat_map(|frame| {
            [
                (frame as f32 / 100_000.0).to_le_bytes(),
                (-0.25f32).to_le_bytes(),
            ]
            .concat()
        })
        .collect()
}
fn dir(root: &Path, id: &CaptureIdentity) -> PathBuf {
    root.join(hash(serde_json::to_vec(&id.scope).unwrap()))
        .join(&id.capture_id)
}
fn receipt(snapshot: &CaptureSnapshot) -> CapturePrefixReceipt {
    CapturePrefixReceipt {
        capture_fingerprint: snapshot.capture_fingerprint.clone(),
        chunk_count: snapshot.chunk_count,
        frames: snapshot.frames,
        pcm_bytes: snapshot.pcm_bytes,
        chain_digest: snapshot.chain_digest.clone(),
    }
}
fn publication(snapshot: &CaptureSnapshot) -> CapturePublication {
    CapturePublication {
        source: snapshot.seal.clone().unwrap(),
        session_id: snapshot.session_id.clone().unwrap(),
        canonical_audio_sha256: hash(b"independently verified encoded server artifact"),
        canonical_audio_bytes: 1234,
    }
}
async fn open(root: &Path) -> Arc<CaptureSpool> {
    CaptureSpool::open(root.into(), CaptureSpoolLimits::default())
        .await
        .unwrap()
}

#[tokio::test]
async fn immutable_chunks_reopen_with_exact_bytes_channels_offsets_and_source_digest() {
    let temp = test_directory();
    let root = temp.path().join("spool");
    let id = identity();
    let spool = open(&root).await;
    let writer = spool
        .begin(id.clone(), CaptureFormat::stereo())
        .await
        .unwrap();
    let first = pcm(160);
    let second = pcm(80);
    let one = writer.append_chunk(0, 160, first.clone()).await.unwrap();
    let two = writer.append_chunk(1, 80, second.clone()).await.unwrap();
    assert_eq!(
        (one.first_frame, one.pcm_offset, one.frames, one.bytes),
        (0, 0, 160, 1280)
    );
    assert_eq!((two.first_frame, two.pcm_offset), (160, 1280));
    assert_eq!(two.previous_digest, one.digest);
    let seal = writer.seal().await.unwrap();
    assert_eq!(
        (seal.frames, seal.chunk_count, seal.pcm_bytes),
        (240, 2, 1920)
    );
    assert_eq!(
        seal.source_sha256,
        hash([first.clone(), second.clone()].concat())
    );
    assert_eq!(
        writer.append_chunk(0, 160, first.clone()).await.unwrap(),
        one
    );
    assert_eq!(
        writer
            .append_chunk(2, 160, first.clone())
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Conflict
    );
    let snapshot = writer.snapshot().await.unwrap();
    drop(writer);
    drop(spool);
    let spool = open(&root).await;
    let writer = spool.resume(id).await.unwrap();
    assert_eq!(writer.snapshot().await.unwrap(), snapshot);
    assert_eq!(writer.read_chunk(0).await.unwrap().bytes, first);
    assert_eq!(writer.read_chunk(1).await.unwrap().bytes, second);
    assert_eq!(writer.seal().await.unwrap(), seal);
    assert_eq!(writer.chunks(None, 1).await.unwrap(), vec![one]);
    assert_eq!(writer.chunks(Some(0), 200).await.unwrap(), vec![two]);
    assert!(writer.chunks(Some(u64::MAX), 200).await.unwrap().is_empty());
}

#[tokio::test]
async fn pending_session_binding_preserves_chunk_identity_and_cannot_redirect_a_capture() {
    let temp = test_directory();
    let spool = open(&temp.path().join("spool")).await;
    let mut id = identity();
    id.target = CaptureTarget::PendingSession {
        create_request_id: uuid::Uuid::from_u128(3).to_string(),
    };
    let writer = spool
        .begin(id.clone(), CaptureFormat::stereo())
        .await
        .unwrap();
    let descriptor = writer.append_chunk(0, 4, pcm(4)).await.unwrap();
    let before = writer.snapshot().await.unwrap();
    assert!(before.session_id.is_none());
    let after = writer
        .bind_session("admitted-session".into())
        .await
        .unwrap();
    assert_eq!(after.identity, id);
    assert_eq!(after.capture_fingerprint, before.capture_fingerprint);
    assert_eq!(after.chain_digest, before.chain_digest);
    assert_eq!(
        writer
            .bind_session("admitted-session".into())
            .await
            .unwrap(),
        after
    );
    assert_eq!(
        writer
            .bind_session("another-session".into())
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(writer.read_chunk(0).await.unwrap().descriptor, descriptor);
    assert_eq!(
        writer
            .bind_session("../escape".into())
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Validation
    );
    let mut changed = id;
    changed.target = CaptureTarget::ExistingSession {
        session_id: "admitted-session".into(),
        expected_audio_sha256: None,
    };
    assert_eq!(
        spool.resume(changed).await.err().unwrap().code,
        ErrorCode::Conflict
    );
}

#[tokio::test]
async fn capture_session_coordinates_use_the_canonical_portable_vault_rules() {
    let temp = test_directory();
    let spool = open(&temp.path().join("spool")).await;
    for session in [
        "",
        "CON",
        "con.txt",
        "PRN",
        "AUX",
        "NUL",
        "COM1",
        "LPT9.txt",
        "note:stream",
        "note.",
        "note ",
        "bad?name",
        "../escape",
        "path\\name",
        ".hidden",
        "bad\0name",
    ] {
        assert!(
            hypr_loofah_protocol::validate_session_id(session).is_err(),
            "{session}"
        );
        let mut id = identity();
        id.target = CaptureTarget::ExistingSession {
            session_id: session.into(),
            expected_audio_sha256: None,
        };
        assert_eq!(
            spool
                .begin(id, CaptureFormat::stereo())
                .await
                .err()
                .unwrap()
                .code,
            ErrorCode::Validation,
            "{session}"
        );
    }
    assert!(spool.inventory(scope()).await.unwrap().is_empty());
    let mut id = identity();
    id.target = CaptureTarget::PendingSession {
        create_request_id: uuid::Uuid::new_v4().to_string(),
    };
    let writer = spool.begin(id, CaptureFormat::stereo()).await.unwrap();
    for session in ["CON", "note:stream", "note.", "note "] {
        assert_eq!(
            writer
                .bind_session(session.into())
                .await
                .err()
                .unwrap()
                .code,
            ErrorCode::Validation
        );
    }
    assert!(writer.snapshot().await.unwrap().session_id.is_none());
    assert_eq!(
        writer
            .bind_session("Windows-safe-id_123".into())
            .await
            .unwrap()
            .session_id
            .as_deref(),
        Some("Windows-safe-id_123")
    );
}

#[tokio::test]
async fn noncanonical_scope_and_capture_aliases_are_retained_and_never_reused_for_new_audio() {
    for coordinate in ["scope", "capture"] {
        let temp = test_directory();
        let root = temp.path().join("spool");
        let spool = open(&root).await;
        let mut id = identity();
        id.capture_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".into();
        let writer = spool
            .begin(id.clone(), CaptureFormat::stereo())
            .await
            .unwrap();
        writer.append_chunk(0, 8, pcm(8)).await.unwrap();
        drop(writer);
        drop(spool);
        let original = if coordinate == "scope" {
            dir(&root, &id).parent().unwrap().to_path_buf()
        } else {
            dir(&root, &id)
        };
        let alias = original.with_file_name(
            original
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_ascii_uppercase(),
        );
        let temporary = original.with_file_name("case-transition");
        std::fs::rename(&original, &temporary).unwrap();
        std::fs::rename(&temporary, &alias).unwrap();
        let capture = if coordinate == "scope" {
            alias.join(&id.capture_id)
        } else {
            alias.clone()
        };
        let path = capture.join("chunks/00000000000000000000.chunk");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            CaptureSpool::open(root, CaptureSpoolLimits::default())
                .await
                .err()
                .unwrap()
                .code,
            ErrorCode::Internal,
            "{coordinate}"
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn invalid_pcm_declarations_and_changed_replays_never_replace_audio() {
    let temp = test_directory();
    let spool = open(&temp.path().join("spool")).await;
    let writer = spool
        .begin(identity(), CaptureFormat::stereo())
        .await
        .unwrap();
    let original = pcm(8);
    let descriptor = writer.append_chunk(0, 8, original.clone()).await.unwrap();
    for (sequence, frames, bytes, code) in [
        (0, 8, pcm(9), ErrorCode::Validation),
        (0, 8, vec![0; 64], ErrorCode::Conflict),
        (2, 8, pcm(8), ErrorCode::Conflict),
        (1, 0, vec![], ErrorCode::Validation),
        (
            1,
            1,
            [f32::NAN.to_le_bytes(), 0f32.to_le_bytes()].concat(),
            ErrorCode::Validation,
        ),
        (
            1,
            1,
            [f32::INFINITY.to_le_bytes(), 0f32.to_le_bytes()].concat(),
            ErrorCode::Validation,
        ),
    ] {
        assert_eq!(
            writer
                .append_chunk(sequence, frames, bytes)
                .await
                .err()
                .unwrap()
                .code,
            code
        );
    }
    assert_eq!(writer.snapshot().await.unwrap().chunk_count, 1);
    assert_eq!(writer.read_chunk(0).await.unwrap().descriptor, descriptor);
    assert_eq!(writer.read_chunk(0).await.unwrap().bytes, original);
    assert_eq!(
        writer.chunks(None, 0).await.err().unwrap().code,
        ErrorCode::Validation
    );
    assert_eq!(
        writer.read_chunk(u64::MAX).await.err().unwrap().code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn receipts_and_publication_are_bound_monotone_and_never_remove_audio() {
    let temp = test_directory();
    let root = temp.path().join("spool");
    let spool = open(&root).await;
    let id = identity();
    let writer = spool
        .begin(id.clone(), CaptureFormat::stereo())
        .await
        .unwrap();
    writer.append_chunk(0, 4, pcm(4)).await.unwrap();
    let first = receipt(&writer.snapshot().await.unwrap());
    writer.append_chunk(1, 4, pcm(4)).await.unwrap();
    writer.seal().await.unwrap();
    let all = receipt(&writer.snapshot().await.unwrap());
    let observation = publication(&writer.snapshot().await.unwrap());
    assert_eq!(
        writer
            .observe_publication(observation.clone())
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Conflict
    );
    writer.acknowledge_prefix(all.clone()).await.unwrap();
    assert_eq!(
        writer.acknowledge_prefix(first).await.unwrap().acknowledged,
        Some(all.clone())
    );
    for wrong in [
        CapturePrefixReceipt {
            capture_fingerprint: hash(b"another account"),
            ..all.clone()
        },
        CapturePrefixReceipt {
            frames: 123,
            ..all.clone()
        },
        CapturePrefixReceipt {
            chunk_count: 3,
            ..all.clone()
        },
    ] {
        assert_eq!(
            writer.acknowledge_prefix(wrong).await.err().unwrap().code,
            ErrorCode::Conflict
        );
    }
    let mut wrong = observation.clone();
    wrong.source.source_sha256 = hash(b"different recording");
    assert_eq!(
        writer.observe_publication(wrong).await.err().unwrap().code,
        ErrorCode::Conflict
    );
    let mut wrong = observation.clone();
    wrong.session_id = "other-session".into();
    assert_eq!(
        writer.observe_publication(wrong).await.err().unwrap().code,
        ErrorCode::Conflict
    );
    let published = writer
        .observe_publication(observation.clone())
        .await
        .unwrap();
    assert_eq!(published.publication, Some(observation.clone()));
    let mut wrong = observation;
    wrong.canonical_audio_sha256 = hash(b"replacement artifact");
    assert_eq!(
        writer.observe_publication(wrong).await.err().unwrap().code,
        ErrorCode::Conflict
    );
    for sequence in 0..2 {
        assert!(
            dir(&root, &id)
                .join("chunks")
                .join(format!("{sequence:020}.chunk"))
                .is_file()
        );
        assert_eq!(writer.read_chunk(sequence).await.unwrap().bytes, pcm(4));
    }
    drop(writer);
    drop(spool);
    let spool = open(&root).await;
    assert_eq!(
        spool.resume(id).await.unwrap().snapshot().await.unwrap(),
        published
    );
}

#[tokio::test]
async fn scopes_capture_ids_and_request_ids_cannot_be_rebound_or_cross_read() {
    let temp = test_directory();
    let spool = open(&temp.path().join("spool")).await;
    let id = identity();
    let writer = spool
        .begin(id.clone(), CaptureFormat::stereo())
        .await
        .unwrap();
    writer.append_chunk(0, 1, pcm(1)).await.unwrap();
    let mut changed = id.clone();
    changed.request_id = uuid::Uuid::from_u128(9).to_string();
    assert_eq!(
        spool.resume(changed).await.err().unwrap().code,
        ErrorCode::Conflict
    );
    let mut changed = id.clone();
    changed.capture_id = uuid::Uuid::from_u128(9).to_string();
    assert_eq!(
        spool
            .begin(changed, CaptureFormat::stereo())
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Conflict
    );
    let mut other = id.clone();
    other.scope.member_id = "another-member".into();
    assert_eq!(
        spool.resume(other.clone()).await.err().unwrap().code,
        ErrorCode::NotFound
    );
    let second = spool
        .begin(other.clone(), CaptureFormat::stereo())
        .await
        .unwrap();
    assert_eq!(second.snapshot().await.unwrap().chunk_count, 0);
    assert_eq!(spool.inventory(id.scope.clone()).await.unwrap().len(), 1);
    assert_eq!(spool.inventory(other.scope.clone()).await.unwrap().len(), 1);
    assert_ne!(
        writer.snapshot().await.unwrap().capture_fingerprint,
        second.snapshot().await.unwrap().capture_fingerprint
    );
    let mut changed = CaptureFormat::stereo();
    changed.sample_rate = 48_000;
    assert_eq!(
        spool.begin(id.clone(), changed).await.err().unwrap().code,
        ErrorCode::Validation
    );
    let mono = CaptureFormat {
        encoding: CaptureEncoding::PcmF32Le,
        sample_rate: 16_000,
        channels: vec![CaptureChannel::Microphone],
    };
    assert_eq!(
        spool.begin(id, mono).await.err().unwrap().code,
        ErrorCode::Conflict
    );
}

#[tokio::test]
async fn quotas_and_injected_storage_full_keep_all_previously_accepted_chunks() {
    let temp = test_directory();
    let root = temp.path().join("spool");
    let limits = CaptureSpoolLimits {
        max_chunks: 2,
        max_captures: 1,
        ..CaptureSpoolLimits::default()
    };
    let spool = CaptureSpool::open(root.clone(), limits).await.unwrap();
    let writer = spool
        .begin(identity(), CaptureFormat::stereo())
        .await
        .unwrap();
    writer.append_chunk(0, 8, pcm(8)).await.unwrap();
    spool
        .fault
        .store(Fault::BeforeChunkWrite as u8, Ordering::SeqCst);
    let error = writer.append_chunk(1, 8, pcm(8)).await.err().unwrap();
    assert_eq!(error.code, ErrorCode::ResourceLimit);
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
    assert_eq!(writer.snapshot().await.unwrap().chunk_count, 1);
    writer.append_chunk(1, 8, pcm(8)).await.unwrap();
    assert_eq!(
        writer.append_chunk(2, 8, pcm(8)).await.err().unwrap().code,
        ErrorCode::ResourceLimit
    );
    let mut second = identity();
    second.capture_id = uuid::Uuid::new_v4().to_string();
    second.request_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        spool
            .begin(second, CaptureFormat::stereo())
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::ResourceLimit
    );
    for sequence in 0..2 {
        assert_eq!(writer.read_chunk(sequence).await.unwrap().bytes, pcm(8));
    }
    drop(writer);
    drop(spool);
    assert_eq!(
        CaptureSpool::open(
            root,
            CaptureSpoolLimits {
                max_directory_entries: 1,
                ..CaptureSpoolLimits::default()
            }
        )
        .await
        .err()
        .unwrap()
        .code,
        ErrorCode::ResourceLimit
    );
    let byte_root = temp.path().join("byte-limited");
    let spool = CaptureSpool::open(
        byte_root,
        CaptureSpoolLimits {
            max_bytes: 64 * 1024,
            ..CaptureSpoolLimits::default()
        },
    )
    .await
    .unwrap();
    let writer = spool
        .begin(identity(), CaptureFormat::stereo())
        .await
        .unwrap();
    assert_eq!(
        writer
            .append_chunk(0, 16_000, pcm(16_000))
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::ResourceLimit
    );
    assert_eq!(writer.snapshot().await.unwrap().chunk_count, 0);
}

#[tokio::test]
async fn directory_quotas_are_enforced_before_accepting_data_that_cannot_reopen() {
    let temp = test_directory();
    let root = temp.path().join("spool");
    let limits = CaptureSpoolLimits {
        max_directory_entries: 7,
        ..CaptureSpoolLimits::default()
    };
    let spool = CaptureSpool::open(root.clone(), limits.clone())
        .await
        .unwrap();
    let writer = spool
        .begin(identity(), CaptureFormat::stereo())
        .await
        .unwrap();
    writer.append_chunk(0, 8, pcm(8)).await.unwrap();
    assert_eq!(
        writer.append_chunk(1, 8, pcm(8)).await.err().unwrap().code,
        ErrorCode::ResourceLimit
    );
    let before = writer.snapshot().await.unwrap();
    drop(writer);
    drop(spool);
    let spool = CaptureSpool::open(root, limits).await.unwrap();
    let writer = spool.resume(identity()).await.unwrap();
    assert_eq!(writer.snapshot().await.unwrap(), before);
    assert_eq!(writer.read_chunk(0).await.unwrap().bytes, pcm(8));
    let root = temp.path().join("cannot-begin");
    let spool = CaptureSpool::open(
        root.clone(),
        CaptureSpoolLimits {
            max_directory_entries: 4,
            ..CaptureSpoolLimits::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        spool
            .begin(identity(), CaptureFormat::stereo())
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::ResourceLimit
    );
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
}

fn disk_bytes(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                disk_bytes(&entry.path())
            } else {
                meta.len()
            }
        })
        .sum()
}

#[tokio::test]
async fn metadata_quota_failures_restore_cached_state_and_leave_the_previous_manifest_canonical() {
    for operation in ["bind", "ack", "seal", "publication"] {
        let temp = test_directory();
        let root = temp.path().join("spool");
        let spool = open(&root).await;
        let mut id = identity();
        if operation == "bind" {
            id.target = CaptureTarget::PendingSession {
                create_request_id: uuid::Uuid::new_v4().to_string(),
            };
        }
        let writer = spool
            .begin(id.clone(), CaptureFormat::stereo())
            .await
            .unwrap();
        writer.append_chunk(0, 8, pcm(8)).await.unwrap();
        if operation == "publication" {
            writer.seal().await.unwrap();
            writer
                .acknowledge_prefix(receipt(&writer.snapshot().await.unwrap()))
                .await
                .unwrap();
        }
        let before = writer.snapshot().await.unwrap();
        drop(writer);
        drop(spool);
        let limits = CaptureSpoolLimits {
            max_bytes: disk_bytes(&root) + 1,
            ..CaptureSpoolLimits::default()
        };
        let spool = CaptureSpool::open(root.clone(), limits.clone())
            .await
            .unwrap();
        let writer = spool.resume(id.clone()).await.unwrap();
        let error = match operation {
            "bind" => writer
                .bind_session("server-session".into())
                .await
                .err()
                .unwrap(),
            "ack" => writer
                .acknowledge_prefix(receipt(&before))
                .await
                .err()
                .unwrap(),
            "seal" => writer.seal().await.err().unwrap(),
            "publication" => writer
                .observe_publication(publication(&before))
                .await
                .err()
                .unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(error.code, ErrorCode::ResourceLimit);
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
        assert_eq!(writer.snapshot().await.unwrap(), before, "{operation}");
        drop(writer);
        drop(spool);
        let spool = CaptureSpool::open(root, limits).await.unwrap();
        assert_eq!(
            spool.resume(id).await.unwrap().snapshot().await.unwrap(),
            before,
            "{operation}"
        );
    }
}

#[tokio::test]
async fn mono_channels_empty_seals_and_float_bit_patterns_are_explicit() {
    let temp = test_directory();
    let spool = open(&temp.path().join("spool")).await;
    for channel in [
        CaptureChannel::Microphone,
        CaptureChannel::SystemAudio,
        CaptureChannel::Mixed,
    ] {
        let mut id = identity();
        id.capture_id = uuid::Uuid::new_v4().to_string();
        id.request_id = uuid::Uuid::new_v4().to_string();
        let format = CaptureFormat {
            encoding: CaptureEncoding::PcmF32Le,
            sample_rate: 16_000,
            channels: vec![channel],
        };
        let writer = spool.begin(id, format).await.unwrap();
        let bytes = [
            (-0.0f32).to_le_bytes(),
            f32::MIN_POSITIVE.to_le_bytes(),
            (-1e-20f32).to_le_bytes(),
        ]
        .concat();
        writer.append_chunk(0, 3, bytes.clone()).await.unwrap();
        assert_eq!(writer.read_chunk(0).await.unwrap().bytes, bytes);
        assert_eq!(writer.seal().await.unwrap().frames, 3);
    }
    let writer = spool
        .begin(identity(), CaptureFormat::stereo())
        .await
        .unwrap();
    let seal = writer.seal().await.unwrap();
    assert_eq!(seal.chunk_count, 0);
    assert_eq!(seal.frames, 0);
    assert_eq!(seal.source_sha256, hash([]));
    assert_eq!(
        writer.append_chunk(0, 1, pcm(1)).await.err().unwrap().code,
        ErrorCode::Conflict
    );
}

#[tokio::test]
async fn chunk_corruption_during_ownership_is_detected_before_bytes_leave_the_spool() {
    let temp = test_directory();
    let root = temp.path().join("spool");
    let spool = open(&root).await;
    let writer = spool
        .begin(identity(), CaptureFormat::stereo())
        .await
        .unwrap();
    writer.append_chunk(0, 8, pcm(8)).await.unwrap();
    let chunk = dir(&root, &identity()).join("chunks/00000000000000000000.chunk");
    let mut corrupt = std::fs::read(&chunk).unwrap();
    *corrupt.last_mut().unwrap() ^= 1;
    std::fs::write(&chunk, &corrupt).unwrap();
    assert_eq!(
        writer.read_chunk(0).await.err().unwrap().code,
        ErrorCode::Internal
    );
    assert_eq!(std::fs::read(chunk).unwrap(), corrupt);
}

#[tokio::test]
async fn all_write_fault_boundaries_recover_once_and_reconfirm_before_replay() {
    let temp = test_directory();
    let root = temp.path().join("spool");
    let spool = open(&root).await;
    let id = identity();
    let writer = spool
        .begin(id.clone(), CaptureFormat::stereo())
        .await
        .unwrap();
    let mut source = vec![];
    for (sequence, point) in [
        Fault::AfterChunkFileSync,
        Fault::AfterChunkRename,
        Fault::AfterChunkDirectorySync,
        Fault::AfterManifestRename,
        Fault::AfterManifestDirectorySync,
    ]
    .into_iter()
    .enumerate()
    {
        let bytes = pcm(sequence as u32 + 1);
        source.extend(&bytes);
        spool.fault.store(point as u8, Ordering::SeqCst);
        assert_eq!(
            writer
                .append_chunk(sequence as u64, sequence as u32 + 1, bytes.clone())
                .await
                .err()
                .unwrap()
                .mutation_outcome,
            Some(MutationOutcome::Unknown)
        );
        let resumed = spool.resume(id.clone()).await.unwrap();
        assert_eq!(
            resumed.snapshot().await.unwrap().chunk_count,
            sequence as u64 + 1
        );
        let replay = resumed
            .append_chunk(sequence as u64, sequence as u32 + 1, bytes.clone())
            .await
            .unwrap();
        assert_eq!(
            resumed
                .read_chunk(sequence as u64)
                .await
                .unwrap()
                .descriptor,
            replay
        );
        assert_eq!(
            resumed.read_chunk(sequence as u64).await.unwrap().bytes,
            bytes
        );
    }
    assert_eq!(writer.seal().await.unwrap().source_sha256, hash(source));
    assert_eq!(
        std::fs::read_dir(dir(&root, &id).join("chunks"))
            .unwrap()
            .count(),
        5
    );
}

#[tokio::test]
async fn abandoned_callers_retain_the_root_lease_and_bounded_io_until_disk_work_finishes() {
    let temp = test_directory();
    let root = temp.path().join("spool");
    let spool = open(&root).await;
    let writer = spool
        .begin(identity(), CaptureFormat::stereo())
        .await
        .unwrap();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let release = Arc::new(std::sync::Barrier::new(2));
    *spool.pause.lock().unwrap() = Some(TestPause {
        point: Fault::AfterChunkFileSync as u8,
        entered,
        release: release.clone(),
    });
    let first = writer.clone();
    let first = tokio::spawn(async move { first.append_chunk(0, 8, pcm(8)).await });
    waiting.await.unwrap();
    let second = writer.clone();
    let second = tokio::spawn(async move { second.append_chunk(1, 8, pcm(8)).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while spool.io.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        writer.snapshot().await.err().unwrap().code,
        ErrorCode::Unavailable
    );
    first.abort();
    second.abort();
    let _ = first.await;
    let _ = second.await;
    drop(writer);
    drop(spool);
    assert_eq!(
        CaptureSpool::open(root.clone(), CaptureSpoolLimits::default())
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Unavailable
    );
    tokio::task::spawn_blocking(move || release.wait())
        .await
        .unwrap();
    let reopened = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match CaptureSpool::open(root.clone(), CaptureSpoolLimits::default()).await {
                Ok(spool) => break spool,
                Err(error) if error.code == ErrorCode::Unavailable => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        reopened
            .resume(identity())
            .await
            .unwrap()
            .snapshot()
            .await
            .unwrap()
            .chunk_count,
        2
    );
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "subprocess crash fixture"]
fn capture_crash_child() {
    let root = PathBuf::from(std::env::var_os("LOOFAH_CAPTURE_CRASH_ROOT").unwrap());
    let point: u8 = std::env::var("LOOFAH_CAPTURE_CRASH_POINT")
        .unwrap()
        .parse()
        .unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let spool = open(&root).await;
            let writer = spool
                .begin(identity(), CaptureFormat::stereo())
                .await
                .unwrap();
            spool.fault.store(point, Ordering::SeqCst);
            writer.append_chunk(0, 16, pcm(16)).await.unwrap();
            std::fs::write(
                std::env::var_os("LOOFAH_CAPTURE_CRASH_MARKER").unwrap(),
                b"accepted",
            )
            .unwrap();
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
}

#[tokio::test]
async fn killed_processes_recover_synced_pending_renamed_and_accepted_chunks_without_duplication() {
    for point in [
        Fault::AfterChunkFileSync as u8,
        Fault::AfterChunkRename as u8,
        Fault::AfterManifestRename as u8,
        0,
    ] {
        let temp = test_directory();
        let root = temp.path().join("spool");
        let marker = temp.path().join("paused");
        let mut child = Child(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "capture_spool::tests::capture_crash_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("LOOFAH_CAPTURE_CRASH_ROOT", &root)
                .env("LOOFAH_CAPTURE_CRASH_POINT", point.to_string())
                .env("LOOFAH_CAPTURE_CRASH_MARKER", &marker)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            while !marker.exists() {
                assert!(child.0.try_wait().unwrap().is_none());
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(child.0.try_wait().unwrap().is_none());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let spool = open(&root).await;
        let writer = spool.resume(identity()).await.unwrap();
        assert_eq!(writer.snapshot().await.unwrap().chunk_count, 1);
        let replay = writer.append_chunk(0, 16, pcm(16)).await.unwrap();
        assert_eq!(writer.read_chunk(0).await.unwrap().descriptor, replay);
        assert_eq!(writer.read_chunk(0).await.unwrap().bytes, pcm(16));
        assert_eq!(
            std::fs::read_dir(dir(&root, &identity()).join("chunks"))
                .unwrap()
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn corrupt_future_incomplete_and_foreign_recovery_files_are_retained_without_resetting_identity()
 {
    for mode in [
        "future-manifest",
        "broken-manifest",
        "future-chunk",
        "broken-chunk",
        "incomplete-pending",
        "foreign-header",
    ] {
        let temp = test_directory();
        let root = temp.path().join("spool");
        let spool = open(&root).await;
        let id = identity();
        let writer = spool
            .begin(id.clone(), CaptureFormat::stereo())
            .await
            .unwrap();
        writer.append_chunk(0, 8, pcm(8)).await.unwrap();
        drop(writer);
        drop(spool);
        let directory = dir(&root, &id);
        let manifest = directory.join("manifest.json");
        let chunk = directory.join("chunks/00000000000000000000.chunk");
        let target = match mode {
            "future-manifest" => {
                let mut value: Value =
                    serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
                value["schema_version"] = json!(2);
                std::fs::write(&manifest, value.to_string()).unwrap();
                manifest.clone()
            }
            "broken-manifest" => {
                std::fs::write(&manifest, b"{not json").unwrap();
                manifest.clone()
            }
            "incomplete-pending" => {
                let path = directory.join("chunks/00000000000000000001.pending");
                std::fs::write(&path, b"LFCAP001partial").unwrap();
                path
            }
            "broken-chunk" => {
                let mut bytes = std::fs::read(&chunk).unwrap();
                *bytes.last_mut().unwrap() ^= 1;
                std::fs::write(&chunk, bytes).unwrap();
                chunk.clone()
            }
            _ => {
                let bytes = std::fs::read(&chunk).unwrap();
                let header_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
                let mut header: Value =
                    serde_json::from_slice(&bytes[12..12 + header_len]).unwrap();
                if mode == "future-chunk" {
                    header["schema_version"] = json!(2);
                } else {
                    header["descriptor"]["capture_fingerprint"] = json!(hash(b"another vault"));
                }
                let header = serde_json::to_vec(&header).unwrap();
                let mut output = b"LFCAP001".to_vec();
                output.extend((header.len() as u32).to_le_bytes());
                output.extend(header);
                output.extend(&bytes[12 + header_len..]);
                std::fs::write(&chunk, output).unwrap();
                chunk.clone()
            }
        };
        let original = std::fs::read(&target).unwrap();
        assert_eq!(
            CaptureSpool::open(root, CaptureSpoolLimits::default())
                .await
                .err()
                .unwrap()
                .code,
            ErrorCode::Internal,
            "{mode}"
        );
        assert_eq!(std::fs::read(target).unwrap(), original, "{mode}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_cannot_redirect_the_owner_manifest_or_chunk_and_private_modes_are_kept() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = test_directory();
    let root = temp.path().join("spool");
    let spool = open(&root).await;
    let id = identity();
    let writer = spool
        .begin(id.clone(), CaptureFormat::stereo())
        .await
        .unwrap();
    writer.append_chunk(0, 8, pcm(8)).await.unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for path in [
        root.join(".lock"),
        dir(&root, &id).join("manifest.json"),
        dir(&root, &id).join("chunks/00000000000000000000.chunk"),
    ] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(writer);
    drop(spool);
    for path in [
        root.join(".lock"),
        dir(&root, &id).join("manifest.json"),
        dir(&root, &id).join("chunks/00000000000000000000.chunk"),
    ] {
        let outside = temp.path().join("outside");
        let original = std::fs::read(&path).unwrap();
        std::fs::rename(&path, &outside).unwrap();
        symlink(&outside, &path).unwrap();
        assert_eq!(
            CaptureSpool::open(root.clone(), CaptureSpoolLimits::default())
                .await
                .err()
                .unwrap()
                .code,
            ErrorCode::Internal
        );
        assert!(path.is_symlink());
        assert_eq!(std::fs::read(&outside).unwrap(), original);
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(&outside, &path).unwrap();
    }
    let alias = temp.path().join("alias");
    symlink(&root, &alias).unwrap();
    assert_eq!(
        CaptureSpool::open(alias, CaptureSpoolLimits::default())
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Internal
    );
}

#[derive(Default)]
struct FakeSender {
    chunks: BTreeMap<u64, CaptureChunkDescriptor>,
    bytes: BTreeMap<u64, Vec<u8>>,
    attempts: usize,
}
impl FakeSender {
    fn send(&mut self, chunk: CaptureChunk, lose_reply: bool) -> Option<CapturePrefixReceipt> {
        self.attempts += 1;
        if let Some(existing) = self.chunks.get(&chunk.descriptor.sequence) {
            assert_eq!(existing, &chunk.descriptor);
            assert_eq!(self.bytes[&chunk.descriptor.sequence], chunk.bytes);
        } else {
            self.bytes.insert(chunk.descriptor.sequence, chunk.bytes);
            self.chunks
                .insert(chunk.descriptor.sequence, chunk.descriptor.clone());
        }
        if lose_reply {
            return None;
        }
        let last = self.chunks.last_key_value().unwrap().1;
        Some(CapturePrefixReceipt {
            capture_fingerprint: last.capture_fingerprint.clone(),
            chunk_count: last.sequence + 1,
            frames: last.first_frame + last.frames as u64,
            pcm_bytes: last.pcm_offset + last.bytes as u64,
            chain_digest: last.digest.clone(),
        })
    }
}

#[tokio::test]
async fn fake_sender_lost_ack_disconnect_and_device_restart_replay_identical_chunks_only() {
    let temp = test_directory();
    let root = temp.path().join("spool");
    let spool = open(&root).await;
    let writer = spool
        .begin(identity(), CaptureFormat::stereo())
        .await
        .unwrap();
    for sequence in 0..8 {
        writer.append_chunk(sequence, 8, pcm(8)).await.unwrap();
    }
    writer.seal().await.unwrap();
    let mut sender = FakeSender::default();
    let ack = sender
        .send(writer.read_chunk(0).await.unwrap(), false)
        .unwrap();
    writer.acknowledge_prefix(ack).await.unwrap();
    assert!(
        sender
            .send(writer.read_chunk(1).await.unwrap(), true)
            .is_none()
    );
    drop(writer);
    drop(spool);
    let spool = open(&root).await;
    assert_eq!(spool.inventory(scope()).await.unwrap().len(), 1);
    assert_eq!(sender.attempts, 2);
    let writer = spool.resume(identity()).await.unwrap();
    let acknowledged = writer.snapshot().await.unwrap().acknowledged.unwrap();
    let remaining = writer
        .chunks(Some(acknowledged.chunk_count - 1), 200)
        .await
        .unwrap();
    for chunk in remaining {
        let ack = sender
            .send(writer.read_chunk(chunk.sequence).await.unwrap(), false)
            .unwrap();
        writer.acknowledge_prefix(ack).await.unwrap();
    }
    assert_eq!(sender.chunks.len(), 8);
    assert_eq!(sender.attempts, 9);
    let complete = writer.snapshot().await.unwrap();
    writer
        .observe_publication(publication(&complete))
        .await
        .unwrap();
    for sequence in 0..8 {
        assert_eq!(
            writer.read_chunk(sequence).await.unwrap().bytes,
            sender.bytes[&sequence]
        );
    }
    assert_eq!(
        std::fs::read_dir(dir(&root, &identity()).join("chunks"))
            .unwrap()
            .count(),
        8
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "two-hour synthetic storage stress; run explicitly on T7"]
async fn two_hour_dual_channel_synthetic_spool_is_bounded_and_reopens_without_missing_frames() {
    let temp = test_directory();
    let root = std::env::var_os("LOOFAH_CAPTURE_STRESS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| temp.path().to_path_buf())
        .join(uuid::Uuid::new_v4().to_string());
    assert!(
        root.starts_with("/Volumes/T7"),
        "The stress fixture must run on T7"
    );
    let started = Instant::now();
    let spool = open(&root).await;
    let writer = spool
        .begin(identity(), CaptureFormat::stereo())
        .await
        .unwrap();
    let initial = std::fs::metadata(dir(&root, &identity()).join("manifest.json"))
        .unwrap()
        .len();
    let mut largest = initial;
    let template = pcm(16_000);
    let mut expected = Sha256::new();
    println!("capture stress root: {}", root.display());
    for sequence in 0..7200u64 {
        let mut bytes = template.clone();
        bytes[..4].copy_from_slice(&(sequence as f32).to_le_bytes());
        expected.update(&bytes);
        writer.append_chunk(sequence, 16_000, bytes).await.unwrap();
        largest = largest.max(
            std::fs::metadata(dir(&root, &identity()).join("manifest.json"))
                .unwrap()
                .len(),
        );
        if (sequence + 1) % 1800 == 0 {
            println!(
                "persisted {} synthetic seconds; elapsed {:?}; manifest bytes {}",
                sequence + 1,
                started.elapsed(),
                largest
            );
        }
    }
    assert!(
        largest < initial + 256,
        "Manifest must not grow with the chunk list"
    );
    let seal = writer.seal().await.unwrap();
    let expected = expected
        .finalize()
        .iter()
        .map(|c| format!("{c:02x}"))
        .collect::<String>();
    assert_eq!(seal.source_sha256, expected);
    assert_eq!(seal.frames, 115_200_000);
    assert_eq!(seal.pcm_bytes, 921_600_000);
    writer
        .acknowledge_prefix(receipt(&writer.snapshot().await.unwrap()))
        .await
        .unwrap();
    let published = writer
        .observe_publication(publication(&writer.snapshot().await.unwrap()))
        .await
        .unwrap();
    drop(writer);
    drop(spool);
    let spool = open(&root).await;
    let writer = spool.resume(identity()).await.unwrap();
    assert_eq!(writer.snapshot().await.unwrap(), published);
    assert_eq!(writer.seal().await.unwrap(), seal);
    let mut count = 0;
    let mut after = None;
    loop {
        let page = writer.chunks(after, 200).await.unwrap();
        if page.is_empty() {
            break;
        }
        count += page.len();
        after = page.last().map(|chunk| chunk.sequence);
    }
    assert_eq!(count, 7200);
    assert_eq!(
        std::fs::read_dir(dir(&root, &identity()).join("chunks"))
            .unwrap()
            .count(),
        7200
    );
    println!(
        "two-hour synthetic capture verified: frames={}, bytes={}, chunks={}, source_sha256={}, largest_manifest={}, elapsed={:?}",
        seal.frames,
        seal.pcm_bytes,
        seal.chunk_count,
        seal.source_sha256,
        largest,
        started.elapsed()
    );
}

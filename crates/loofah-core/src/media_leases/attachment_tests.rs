use super::tests::fixture;
use super::*;

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[tokio::test]
async fn attachment_leases_pin_exact_files_and_share_lifetimes_without_becoming_recordings() {
    let (root, context) = fixture().await;
    let saved = context
        .store
        .save_attachment(
            "note",
            "diagram + # ü.png",
            b"original image bytes".to_vec(),
        )
        .await
        .unwrap();
    let original_note = std::fs::read(root.path().join("sessions/note/notes.md")).unwrap();
    let leases = MediaLeases::default();
    let recording_id = id();
    leases
        .open(context.clone(), "alice", &recording_id, "note")
        .await
        .unwrap()
        .unwrap();
    context.store.note_recording_active("note");
    let attachment_id = id();
    let opened = leases
        .open_attachment(
            context.clone(),
            "alice",
            &attachment_id,
            "note",
            &saved.attachment_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(opened.metadata.filename, saved.attachment_id);
    assert_eq!(opened.metadata.content_type, "image/png");
    assert_eq!(opened.metadata.sha256, sha(b"original image bytes"));
    assert_eq!(
        leases
            .open_attachment(
                context.clone(),
                "alice",
                &attachment_id,
                "note",
                &saved.attachment_id
            )
            .await
            .unwrap(),
        Some(opened)
    );
    assert!(leases.reader("alice", "vault", &attachment_id).is_err());
    assert!(
        leases
            .attachment_reader("alice", "vault", &recording_id)
            .is_err()
    );
    assert!(
        leases
            .attachment_reader("bob", "vault", &attachment_id)
            .is_err()
    );
    assert!(
        leases
            .attachment_reader("alice", "other", &attachment_id)
            .is_err()
    );
    context
        .store
        .save_attachment("note", "other.png", b"other image".to_vec())
        .await
        .unwrap();
    assert_eq!(
        leases
            .open_attachment(
                context.clone(),
                "alice",
                &attachment_id,
                "note",
                "other.png"
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        leases
            .open(context.clone(), "alice", &attachment_id, "note")
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert!(leases.close("alice", "vault", &attachment_id).is_err());
    let reader = leases
        .attachment_reader("alice", "vault", &attachment_id)
        .unwrap();
    assert_eq!(
        reader
            .waveform(tokio_util::sync::CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    let path = root.path().join(saved.relative_path);
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    std::fs::write(path, b"replacement image").unwrap();
    assert_eq!(reader.read_chunk(2, 8).await.unwrap(), b"iginal i");
    leases.close_recording_session("vault", "note");
    assert!(leases.reader("alice", "vault", &recording_id).is_err());
    assert_eq!(reader.read_chunk(0, 8).await.unwrap(), b"original");
    leases.close_owner("alice");
    assert_eq!(
        reader.read_chunk(0, 1).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        std::fs::read(root.path().join("sessions/note/notes.md")).unwrap(),
        original_note
    );
}

#[tokio::test]
async fn attachment_paths_do_not_fall_back_to_canonical_notes_unknown_flat_files_or_hidden_files() {
    let (root, context) = fixture().await;
    let directory = root.path().join("sessions/note");
    std::fs::write(directory.join("unknown-user.txt"), b"private unknown file").unwrap();
    context
        .store
        .save_attachment("note", "report.PDF", b"pdf attachment".to_vec())
        .await
        .unwrap();
    std::fs::write(directory.join("attachments/.private"), b"hidden attachment").unwrap();
    let leases = MediaLeases::default();
    for filename in [
        "",
        ".private",
        "../notes.md",
        "nested/report.PDF",
        "a\\b",
        "a\0b",
    ] {
        assert_eq!(
            leases
                .open_attachment(context.clone(), "alice", &id(), "note", filename)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }
    for filename in ["notes.md", "unknown-user.txt", "missing"] {
        assert!(
            leases
                .open_attachment(context.clone(), "alice", &id(), "note", filename)
                .await
                .unwrap()
                .is_none()
        );
    }
    let opened = leases
        .open_attachment(context, "alice", &id(), "note", "report.PDF")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(opened.metadata.content_type, "application/pdf");
    assert_eq!(opened.metadata.size_bytes, 14);
    assert_eq!(leases.0.slots.available_permits(), 63);
}

#[tokio::test]
async fn attachments_and_recordings_share_descriptor_and_hash_capacity() {
    let (_root, context) = fixture().await;
    context
        .store
        .save_attachment("note", "a.bin", vec![1; 128 * 1024 + 1])
        .await
        .unwrap();
    context
        .store
        .save_attachment("note", "empty.bin", vec![])
        .await
        .unwrap();
    let leases = MediaLeases::new(2, 1, Duration::from_secs(60));
    let audio_id = id();
    leases
        .open(context.clone(), "alice", &audio_id, "note")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        leases
            .open_attachment(context.clone(), "alice", &id(), "note", "a.bin")
            .await
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimit
    );
    let bob = leases
        .open_attachment(context.clone(), "bob", &id(), "note", "empty.bin")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bob.metadata.size_bytes, 0);
    assert_eq!(bob.metadata.content_type, "application/octet-stream");
    assert_eq!(bob.metadata.sha256, sha(b""));
    leases.close("alice", "vault", &audio_id).unwrap();
    assert!(
        leases
            .open_attachment(context.clone(), "alice", &id(), "note", "a.bin")
            .await
            .unwrap()
            .is_some()
    );
    let bounded = MediaLeases::with_index_limits(4, 4, Duration::from_secs(60), 64, 64);
    let refused = bounded
        .open_attachment(context.clone(), "alice", &id(), "note", "a.bin")
        .await
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::ResourceLimit);
    assert!(!refused.retryable);
    assert_eq!(bounded.0.index_budget.available_permits(), 64);
    assert_eq!(bounded.0.slots.available_permits(), 4);
}

#[tokio::test]
async fn attachment_chunks_refuse_in_place_edits_even_when_size_and_mtime_are_restored() {
    use std::io::Write;
    let (root, context) = fixture().await;
    let saved = context
        .store
        .save_attachment("note", "a.bin", vec![7; 128 * 1024])
        .await
        .unwrap();
    let leases = MediaLeases::default();
    let lease_id = id();
    leases
        .open_attachment(context, "alice", &lease_id, "note", &saved.attachment_id)
        .await
        .unwrap()
        .unwrap();
    let reader = leases
        .attachment_reader("alice", "vault", &lease_id)
        .unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(root.path().join(saved.relative_path))
        .unwrap();
    let times = file.metadata().unwrap();
    file.seek(SeekFrom::Start(64 * 1024)).unwrap();
    file.write_all(&[9]).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(times.modified().unwrap()))
        .unwrap();
    assert_eq!(
        reader.read_chunk(64 * 1024, 1).await.unwrap_err().code,
        ErrorCode::Unavailable
    );
    assert_eq!(
        reader.read_chunk(0, 1).await.unwrap_err().code,
        ErrorCode::NotFound
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_attachment_files_and_directories_are_not_opened() {
    let (root, context) = fixture().await;
    let directory = root.path().join("sessions/note/attachments");
    std::fs::create_dir(&directory).unwrap();
    let secret = root.path().join("secret");
    std::fs::write(&secret, b"not an attachment").unwrap();
    std::os::unix::fs::symlink(&secret, directory.join("alias.txt")).unwrap();
    let leases = MediaLeases::default();
    assert!(
        leases
            .open_attachment(context.clone(), "alice", &id(), "note", "alias.txt")
            .await
            .is_err()
    );
    std::fs::rename(&directory, root.path().join("retired-attachments")).unwrap();
    std::os::unix::fs::symlink(root.path().join("retired-attachments"), &directory).unwrap();
    assert!(
        leases
            .open_attachment(context, "alice", &id(), "note", "alias.txt")
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(secret).unwrap(), b"not an attachment");
    assert_eq!(leases.0.slots.available_permits(), 64);
}

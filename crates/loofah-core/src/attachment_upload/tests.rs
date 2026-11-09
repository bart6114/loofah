use super::*;
use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
use hypr_vault_write::SessionStore;
use sha2::{Digest, Sha256};
use std::{
    io::{self, Cursor, Read},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

fn input(bytes: &[u8]) -> UploadAttachmentRequest {
    UploadAttachmentRequest {
        filename: "chart + ü.pdf".into(),
        size_bytes: bytes.len() as u64,
        sha256: Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    }
}
async fn fixture() -> (tempfile::TempDir, tempfile::TempDir, Arc<VaultContext>) {
    let vault = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(vault.path().into()));
    let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Upload","created_at":"2026-11-06T10:00:00Z","tags":[]})).unwrap();
    store
        .create_note_session(&meta, "Canonical note")
        .await
        .unwrap();
    let context = Arc::new(
        VaultContext::new(
            "vault".into(),
            store,
            StartupState::new(Some(vault.path())),
            Arc::new(ConfigState::load_or_default(vault.path())),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(runtime.path().into()),
    );
    (vault, runtime, context)
}
struct MustNotRead;
impl Read for MustNotRead {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("This operation must not consume input bytes");
    }
}

#[tokio::test]
async fn local_uploads_are_login_free_and_completed_replays_and_status_never_upload_bytes_again() {
    let (vault, _runtime, context) = fixture().await;
    let id = uuid::Uuid::new_v4().to_string();
    let original = input(b"payload");
    assert_eq!(
        context
            .attachment_upload_status("desktop".into(), id.clone(), "note", original.clone())
            .await
            .unwrap(),
        AttachmentUploadStatus::Missing
    );
    let saved = context
        .upload_attachment(
            "desktop".into(),
            id.clone(),
            "note",
            original.clone(),
            Cursor::new(b"payload".to_vec()),
        )
        .await
        .unwrap();
    assert_eq!(saved.filename, original.filename);
    assert_eq!(saved.content_type, "application/pdf");
    assert_eq!(saved.size_bytes, original.size_bytes);
    assert_eq!(saved.sha256, original.sha256);
    assert_eq!(
        serde_json::to_value(&saved)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        context
            .upload_attachment(
                "desktop".into(),
                id.clone(),
                "note",
                original.clone(),
                MustNotRead
            )
            .await
            .unwrap(),
        saved
    );
    context.store.delete_session("note").await.unwrap();
    assert_eq!(
        context
            .attachment_upload_status("desktop".into(), id.clone(), "note", original.clone())
            .await
            .unwrap(),
        AttachmentUploadStatus::Committed(saved.clone())
    );
    assert_eq!(
        context
            .upload_attachment("desktop".into(), id, "note", original, MustNotRead)
            .await
            .unwrap(),
        saved
    );
    assert!(!vault.path().join("sessions/note").exists());
}

#[tokio::test]
async fn authorization_is_rechecked_after_copying_and_for_existing_receipts() {
    let (vault, _runtime, context) = fixture().await;
    let id = uuid::Uuid::new_v4().to_string();
    let denied = || async { Err::<(), _>(error(ErrorCode::Forbidden, "Access removed")) };
    let cause = context
        .upload_attachment_authorized(
            "alice".into(),
            id.clone(),
            "note",
            input(b"payload"),
            Cursor::new(b"payload".to_vec()),
            denied,
        )
        .await
        .unwrap_err();
    assert_eq!(cause.code, ErrorCode::Forbidden);
    assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert!(
        std::fs::read_dir(vault.path().join("sessions/note/attachments"))
            .unwrap()
            .next()
            .is_none()
    );
    context
        .upload_attachment(
            "alice".into(),
            id.clone(),
            "note",
            input(b"payload"),
            Cursor::new(b"payload".to_vec()),
        )
        .await
        .unwrap();
    assert_eq!(
        context
            .attachment_upload_status_authorized(
                "alice".into(),
                id.clone(),
                "note",
                input(b"payload"),
                denied
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    assert_eq!(
        context
            .upload_attachment_authorized(
                "alice".into(),
                id,
                "note",
                input(b"payload"),
                MustNotRead,
                denied
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    assert_eq!(
        std::fs::read_dir(vault.path().join("sessions/note/attachments"))
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn invalid_inputs_and_missing_sessions_are_rejected_before_source_reads_and_wrong_bytes_never_publish()
 {
    let (vault, _runtime, context) = fixture().await;
    let id = uuid::Uuid::new_v4().to_string();
    let mut request = input(b"payload");
    request.filename = "../notes.md".into();
    assert_eq!(
        context
            .upload_attachment("alice".into(), id.clone(), "note", request, MustNotRead)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(
        context
            .upload_attachment(
                "alice".into(),
                "not-uuid".into(),
                "note",
                input(b"payload"),
                MustNotRead
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(
        context
            .upload_attachment(
                "alice".into(),
                id.clone(),
                "missing",
                input(b"payload"),
                MustNotRead
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let cause = context
        .upload_attachment(
            "alice".into(),
            id.clone(),
            "note",
            input(b"payload"),
            Cursor::new(b"changed".to_vec()),
        )
        .await
        .unwrap_err();
    assert_eq!(cause.code, ErrorCode::Conflict);
    assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert_eq!(
        context
            .attachment_upload_status("alice".into(), id.clone(), "note", input(b"payload"))
            .await
            .unwrap(),
        AttachmentUploadStatus::Missing
    );
    context
        .upload_attachment(
            "alice".into(),
            id.clone(),
            "note",
            input(b"payload"),
            Cursor::new(b"payload".to_vec()),
        )
        .await
        .unwrap();
    assert_eq!(
        context
            .upload_attachment("alice".into(), id, "note", input(b"changed"), MustNotRead)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        std::fs::read(vault.path().join("sessions/note/notes.md")).unwrap(),
        b"Canonical note"
    );
}

struct PausedReader {
    entered: Option<tokio::sync::oneshot::Sender<()>>,
    release: std::sync::mpsc::Receiver<()>,
    source: Cursor<Vec<u8>>,
}
impl Read for PausedReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if let Some(entered) = self.entered.take() {
            let _ = entered.send(());
            self.release.recv().unwrap();
        }
        self.source.read(bytes)
    }
}

#[tokio::test]
async fn copying_leaves_note_reads_responsive_and_revocation_and_cancellation_prevent_late_publication()
 {
    let (vault, _runtime, context) = fixture().await;
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, paused) = std::sync::mpsc::channel();
    let allowed = Arc::new(AtomicBool::new(true));
    let auth = allowed.clone();
    let upload = context.clone();
    let task = tokio::spawn(async move {
        upload
            .upload_attachment_authorized(
                "alice".into(),
                uuid::Uuid::new_v4().to_string(),
                "note",
                input(b"payload"),
                PausedReader {
                    entered: Some(entered),
                    release: paused,
                    source: Cursor::new(b"payload".to_vec()),
                },
                move || async move {
                    if auth.load(Ordering::SeqCst) {
                        Ok(())
                    } else {
                        Err(error(ErrorCode::Forbidden, "Access removed"))
                    }
                },
            )
            .await
    });
    started.await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), context.read_note("note"))
            .await
            .unwrap()
            .unwrap()
            .markdown
            .as_deref(),
        Some("Canonical note")
    );
    task.abort();
    assert_eq!(context.upload_writers.available_permits(), 1);
    allowed.store(false, Ordering::SeqCst);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while context.upload_writers.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        std::fs::read_dir(vault.path().join("sessions/note/attachments"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(context.store.freeze_for_vault_move().await.is_ok());
}

struct InfiniteReader(Arc<AtomicUsize>);
impl Read for InfiniteReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        bytes.fill(b'x');
        self.0.fetch_add(bytes.len(), Ordering::SeqCst);
        Ok(bytes.len())
    }
}

#[tokio::test]
async fn empty_attachments_publish_and_keep_the_empty_file_checksum() {
    let (vault, _runtime, context) = fixture().await;
    let request = input(b"");
    let metadata = context
        .upload_attachment(
            "desktop".into(),
            uuid::Uuid::new_v4().to_string(),
            "note",
            request.clone(),
            Cursor::new(Vec::<u8>::new()),
        )
        .await
        .unwrap();
    assert_eq!(metadata.sha256, request.sha256);
    assert_eq!(metadata.size_bytes, 0);
    assert_eq!(
        std::fs::metadata(
            vault
                .path()
                .join("sessions/note/attachments")
                .join(metadata.filename)
        )
        .unwrap()
        .len(),
        0
    );
}

#[tokio::test]
async fn copying_is_bounded_by_the_declared_length_plus_one_verification_byte() {
    let (vault, _runtime, context) = fixture().await;
    let read = Arc::new(AtomicUsize::new(0));
    assert_eq!(
        context
            .upload_attachment(
                "alice".into(),
                uuid::Uuid::new_v4().to_string(),
                "note",
                input(b""),
                InfiniteReader(read.clone())
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(read.load(Ordering::SeqCst), 1);
    assert!(
        std::fs::read_dir(vault.path().join("sessions/note/attachments"))
            .unwrap()
            .next()
            .is_none()
    );
}

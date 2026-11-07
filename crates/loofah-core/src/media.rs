use std::sync::Arc;

use hypr_loofah_protocol::{ApplicationError, ErrorCode, FileInfo, FileMetadata};
use hypr_vault_write::media::RecordingSnapshot;

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MediaSource {
    Recording,
    Attachment(String),
}

pub(crate) struct OpenedMedia {
    pub filename: String,
    pub content_type: String,
    pub file: std::fs::File,
}

impl VaultContext {
    pub async fn attachment_page(
        self: &Arc<Self>,
        session_id: &str,
        limit: usize,
        after: Option<&str>,
    ) -> Result<hypr_loofah_protocol::AttachmentPage, ApplicationError> {
        self.validate(session_id)?;
        if !(1..=200).contains(&limit)
            || after.is_some_and(|cursor| {
                hypr_loofah_protocol::validate_attachment_basename(cursor).is_err()
            })
        {
            return Err(error(
                ErrorCode::Validation,
                "Invalid attachment page size or cursor",
            ));
        }
        let permit = self
            .media_readers
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                error(
                    ErrorCode::Unavailable,
                    "Media readers are busy; retry shortly",
                )
            })?;
        let context = self.clone();
        let id = session_id.to_owned();
        let after = after.map(str::to_owned);
        tokio::spawn(async move {
            let _permit = permit;
            let entries = context
                .store
                .list_attachments(&id, limit, after.as_deref())
                .await
                .map_err(|cause| storage_error(cause, false))?
                .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))?;
            let next_cursor = (entries.len() > limit).then(|| entries[limit - 1].filename.clone());
            let files = entries
                .into_iter()
                .take(limit)
                .map(|entry| FileInfo {
                    content_type: mime_guess::from_path(&entry.filename)
                        .first_or_octet_stream()
                        .essence_str()
                        .into(),
                    filename: entry.filename,
                    size_bytes: entry.size_bytes,
                })
                .collect();
            Ok(hypr_loofah_protocol::AttachmentPage { files, next_cursor })
        })
        .await
        .map_err(|_| error(ErrorCode::Internal, "Attachment listing request stopped"))?
    }

    pub async fn attachment_info(
        self: &Arc<Self>,
        session_id: &str,
        attachment_id: &str,
    ) -> Result<Option<FileInfo>, ApplicationError> {
        self.read_media(
            session_id,
            MediaSource::Attachment(attachment_id.to_owned()),
            |source| {
                let metadata = source.file.metadata().map_err(media_error)?;
                Ok(FileInfo {
                    filename: source.filename,
                    content_type: source.content_type,
                    size_bytes: metadata.len(),
                })
            },
        )
        .await
    }

    pub async fn delete_recording(
        self: &Arc<Self>,
        session_id: &str,
        input: hypr_loofah_protocol::DeleteRecordingRequest,
    ) -> Result<bool, ApplicationError> {
        self.delete_recording_authorized(session_id, input, || async { Ok(()) })
            .await
    }

    // Hosts recheck authorization after long hashing and retain their grant only through commit.
    pub async fn delete_recording_authorized<
        G: Send + 'static,
        F: std::future::Future<Output = Result<G, ApplicationError>> + Send + 'static,
    >(
        self: &Arc<Self>,
        session_id: &str,
        input: hypr_loofah_protocol::DeleteRecordingRequest,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<bool, ApplicationError> {
        use hypr_loofah_protocol::MutationOutcome;
        use hypr_vault_write::{
            StoreError,
            recording_delete::{RecordingDeletion, RecordingDeletionPlan},
        };
        let rejected = |code, message| {
            let mut cause = error(code, message);
            cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
            cause
        };
        self.validate(session_id).map_err(|mut cause| {
            cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
            cause
        })?;
        if input.expected_sha256.len() != 64
            || !input
                .expected_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(rejected(
                ErrorCode::Validation,
                "A lowercase SHA-256 recording revision is required",
            ));
        }
        let permit = self
            .media_readers
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                rejected(
                    ErrorCode::Unavailable,
                    "Recording readers are busy; retry shortly",
                )
            })?;
        let context = self.clone();
        let session_id = session_id.to_owned();
        tokio::spawn(async move {
            let _permit = permit;
            let prepared = context
                .store
                .prepare_recording_deletion(&session_id, &input.expected_sha256)
                .await;
            let checked = match prepared {
                Ok(Some(RecordingDeletionPlan::Prepared(commit))) => {
                    (*commit).recheck().await.map(Some)
                }
                Ok(Some(RecordingDeletionPlan::Finished(status))) => {
                    Ok(Some(RecordingDeletionPlan::Finished(status)))
                }
                Ok(None) => Ok(None),
                Err(cause) => Err(cause),
            };
            let _grant = authorize().await.map_err(|mut cause| {
                cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
                cause
            })?;
            let (result, commit_started) = match checked {
                Ok(Some(RecordingDeletionPlan::Prepared(commit))) => {
                    ((*commit).commit().await, true)
                }
                Ok(Some(RecordingDeletionPlan::Finished(status))) => (Ok(Some(status)), false),
                Ok(None) => (Ok(None), false),
                Err(cause) => (Err(cause), false),
            };
            match result {
                Ok(Some(RecordingDeletion::Deleted)) => Ok(true),
                Ok(Some(RecordingDeletion::Absent)) => Ok(false),
                Ok(Some(RecordingDeletion::Busy)) | Err(StoreError::Conflict(_)) => Err(rejected(
                    ErrorCode::Unavailable,
                    "Recording or import is busy; retry when it finishes",
                )),
                Ok(Some(RecordingDeletion::Changed)) => Err(rejected(
                    ErrorCode::Conflict,
                    "The recording changed; reload it before deleting",
                )),
                Ok(None) => Err(rejected(ErrorCode::NotFound, "Session not found")),
                Err(cause) => {
                    let mut cause = storage_error(cause, commit_started);
                    if !commit_started {
                        cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
                    }
                    Err(cause)
                }
            }
        })
        .await
        .map_err(|cause| {
            tracing::error!(%cause,"recording deletion stopped");
            let mut cause = error(ErrorCode::Internal, "Recording deletion stopped");
            cause.mutation_outcome = Some(MutationOutcome::Unknown);
            cause
        })?
    }

    pub async fn audio_metadata(
        self: &Arc<Self>,
        session_id: &str,
    ) -> Result<Option<FileMetadata>, ApplicationError> {
        self.read_recording(session_id, |source| {
            media_with_metadata(source, |filename, _, file, _| {
                hypr_fs_sync_core::audio::metadata_from_reader(filename, file)
                    .map_err(media_error)?
                    .map(|metadata| (metadata, ()))
                    .ok_or_else(|| error(ErrorCode::Internal, "Unsupported recording format"))
            })
            .map(|(_, metadata, _, ())| metadata)
        })
        .await
    }

    pub(crate) async fn read_recording<T, F>(
        self: &Arc<Self>,
        session_id: &str,
        read: F,
    ) -> Result<Option<T>, ApplicationError>
    where
        T: Send + 'static,
        F: FnOnce(OpenedMedia) -> Result<T, ApplicationError> + Send + 'static,
    {
        self.read_media(session_id, MediaSource::Recording, read)
            .await
    }

    pub(crate) fn validate_media(
        &self,
        session_id: &str,
        source: &MediaSource,
    ) -> Result<(), ApplicationError> {
        self.validate(session_id)?;
        if let MediaSource::Attachment(id) = source {
            hypr_vault_read::paths::validate_attachment_id(id)
                .map_err(|_| error(ErrorCode::Validation, "Invalid attachment ID"))?;
        }
        Ok(())
    }

    pub(crate) async fn read_media<T, F>(
        self: &Arc<Self>,
        session_id: &str,
        target: MediaSource,
        read: F,
    ) -> Result<Option<T>, ApplicationError>
    where
        T: Send + 'static,
        F: FnOnce(OpenedMedia) -> Result<T, ApplicationError> + Send + 'static,
    {
        self.validate_media(session_id, &target)?;
        let permit = self
            .media_readers
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                error(
                    ErrorCode::Unavailable,
                    "Media readers are busy; retry shortly",
                )
            })?;
        let context = self.clone();
        let id = session_id.to_owned();
        tokio::spawn(async move {
            let source = match target {
                MediaSource::Recording => {
                    let source = context
                        .store
                        .open_recording(&id)
                        .await
                        .map_err(|cause| storage_error(cause, false))?
                        .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))?;
                    match source {
                        RecordingSnapshot::Absent => return Ok(None),
                        RecordingSnapshot::Busy => {
                            return Err(error(
                                ErrorCode::Unavailable,
                                "Recording is not ready for playback",
                            ));
                        }
                        RecordingSnapshot::File(source) => OpenedMedia {
                            content_type: hypr_fs_sync_core::audio::content_type(source.filename)
                                .ok_or_else(|| {
                                    error(ErrorCode::Internal, "Unsupported recording format")
                                })?
                                .into(),
                            filename: source.filename.into(),
                            file: source.file,
                        },
                    }
                }
                MediaSource::Attachment(attachment_id) => {
                    let Some(source) = context
                        .store
                        .open_attachment(&id, &attachment_id)
                        .await
                        .map_err(|cause| storage_error(cause, false))?
                    else {
                        return Ok(None);
                    };
                    OpenedMedia {
                        content_type: mime_guess::from_path(&source.attachment_id)
                            .first_or_octet_stream()
                            .essence_str()
                            .into(),
                        filename: source.attachment_id,
                        file: source.file,
                    }
                }
            };
            // Retain admission and vault ownership through blocking work after caller cancellation.
            tokio::task::spawn_blocking(move || {
                let _context = context;
                let _permit = permit;
                read(source).map(Some)
            })
            .await
            .map_err(|_| error(ErrorCode::Internal, "Media reader stopped"))?
        })
        .await
        .map_err(|_| error(ErrorCode::Internal, "Media request stopped"))?
    }
}

pub(crate) fn media_with_metadata<T>(
    mut source: OpenedMedia,
    read: impl FnOnce(
        &str,
        &str,
        &mut std::fs::File,
        u64,
    ) -> Result<(FileMetadata, T), ApplicationError>,
) -> Result<(OpenedMedia, FileMetadata, std::fs::Metadata, T), ApplicationError> {
    let before = source.file.metadata().map_err(media_error)?;
    let (metadata, extra) = read(
        &source.filename,
        &source.content_type,
        &mut source.file,
        before.len(),
    )?;
    let after = source.file.metadata().map_err(media_error)?;
    if before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || metadata.size_bytes != after.len()
    {
        return Err(error(
            ErrorCode::Unavailable,
            "Media changed while its metadata was read",
        ));
    }
    Ok((source, metadata, after, extra))
}

fn media_error(cause: std::io::Error) -> ApplicationError {
    tracing::error!(%cause, "recording metadata read failed");
    error(ErrorCode::Internal, "Cannot read recording metadata")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
    use hypr_vault_write::SessionStore;

    async fn fixture() -> (tempfile::TempDir, Arc<VaultContext>) {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(root.path().into()));
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Title","created_at":"2026-11-06T10:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "Notes remain unchanged")
            .await
            .unwrap();
        let context = Arc::new(VaultContext::new(
            "vault".into(),
            store,
            StartupState::new(Some(root.path())),
            Arc::new(ConfigState::load_or_default(root.path())),
            Arc::new(SearchService::default()),
        ));
        (root, context)
    }

    #[tokio::test]
    async fn attachment_pages_validate_cursors_keep_admission_and_never_hash_file_bytes() {
        let (root, context) = fixture().await;
        let dir = root.path().join("sessions/note/attachments");
        std::fs::create_dir(&dir).unwrap();
        let file = std::fs::File::create(dir.join("large.pdf")).unwrap();
        file.set_len(3 * 1024 * 1024 * 1024).unwrap();
        let page = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            context.attachment_page("note", 1, None),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            page.files,
            vec![FileInfo {
                filename: "large.pdf".into(),
                content_type: "application/pdf".into(),
                size_bytes: 3 * 1024 * 1024 * 1024
            }]
        );
        assert!(page.next_cursor.is_none());
        for (limit, after) in [(0, None), (201, None), (1, Some("../private"))] {
            assert_eq!(
                context
                    .attachment_page("note", limit, after)
                    .await
                    .unwrap_err()
                    .code,
                ErrorCode::Validation
            );
        }
        assert_eq!(
            context
                .attachment_page("missing", 1, None)
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        let permit = context
            .media_readers
            .clone()
            .acquire_many_owned(4)
            .await
            .unwrap();
        assert_eq!(
            context
                .attachment_page("note", 1, None)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Unavailable
        );
        drop(permit);
        assert!(context.attachment_page("note", 1, None).await.is_ok());
        assert_eq!(
            std::fs::read_to_string(root.path().join("sessions/note/notes.md")).unwrap(),
            "Notes remain unchanged"
        );
    }

    #[tokio::test]
    async fn recording_deletion_validates_revisions_and_reports_busy_stale_missing_and_idempotent_outcomes()
     {
        use hypr_loofah_protocol::{DeleteRecordingRequest, MutationOutcome};
        let (root, context) = fixture().await;
        let directory = root.path().join("sessions/note");
        std::fs::write(directory.join("audio.wav"), b"Original recording").unwrap();
        let sha = context
            .audio_metadata("note")
            .await
            .unwrap()
            .unwrap()
            .sha256;
        for revision in ["".into(), "0".repeat(63), "G".repeat(64), "A".repeat(64)] {
            let cause = context
                .delete_recording(
                    "note",
                    DeleteRecordingRequest {
                        expected_sha256: revision,
                    },
                )
                .await
                .unwrap_err();
            assert_eq!(cause.code, ErrorCode::Validation);
            assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
        }
        let input = DeleteRecordingRequest {
            expected_sha256: sha,
        };
        assert_eq!(
            context
                .delete_recording("missing", input.clone())
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        context.store.note_recording_active("note");
        let busy = context
            .delete_recording("note", input.clone())
            .await
            .unwrap_err();
        assert_eq!(busy.code, ErrorCode::Unavailable);
        assert!(busy.retryable);
        context
            .store
            .release_recording_prepare("note")
            .await
            .unwrap();
        std::fs::write(directory.join("audio.wav"), b"Changed recording").unwrap();
        let stale = context.delete_recording("note", input).await.unwrap_err();
        assert_eq!(stale.code, ErrorCode::Conflict);
        assert_eq!(stale.mutation_outcome, Some(MutationOutcome::NotCommitted));
        let current = context.audio_metadata("note").await.unwrap().unwrap();
        let input = DeleteRecordingRequest {
            expected_sha256: current.sha256,
        };
        assert!(
            context
                .delete_recording("note", input.clone())
                .await
                .unwrap()
        );
        assert!(!context.delete_recording("note", input).await.unwrap());
        assert_eq!(
            std::fs::read_to_string(directory.join("notes.md")).unwrap(),
            "Notes remain unchanged"
        );
    }

    #[tokio::test]
    async fn deletion_preparation_failure_is_known_not_committed_and_fresh_authorization_still_runs()
     {
        use hypr_loofah_protocol::{DeleteRecordingRequest, MutationOutcome};
        let (root, context) = fixture().await;
        std::fs::create_dir(root.path().join("sessions/note/audio.wav")).unwrap();
        let authorized = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = authorized.clone();
        let cause = context
            .delete_recording_authorized(
                "note",
                DeleteRecordingRequest {
                    expected_sha256: "0".repeat(64),
                },
                move || async move {
                    observed.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
            .unwrap_err();
        assert!(authorized.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(cause.code, ErrorCode::Internal);
        assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
        assert_eq!(
            std::fs::read_to_string(root.path().join("sessions/note/notes.md")).unwrap(),
            "Notes remain unchanged"
        );
    }

    #[tokio::test]
    async fn authorization_refusal_after_byte_checks_preserves_the_recording_and_releases_ownership()
     {
        use hypr_loofah_protocol::{DeleteRecordingRequest, MutationOutcome};
        let (root, context) = fixture().await;
        let path = root.path().join("sessions/note/audio.wav");
        std::fs::write(&path, b"Keep until authorized").unwrap();
        let sha256 = context
            .audio_metadata("note")
            .await
            .unwrap()
            .unwrap()
            .sha256;
        let input = DeleteRecordingRequest {
            expected_sha256: sha256,
        };
        let cause = context
            .delete_recording_authorized("note", input.clone(), || async {
                Err::<(), _>(error(ErrorCode::Forbidden, "Access revoked"))
            })
            .await
            .unwrap_err();
        assert_eq!(cause.code, ErrorCode::Forbidden);
        assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
        assert_eq!(std::fs::read(&path).unwrap(), b"Keep until authorized");
        assert!(context.delete_recording("note", input).await.unwrap());
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn attachment_info_reads_only_explicit_owned_names_without_paths_hashes_or_capture_coupling()
     {
        let (root, context) = fixture().await;
        let before = std::fs::read(root.path().join("sessions/note/notes.md")).unwrap();
        let name = "diagram + # ü.png";
        context
            .store
            .save_attachment("note", name, b"image".to_vec())
            .await
            .unwrap();
        std::fs::write(
            root.path().join("sessions/note/unknown.png"),
            b"Ignore this attachment",
        )
        .unwrap();
        context.store.note_recording_active("note");
        let info = context
            .attachment_info("note", name)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(info.filename, name);
        assert_eq!(info.content_type, "image/png");
        assert_eq!(info.size_bytes, 5);
        let value = serde_json::to_value(&info).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 3);
        assert!(value.get("sha256").is_none() && value.get("path").is_none());
        assert!(
            context
                .attachment_info("missing", name)
                .await
                .unwrap()
                .is_none()
        );
        for id in ["missing.png", "notes.md", "unknown.png"] {
            assert!(context.attachment_info("note", id).await.unwrap().is_none());
        }
        for id in ["", ".private", "../notes.md", "a/b", "a\\b"] {
            assert_eq!(
                context.attachment_info("note", id).await.unwrap_err().code,
                ErrorCode::Validation
            );
        }
        std::fs::write(
            root.path().join("sessions/note/attachments").join(name),
            b"updated image",
        )
        .unwrap();
        assert_eq!(
            context
                .attachment_info("note", name)
                .await
                .unwrap()
                .unwrap()
                .size_bytes,
            13
        );
        assert_eq!(
            std::fs::read(root.path().join("sessions/note/notes.md")).unwrap(),
            before
        );
        context.store.set_startup_pending(true);
        assert_eq!(
            context
                .attachment_info("note", name)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Unavailable
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn attachment_info_stats_a_large_sparse_file_without_hashing_or_download_limits() {
        let (root, context) = fixture().await;
        context
            .store
            .save_attachment("note", "large.bin", Vec::new())
            .await
            .unwrap();
        let size = 33_u64 * 1024 * 1024 * 1024;
        let path = root.path().join("sessions/note/attachments/large.bin");
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(size)
            .unwrap();
        let info = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            context.attachment_info("note", "large.bin"),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(info.size_bytes, size);
        assert_eq!(info.content_type, "application/octet-stream");
        std::fs::remove_file(&path).unwrap();
        assert!(
            context
                .attachment_info("note", "large.bin")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn shared_metadata_reuses_portable_hashing_and_keeps_paths_out_of_the_result() {
        let (root, context) = fixture().await;
        assert!(context.audio_metadata("note").await.unwrap().is_none());
        assert_eq!(
            context.audio_metadata("missing").await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            context.audio_metadata("../note").await.unwrap_err().code,
            ErrorCode::Validation
        );
        context.store.set_startup_pending(true);
        assert_eq!(
            context.audio_metadata("note").await.unwrap_err().code,
            ErrorCode::Unavailable
        );
        context.store.set_startup_pending(false);
        let directory = root.path().join("sessions/note");
        std::fs::write(directory.join("audio.mp3"), b"Canonical recording bytes").unwrap();
        let metadata = context.audio_metadata("note").await.unwrap();
        assert_eq!(
            metadata,
            hypr_fs_sync_core::audio::metadata(&directory).unwrap()
        );
        let serialized = serde_json::to_string(&metadata).unwrap();
        assert!(!serialized.contains(&root.path().to_string_lossy().to_string()));
        std::fs::write(directory.join("audio.mp3"), b"A different recording").unwrap();
        assert_ne!(context.audio_metadata("note").await.unwrap(), metadata);
        context.store.note_recording_active("note");
        let busy = context.audio_metadata("note").await.unwrap_err();
        assert_eq!(busy.code, ErrorCode::Unavailable);
        assert!(busy.retryable);
        assert_eq!(
            std::fs::read_to_string(directory.join("notes.md")).unwrap(),
            "Notes remain unchanged"
        );
    }

    #[tokio::test]
    async fn cancelled_media_processing_retains_admission_and_ownership_but_releases_the_write_lock()
     {
        let (root, context) = fixture().await;
        std::fs::write(root.path().join("sessions/note/audio.mp3"), b"Recording").unwrap();
        let store = context.store.clone();
        let weak = Arc::downgrade(&context);
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (finish, completed) = std::sync::mpsc::channel();
        let caller = context.clone();
        let request = tokio::spawn(async move {
            caller
                .read_recording("note", move |source| {
                    assert_eq!(source.filename, "audio.mp3");
                    started.send(()).unwrap();
                    completed
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    Ok(())
                })
                .await
        });
        waiting.await.unwrap();
        assert_eq!(context.media_readers.available_permits(), 3);
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(context.media_readers.available_permits(), 3);
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.save_note("note", "Editing is responsive", None),
        )
        .await
        .unwrap()
        .unwrap();
        drop(context);
        assert!(
            weak.upgrade().is_some(),
            "cancelled work lost vault ownership before finishing"
        );
        finish.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn media_admission_is_bounded_and_busy_requests_do_not_queue_background_work() {
        let (root, context) = fixture().await;
        std::fs::write(root.path().join("sessions/note/audio.wav"), b"Recording").unwrap();
        let mut requests = Vec::new();
        let mut finishers = Vec::new();
        for _ in 0..4 {
            let (started, waiting) = tokio::sync::oneshot::channel();
            let (finish, completed) = std::sync::mpsc::channel();
            finishers.push(finish);
            let caller = context.clone();
            requests.push(tokio::spawn(async move {
                caller
                    .read_recording("note", move |_| {
                        started.send(()).unwrap();
                        completed
                            .recv_timeout(std::time::Duration::from_secs(5))
                            .unwrap();
                        Ok(())
                    })
                    .await
            }));
            waiting.await.unwrap();
        }
        assert_eq!(context.media_readers.available_permits(), 0);
        let busy = context.audio_metadata("note").await.unwrap_err();
        assert_eq!(busy.code, ErrorCode::Unavailable);
        assert!(busy.retryable);
        for finish in finishers {
            finish.send(()).unwrap();
        }
        for request in requests {
            assert_eq!(request.await.unwrap().unwrap(), Some(()));
        }
        assert_eq!(context.media_readers.available_permits(), 4);
        assert!(context.audio_metadata("note").await.unwrap().is_some());
    }
}

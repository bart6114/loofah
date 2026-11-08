use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use hypr_loofah_protocol::Change;
use hypr_vault_write::{SessionStore, StoreError};
use tokio_util::sync::CancellationToken;

use crate::{
    VaultContext,
    search::SearchService,
    settings::ConfigState,
    startup::{self, StartupPhase, StartupState},
};

#[cfg(test)]
mod ownership_tests;
mod processing;
mod watch;

pub struct VaultRuntime {
    pub context: Arc<VaultContext>,
    stop: CancellationToken,
    workers: Vec<tokio::task::JoinHandle<()>>,
    watcher: Option<watch::FileWatcher>,
    processing: Option<crate::jobs::JobWorker>,
    pub(crate) transcription: Option<crate::transcription_jobs::TranscriptionWorker>,
    pub(crate) capture_processing: Option<crate::capture_processing::CaptureProcessingWorker>,
    host: tokio::runtime::Handle,
    draining: bool,
}

struct RuntimeDrain {
    task: tokio::task::JoinHandle<Result<(), StoreError>>,
    phase: tokio::sync::watch::Receiver<&'static str>,
}

impl VaultRuntime {
    pub async fn open(
        id: String,
        vault: &Path,
        cache_root: &Path,
        operation_root: &Path,
    ) -> Result<Self, StoreError> {
        Self::open_owned(
            id,
            vault.to_path_buf(),
            cache_root.to_path_buf(),
            operation_root.to_path_buf(),
            None,
            None,
        )
        .await
    }

    /// A standalone client must transfer its acquired owner instead of taking a second lock.
    pub async fn open_leased(
        id: String,
        lease: hypr_storage::vault::VaultLease,
        cache_root: &Path,
        operation_root: &Path,
        content_limits: hypr_vault_write::ContentLimits,
    ) -> Result<Self, StoreError> {
        let vault = lease.canonical_vault().to_owned();
        Self::open_owned(
            id,
            vault,
            cache_root.to_owned(),
            operation_root.to_owned(),
            None,
            Some((lease, content_limits, None)),
        )
        .await
    }

    pub async fn open_leased_with_ready_cache(
        id: String,
        lease: hypr_storage::vault::VaultLease,
        cache_root: &Path,
        operation_root: &Path,
        content_limits: hypr_vault_write::ContentLimits,
        cache: hypr_search_cache::Cache,
    ) -> Result<Self, StoreError> {
        let vault = lease.canonical_vault().to_owned();
        Self::open_owned(
            id,
            vault,
            cache_root.to_owned(),
            operation_root.to_owned(),
            None,
            Some((lease, content_limits, Some(cache))),
        )
        .await
    }

    /// Reuse a host-owned change source when the host manages filesystem observation.
    pub async fn open_with_changes(
        id: String,
        vault: &Path,
        cache_root: &Path,
        operation_root: &Path,
        changes: tokio::sync::mpsc::UnboundedReceiver<String>,
    ) -> Result<Self, StoreError> {
        Self::open_owned(
            id,
            vault.to_path_buf(),
            cache_root.to_path_buf(),
            operation_root.to_path_buf(),
            Some(crate::vault_watch::ChangeSource::Unbounded(changes)),
            None,
        )
        .await
    }

    async fn open_owned(
        id: String,
        vault: std::path::PathBuf,
        cache: std::path::PathBuf,
        operations: std::path::PathBuf,
        changes: Option<crate::vault_watch::ChangeSource>,
        admitted: Option<(
            hypr_storage::vault::VaultLease,
            hypr_vault_write::ContentLimits,
            Option<hypr_search_cache::Cache>,
        )>,
    ) -> Result<Self, StoreError> {
        // Dropping a request detaches this owner; nested blocking recovery/migration
        // still runs to completion with the real filesystem lease in its stack.
        tokio::spawn(async move {
            Self::open_inner(id, &vault, &cache, &operations, changes, admitted).await
        })
        .await
        .map_err(|cause| StoreError::Io(format!("vault initialization worker failed: {cause}")))?
    }

    async fn open_inner(
        id: String,
        vault: &Path,
        cache_root: &Path,
        operation_root: &Path,
        changes: Option<crate::vault_watch::ChangeSource>,
        admitted: Option<(
            hypr_storage::vault::VaultLease,
            hypr_vault_write::ContentLimits,
            Option<hypr_search_cache::Cache>,
        )>,
    ) -> Result<Self, StoreError> {
        let vault = vault.to_path_buf();
        let operations = operation_root.to_path_buf();
        let (vault, lease, operation_root, content_limits, ready_cache) =
            tokio::task::spawn_blocking(move || {
                let (lease, limits, ready_cache) = match admitted {
                    Some((lease, limits, cache)) => {
                        if lease.canonical_vault() != vault.as_path() {
                            return Err(StoreError::Conflict(
                                "Acquired lease belongs to another vault".into(),
                            ));
                        }
                        if let Some(cache) = &cache {
                            if cache.vault().canonicalize().map_err(io_error)? != vault {
                                return Err(StoreError::Conflict(
                                    "Ready cache belongs to another vault".into(),
                                ));
                            }
                            cache
                                .check()
                                .map_err(|cause| StoreError::Io(cause.to_string()))?;
                        }
                        (lease, limits, cache)
                    }
                    None => {
                        std::fs::create_dir_all(&vault).map_err(io_error)?;
                        let lease =
                            match hypr_storage::vault::VaultLease::try_acquire_existing(&vault)
                                .map_err(io_error)?
                            {
                                hypr_storage::vault::LeaseAttempt::Acquired(lease) => lease,
                                hypr_storage::vault::LeaseAttempt::Busy => {
                                    return Err(StoreError::Conflict(
                                        "vault is already open; connect through its local owner"
                                            .into(),
                                    ));
                                }
                            };
                        (lease, hypr_vault_write::ContentLimits::default(), None)
                    }
                };
                let vault = lease.canonical_vault().to_owned();
                // This is trusted host configuration, not a client path. Resolve its
                // ancestor aliases once; private descendants still use strict guards.
                std::fs::create_dir_all(&operations).map_err(io_error)?;
                let operations = std::fs::canonicalize(operations).map_err(io_error)?;
                Ok((vault, lease, operations, limits, ready_cache))
            })
            .await
            .map_err(|error| StoreError::Io(error.to_string()))??;
        let store = Arc::new(SessionStore::with_content_limits(
            vault.clone(),
            content_limits,
        ));
        store.pin_vault_lease(&lease)?;
        let (lease_pin, recording_publications) = VaultContext::bootstrap_recording_publications(
            store.clone(),
            id.clone(),
            operation_root.join("recording-publications"),
            lease.pin(),
        )
        .await?;
        store.set_startup_pending(true);
        let (watcher, source) = if let Some(changes) = changes {
            (None, changes)
        } else {
            let watched = vault.clone();
            let (watcher, source) = tokio::task::spawn_blocking(move || watch::start(&watched))
                .await
                .map_err(|error| {
                    StoreError::Io(format!("vault watcher startup failed: {error}"))
                })??;
            (Some(watcher), source)
        };
        store
            .recover_document_writes(&operation_root.join("document-saves"))
            .await?;
        let startup = StartupState::new(Some(&vault));
        startup::prepare(store.clone(), startup.clone())
            .await
            .map_err(|error| StoreError::Io(error.to_string()))?;
        let mut context = VaultContext::new(
            id,
            store.clone(),
            startup.clone(),
            Arc::new(ConfigState::load_with_vault_lease(&lease)),
            Arc::new(
                ready_cache
                    .map(SearchService::with_ready_cache)
                    .unwrap_or_default(),
            ),
        )
        .with_operation_storage(operation_root.to_path_buf());
        context.lease = Some(lease_pin);
        context.recording_publications = recording_publications;
        let context = Arc::new(context);
        let stop = CancellationToken::new();
        let mut settings_changes = context.settings.subscribe_processing();
        let settings_events = context.events.clone();
        let settings_stop = stop.clone();
        let settings_worker = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = settings_stop.cancelled() => break,
                    change = settings_changes.changed() => if change.is_err() { break; },
                }
                settings_events.publish(Change::ProcessingSettingsChanged);
            }
        });
        let watch_store = store.clone();
        let watch_stop = stop.clone();
        let watch_worker = tokio::spawn(async move {
            crate::vault_watch::run_source_until_stopped(watch_store, source, watch_stop).await;
        });
        let changes = store
            .take_index_change_receiver()
            .expect("new store owns its change receiver");
        let events = context.events.clone();
        let events_stop = stop.clone();
        let events_worker = tokio::spawn(async move {
            tokio::select! {
                _ = events_stop.cancelled() => {},
                _ = hypr_vault_write::index::run_index_change_dispatcher(changes, move |event| {
                    events.publish(Change::IndexChanged { entity: event.entity, ids: event.ids, artifacts: event.artifacts });
                }) => {},
            }
        });
        let projection_changes = store.subscribe_index_changes();
        let service = context.search.clone();
        let search_events = context.events.clone();
        let search_stop = stop.clone();
        let cache_root = cache_root.to_path_buf();
        store.set_startup_pending(false);
        startup.set_phase(StartupPhase::Ready);
        let search_worker = tokio::spawn(async move {
            service
                .run(
                    &cache_root,
                    &vault,
                    &startup,
                    projection_changes,
                    search_stop,
                    move || {
                        search_events.publish(Change::SearchReady);
                    },
                )
                .await;
        });
        Ok(Self {
            context,
            stop,
            workers: vec![events_worker, search_worker, watch_worker, settings_worker],
            watcher,
            processing: None,
            transcription: None,
            capture_processing: None,
            host: tokio::runtime::Handle::current(),
            draining: false,
        })
    }

    /// A deadline limits this response, not the owned drain. Process hosts retain
    /// a StartupState clone and await Stopped before tearing down their runtime.
    pub async fn shutdown(self) -> Result<(), StoreError> {
        self.shutdown_with_deadline(Duration::from_secs(5)).await
    }

    async fn shutdown_with_deadline(mut self, deadline: Duration) -> Result<(), StoreError> {
        let Some(RuntimeDrain { task, phase }) = self.start_drain() else {
            return Ok(());
        };
        tokio::time::timeout(deadline, task)
            .await
            .map_err(|_| StoreError::Io((*phase.borrow()).into()))?
            .map_err(|cause| StoreError::Io(format!("vault drain worker failed: {cause}")))?
    }

    fn start_drain(&mut self) -> Option<RuntimeDrain> {
        if self.draining {
            return None;
        }
        self.draining = true;
        self.begin_stopping();
        let context = self.context.clone();
        let workers = std::mem::take(&mut self.workers);
        let processing = self.processing.take();
        let transcription = self.transcription.take();
        let capture_processing = self.capture_processing.take();
        if let Some(watcher) = self.watcher.take() {
            // Native cleanup only owns the closed event input, never Store or vault data.
            self.host.spawn_blocking(move || drop(watcher));
        }
        let (phase, progress) =
            tokio::sync::watch::channel("vault flush exceeded shutdown deadline");
        let task = self.host.spawn(async move {
            let mut retry = Duration::from_secs(1);
            loop {
                match context.store.flush_all().await {
                    Ok(()) => break,
                    Err(cause) => {
                        // The buffers may contain the only copy. A response deadline
                        // cannot release ownership while their existing retry tasks run.
                        tracing::error!(%cause, "vault shutdown retains ownership until buffered transcripts are saved");
                        tokio::time::sleep(retry).await;
                        retry = (retry * 2).min(Duration::from_secs(30));
                    }
                }
            }
            let mut failure = None;
            if let Some(worker)=capture_processing {
                phase.send_replace("capture processing coordinator exceeded shutdown deadline");
                if let Err(cause)=worker.shutdown().await {
                    tracing::error!(%cause,"capture processing intent needs review");
                    failure=Some(StoreError::Io("capture processing intent needs review".into()));
                }
            }
            if let Some(service) = context.capture_live.get() {
                phase.send_replace("live capture processing exceeded shutdown deadline");
                if let Err(cause) = service.shutdown().await {
                    tracing::error!(%cause,"live capture output needs review after native drain");
                    failure = Some(StoreError::Io("live capture output needs review".into()));
                }
            }
            if let Some(worker) = transcription {
                phase.send_replace("transcription worker exceeded shutdown deadline");
                if let Err(cause) = worker.shutdown().await {
                    tracing::error!(%cause, "transcription worker did not stop cleanly");
                    failure = Some(StoreError::Io("transcription worker did not stop cleanly".into()));
                }
            }
            if let Some(worker) = processing {
                phase.send_replace("processing worker exceeded shutdown deadline");
                if let Err(cause) = worker.shutdown().await {
                    tracing::error!(%cause, "processing worker did not stop cleanly");
                    failure = Some(StoreError::Io("processing worker did not stop cleanly".into()));
                }
            }
            phase.send_replace("vault workers did not stop before shutdown deadline");
            for worker in workers {
                if let Err(cause) = worker.await {
                    failure.get_or_insert_with(|| StoreError::Io(format!("vault worker failed: {cause}")));
                }
            }
            context.startup.set_phase(StartupPhase::Stopped);
            failure.map_or(Ok(()), Err)
        });
        Some(RuntimeDrain {
            task,
            phase: progress,
        })
    }

    fn begin_stopping(&self) {
        self.context.store.set_startup_pending(true);
        if !matches!(self.context.startup.snapshot().phase, StartupPhase::Stopped) {
            self.context.startup.set_phase(StartupPhase::Stopping);
        }
        self.stop.cancel();
        if let Some(service) = self.context.capture_processing.get() {
            service.request_shutdown();
        }
        if let Some(service) = self.context.capture_live.get() {
            service.request_shutdown();
        }
        if let Some(worker) = &self.processing {
            worker.request_shutdown();
        }
        if let Some(worker) = &self.transcription {
            worker.request_shutdown();
        }
    }
}

impl Drop for VaultRuntime {
    fn drop(&mut self) {
        // Detach the same owned drain used by explicit shutdown. Aborting an async
        // waiter would detach its blocking I/O and release Context's lease early.
        let _ = self.start_drain();
    }
}

fn io_error(error: std::io::Error) -> StoreError {
    StoreError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    async fn open_runtime(
        id: String,
        vault: &std::path::Path,
        cache: &std::path::Path,
        operations: &std::path::Path,
    ) -> Result<crate::VaultRuntime, hypr_vault_write::StoreError> {
        crate::VaultRuntime::open_with_changes(
            id,
            vault,
            cache,
            operations,
            tokio::sync::mpsc::unbounded_channel().1,
        )
        .await
    }
    use super::*;
    use hypr_loofah_protocol::{NoteSaveOutcome, SaveNoteRequest};

    #[tokio::test]
    async fn restored_pending_document_state_recovers_before_headless_readiness_and_indexing() {
        use serde_json::json;
        use sha2::{Digest, Sha256};
        let source = tempfile::tempdir().unwrap();
        let bookkeeping = tempfile::tempdir().unwrap();
        let store = SessionStore::new(source.path().into());
        let meta = serde_json::from_value(
            json!({"id":"note","title":"Meeting","created_at":"2026-11-05T10:00:00Z","tags":[]}),
        )
        .unwrap();
        store
            .create_note_session(&meta, "Personal text")
            .await
            .unwrap();
        store.ensure_summary("note").await.unwrap();
        store
            .update_generated_summary("note", "- [ ] Call Alice", Some(""))
            .await
            .unwrap();
        let target = hypr_loofah_protocol::DocumentTarget::Summary;
        let before = store
            .read_document_snapshot("note", &target)
            .await
            .unwrap()
            .unwrap();
        let mut tasks = store.session_tasks("note").await.unwrap();
        tasks[0].status = "done".into();
        let mut task_bytes = vec![1];
        task_bytes.extend(std::fs::read(source.path().join("sessions/note/tasks.json")).unwrap());
        let hash = |bytes: &[u8]| {
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        std::fs::write(
            source.path().join("sessions/note/summary.md"),
            "- [x] Call Alice",
        )
        .unwrap();
        let after = store
            .read_document_snapshot("note", &target)
            .await
            .unwrap()
            .unwrap();
        let intent = json!({"schema_version":1,"session_id":"note","target":target,"before_revision":before.revision,"after_revision":after.revision,"before_tasks_hash":hash(&task_bytes),"tasks":tasks});
        let filename = format!("{}.json", hash(b"note"));
        hypr_storage::fs::atomic_write(
            &bookkeeping.path().join("document-saves").join(&filename),
            &intent.to_string(),
        )
        .unwrap();
        drop(store);
        let restored_vault = tempfile::tempdir().unwrap();
        let restored_runtime = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        hypr_storage::fs::copy_dir_recursive(source.path(), restored_vault.path(), None)
            .await
            .unwrap();
        hypr_storage::fs::copy_dir_recursive(bookkeeping.path(), restored_runtime.path(), None)
            .await
            .unwrap();
        let runtime = open_runtime(
            "restored".into(),
            restored_vault.path(),
            cache.path(),
            restored_runtime.path(),
        )
        .await
        .unwrap();
        assert!(runtime.context.startup.is_ready());
        assert_eq!(runtime.context.session_tasks("note").await.unwrap(), tasks);
        assert_eq!(
            runtime.context.session_documents("note").await.unwrap()[0].markdown,
            "- [x] Call Alice"
        );
        assert_eq!(
            runtime
                .context
                .read_note("note")
                .await
                .unwrap()
                .markdown
                .as_deref(),
            Some("Personal text")
        );
        assert!(
            !restored_runtime
                .path()
                .join("document-saves")
                .join(&filename)
                .exists()
        );
        assert!(
            bookkeeping
                .path()
                .join("document-saves")
                .join(filename)
                .exists()
        );
        runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn headless_lifecycle_exclusively_owns_vault_and_reopens_canonical_notes() {
        let vault = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let runtime = open_runtime(
            "private".into(),
            vault.path(),
            cache.path(),
            &cache.path().join("operations"),
        )
        .await
        .unwrap();
        assert!(runtime.context.startup.is_ready());
        assert!(matches!(
            open_runtime(
                "other".into(),
                vault.path(),
                cache.path(),
                &cache.path().join("operations")
            )
            .await,
            Err(StoreError::Conflict(_))
        ));
        let meta = serde_json::from_value(serde_json::json!({
            "id": "note", "title": "Original", "created_at": "2026-11-05T10:00:00Z", "tags": []
        }))
        .unwrap();
        runtime.context.store.write_meta(&meta).await.unwrap();
        let revision = runtime.context.read_note("note").await.unwrap().revision;
        let saved = runtime
            .context
            .save_note(
                "note",
                SaveNoteRequest {
                    markdown: "restart-safe".into(),
                    title: None,
                    expected_revision: revision,
                },
            )
            .await
            .unwrap();
        let NoteSaveOutcome::Saved(saved) = saved else {
            panic!("initial save conflicted");
        };
        let remaining_client = runtime.context.clone();
        runtime.shutdown().await.unwrap();
        assert_eq!(
            remaining_client.read_note("note").await.unwrap_err().code,
            hypr_loofah_protocol::ErrorCode::Unavailable
        );
        assert!(matches!(
            open_runtime(
                "other".into(),
                vault.path(),
                cache.path(),
                &cache.path().join("operations")
            )
            .await,
            Err(StoreError::Conflict(_))
        ));
        drop(remaining_client);
        let reopened = open_runtime(
            "private".into(),
            vault.path(),
            cache.path(),
            &cache.path().join("operations"),
        )
        .await
        .unwrap();
        assert_eq!(reopened.context.read_note("note").await.unwrap(), saved);
        reopened.shutdown().await.unwrap();
    }
}

use std::path::Path;
use std::sync::Arc;

pub use hypr_loofah_protocol::{StartupPhase, StartupStatus};
use hypr_vault_write::SessionStore;

#[derive(Clone)]
pub struct StartupState {
    inner: tokio::sync::watch::Sender<StartupStatus>,
    observer: Option<Arc<dyn Fn(StartupStatus) + Send + Sync>>,
}

impl StartupState {
    pub fn new(vault_path: Option<&Path>) -> Self {
        let path = vault_path.map(Path::to_path_buf).unwrap_or_default();
        Self {
            observer: None,
            inner: tokio::sync::watch::channel(StartupStatus {
                revision: 0,
                vault_path: path.to_string_lossy().into_owned(),
                is_cloud_storage: is_cloud_storage_path(&path),
                phase: StartupPhase::OpeningVault,
                migration_issues: Vec::new(),
            })
            .0,
        }
    }

    pub fn snapshot(&self) -> StartupStatus {
        self.inner.borrow().clone()
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.inner.borrow().phase, StartupPhase::Ready)
    }

    pub fn with_observer(
        mut self,
        observer: impl Fn(StartupStatus) + Send + Sync + 'static,
    ) -> Self {
        self.observer = Some(Arc::new(observer));
        self
    }

    pub async fn wait_until_ready(&self) -> Result<(), String> {
        let mut receiver = self.inner.subscribe();
        loop {
            match &receiver.borrow_and_update().phase {
                StartupPhase::Ready => return Ok(()),
                StartupPhase::Failed { message } => return Err(message.clone()),
                _ => {}
            }
            receiver
                .changed()
                .await
                .map_err(|error| error.to_string())?;
        }
    }

    pub fn set_phase(&self, phase: StartupPhase) -> StartupStatus {
        let mut updated = None;
        self.inner.send_modify(|status| {
            status.revision += 1;
            status.phase = phase;
            updated = Some(status.clone());
        });
        let status = updated.unwrap();
        if let Some(observer) = &self.observer {
            observer(status.clone());
        }
        status
    }
}

pub async fn prepare(
    store: Arc<SessionStore>,
    state: StartupState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    state.set_phase(StartupPhase::Scanning { sessions_found: 0 });

    let scan_state = state.clone();
    let layout = store
        .normalize_startup_layout_with_progress(move |sessions_found| {
            scan_state.set_phase(StartupPhase::Scanning { sessions_found });
        })
        .await?;

    let migration = &layout.migration;
    state.inner.send_modify(|status| {
        status.migration_issues = migration
            .skipped
            .iter()
            .chain(&migration.failed)
            .cloned()
            .collect();
    });
    if !migration.renamed.is_empty() || !migration.failed.is_empty() {
        tracing::info!(
            renamed = migration.renamed.len(),
            skipped = migration.skipped.len(),
            failed = ?migration.failed,
            "migrated session directories to canonical IDs"
        );
    }

    state.set_phase(StartupPhase::ArchivingLegacyTemplates);
    let archive_base = store.vault_base().to_path_buf();
    match tokio::task::spawn_blocking(move || {
        hypr_vault_write::legacy_templates::archive(&archive_base)
    })
    .await
    {
        Ok(Ok(archived)) => tracing::info!(archived, "archived legacy summary templates"),
        Ok(Err(error)) => {
            tracing::error!(%error, "legacy template archival failed; will retry next startup")
        }
        Err(error) => {
            tracing::error!(%error, "legacy template archival task failed; will retry next startup")
        }
    }

    let total = layout.session_count();
    state.set_phase(StartupPhase::Indexing {
        completed: 0,
        total,
    });
    let index_state = state.clone();
    let report = store
        .rebuild_index_from_startup_layout_with_progress(layout, move |completed, total| {
            index_state.set_phase(StartupPhase::Indexing { completed, total });
        })
        .await?;
    tracing::info!(
        sessions = report.sessions,
        notes = report.notes,
        transcripts = report.transcripts,
        error_count = report.errors.len(),
        errors = ?report.errors,
        ghost_session_count = report.ghost_sessions.len(),
        ghost_sessions = ?report.ghost_sessions,
        "startup session index rebuild complete"
    );

    let vault_path = store.vault_base().to_path_buf();
    match tokio::task::spawn_blocking(move || {
        hypr_vault_write::agents_doc::ensure_agents_doc(&vault_path)
    })
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => tracing::error!(%error, "failed to write AGENTS.md"),
        Err(error) => tracing::error!(%error, "AGENTS.md task failed"),
    }

    Ok(())
}

fn is_cloud_storage_path(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.contains("/Library/CloudStorage/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn readiness_is_observed_before_and_after_subscription() {
        let state = StartupState::new(None);
        let wait = state.wait_until_ready();
        tokio::pin!(wait);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), &mut wait)
                .await
                .is_err()
        );
        state.set_phase(StartupPhase::Ready);
        wait.await.unwrap();
        state.wait_until_ready().await.unwrap();
    }

    #[tokio::test]
    async fn failed_startup_wakes_waiters_and_late_subscribers() {
        let state = StartupState::new(None);
        let wait = state.wait_until_ready();
        tokio::pin!(wait);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), &mut wait)
                .await
                .is_err()
        );
        state.set_phase(StartupPhase::Failed {
            message: "unreadable vault".into(),
        });
        assert_eq!(wait.await.unwrap_err(), "unreadable vault");
        assert_eq!(
            state.wait_until_ready().await.unwrap_err(),
            "unreadable vault"
        );
    }

    #[test]
    fn phase_changes_preserve_migration_issues_in_the_watch_snapshot() {
        let state = StartupState::new(None);
        state.inner.send_modify(|status| {
            status
                .migration_issues
                .push("sessions/copy: occupied destination".into());
        });
        let ready = state.set_phase(StartupPhase::Ready);
        assert_eq!(
            ready.migration_issues,
            ["sessions/copy: occupied destination"]
        );
        assert_eq!(state.snapshot(), ready);
    }

    #[test]
    fn recognizes_macos_cloud_storage_paths() {
        assert!(is_cloud_storage_path(Path::new(
            "/Users/test/Library/CloudStorage/GoogleDrive-user/My Drive/vault"
        )));
        assert!(!is_cloud_storage_path(Path::new(
            "/Users/test/Documents/vault"
        )));
    }

    #[test]
    fn revisions_increase_with_each_phase_change() {
        let state = StartupState::new(Some(Path::new("/tmp/vault")));

        let scanning = state.set_phase(StartupPhase::Scanning { sessions_found: 3 });
        let ready = state.set_phase(StartupPhase::Ready);

        assert_eq!(scanning.revision, 1);
        assert_eq!(ready.revision, 2);
        assert!(state.is_ready());
    }
}

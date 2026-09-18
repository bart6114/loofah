use std::path::Path;
use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tauri_plugin_notify::NotifyPluginExt;
use tauri_specta::Event;

use crate::session_store::SessionStore;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StartupPhase {
    OpeningVault,
    Scanning { sessions_found: usize },
    Indexing { completed: usize, total: usize },
    ArchivingLegacyTemplates,
    Ready,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct StartupStatus {
    pub revision: u64,
    pub vault_path: String,
    pub is_cloud_storage: bool,
    pub phase: StartupPhase,
    pub migration_issues: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct StartupProgress {
    pub status: StartupStatus,
}

#[derive(Clone)]
pub struct StartupState {
    inner: tokio::sync::watch::Sender<StartupStatus>,
}

impl StartupState {
    pub fn new(vault_path: Option<&Path>) -> Self {
        let path = vault_path.map(Path::to_path_buf).unwrap_or_default();
        Self {
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

    pub fn update<R: tauri::Runtime>(&self, app: &AppHandle<R>, phase: StartupPhase) {
        let status = self.set_phase(phase);
        if let Err(error) = (StartupProgress { status }).emit(app) {
            tracing::warn!(%error, "failed to emit startup progress");
        }
    }

    pub(crate) async fn wait_until_ready(&self) -> Result<(), String> {
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

    pub(crate) fn set_phase(&self, phase: StartupPhase) -> StartupStatus {
        let mut updated = None;
        self.inner.send_modify(|status| {
            status.revision += 1;
            status.phase = phase;
            updated = Some(status.clone());
        });
        updated.unwrap()
    }
}

#[tauri::command]
#[specta::specta]
pub fn get_startup_status<R: tauri::Runtime>(app: AppHandle<R>) -> StartupStatus {
    app.state::<StartupState>().snapshot()
}

pub fn spawn(app: AppHandle, store: Arc<SessionStore>) {
    let state = app.state::<StartupState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = initialize(app.clone(), store, state.clone()).await {
            tracing::error!(%error, "vault startup failed");
            state.update(
                &app,
                StartupPhase::Failed {
                    message: error.to_string(),
                },
            );
        }
    });
}

async fn initialize(
    app: AppHandle,
    store: Arc<SessionStore>,
    state: StartupState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    crate::sync::recover(&app, store.vault_base())
        .await
        .map_err(std::io::Error::other)?;
    state.update(&app, StartupPhase::Scanning { sessions_found: 0 });

    let scan_app = app.clone();
    let scan_state = state.clone();
    let layout = store
        .normalize_startup_layout_with_progress(move |sessions_found| {
            scan_state.update(&scan_app, StartupPhase::Scanning { sessions_found });
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

    state.update(&app, StartupPhase::ArchivingLegacyTemplates);
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
    state.update(
        &app,
        StartupPhase::Indexing {
            completed: 0,
            total,
        },
    );
    let index_app = app.clone();
    let index_state = state.clone();
    let report = store
        .rebuild_index_from_startup_layout_with_progress(layout, move |completed, total| {
            index_state.update(&index_app, StartupPhase::Indexing { completed, total });
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

    crate::vault_watch::spawn(app.clone());
    crate::recording_meta::spawn(app.clone());
    store.set_startup_pending(false);
    state.update(&app, StartupPhase::Ready);

    tokio::task::spawn_blocking(move || {
        if let Err(error) = app.notify().start() {
            tracing::error!(%error, "failed to start vault watcher");
        }
    });

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

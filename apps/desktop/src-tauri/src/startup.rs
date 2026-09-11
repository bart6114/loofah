use std::path::Path;
use std::sync::{Arc, Mutex};

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
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct StartupProgress {
    pub status: StartupStatus,
}

#[derive(Clone)]
pub struct StartupState {
    inner: Arc<Mutex<StartupStatus>>,
}

impl StartupState {
    pub fn new(vault_path: Option<&Path>) -> Self {
        let path = vault_path.map(Path::to_path_buf).unwrap_or_default();
        Self {
            inner: Arc::new(Mutex::new(StartupStatus {
                revision: 0,
                vault_path: path.to_string_lossy().into_owned(),
                is_cloud_storage: is_cloud_storage_path(&path),
                phase: StartupPhase::OpeningVault,
            })),
        }
    }

    pub fn snapshot(&self) -> StartupStatus {
        self.inner.lock().unwrap().clone()
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.inner.lock().unwrap().phase, StartupPhase::Ready)
    }

    pub fn update<R: tauri::Runtime>(&self, app: &AppHandle<R>, phase: StartupPhase) {
        let status = self.set_phase(phase);
        if let Err(error) = (StartupProgress { status }).emit(app) {
            tracing::warn!(%error, "failed to emit startup progress");
        }
    }

    fn set_phase(&self, phase: StartupPhase) -> StartupStatus {
        let mut status = self.inner.lock().unwrap();
        status.revision += 1;
        status.phase = phase;
        status.clone()
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
    state.update(&app, StartupPhase::Scanning { sessions_found: 0 });

    let scan_app = app.clone();
    let scan_state = state.clone();
    let layout = store
        .normalize_startup_layout_with_progress(move |sessions_found| {
            scan_state.update(&scan_app, StartupPhase::Scanning { sessions_found });
        })
        .await?;

    let migration = &layout.migration;
    if !migration.renamed.is_empty() || !migration.failed.is_empty() {
        tracing::info!(
            renamed = migration.renamed.len(),
            skipped = migration.skipped.len(),
            failed = ?migration.failed,
            "migrated legacy session directories to readable names"
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

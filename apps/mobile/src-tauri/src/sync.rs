use super::{MobileState, jobs, persist, recording};
use hypr_vault_sync::{Engine, Resolution, SyncStatus};
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager, State};
use tauri_plugin_mobile_native::MobileNativeExt;

#[derive(Default)]
pub struct SyncState {
    pub folder: Option<String>,
    engine: Option<Engine>,
    last: SyncStatus,
    pub error: Option<String>,
}

impl SyncState {
    pub fn has_audio(&self, session_id: &str) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.has_session_audio(session_id).unwrap_or(false))
    }

    pub fn disconnected(&mut self, reason: &str) {
        self.engine = None;
        self.last.connected = false;
        self.error = Some(reason.into());
    }
    pub fn status(&self) -> SyncStatus {
        let mut status = self.last.clone();
        status.connected = self.engine.is_some() || status.connected;
        if status.state.is_empty() {
            status.state = "disconnected".into();
        }
        if self.error.is_some() {
            status.error = self.error.clone();
            status.state = "error".into();
        }
        status
    }
}

async fn attach(app: &tauri::AppHandle, remote: String) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let binding = state.state_dir.join("vault-binding.json");
    match std::fs::read(&binding) {
        Ok(bytes) => {
            let previous: String = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if previous != remote {
                return Err("This installation is already linked to another vault. Reconnect the original folder to preserve unsynced changes.".into());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    let engine = Engine::open(
        state.store.vault_base(),
        &remote,
        state.state_dir.join("sync"),
    )
    .map_err(|e| e.to_string())?;
    persist::write_json(&binding, &remote)?;
    *state.sync.lock().unwrap() = SyncState {
        engine: Some(engine),
        folder: Some(remote),
        last: SyncStatus {
            connected: true,
            state: "pending".into(),
            ..Default::default()
        },
        error: None,
    };
    state.changed(app);
    Ok(())
}

pub async fn restore(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<MobileState>();
    match std::fs::read(state.state_dir.join("vault-binding.json")) {
        Ok(bytes) => {
            state.sync.lock().unwrap().folder =
                Some(serde_json::from_slice::<String>(&bytes).map_err(|e| e.to_string())?);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = app;
        return Ok(());
    }
    #[cfg(target_os = "ios")]
    {
        if let Some(remote) = app
            .mobile_native()
            .restore_icloud_vault()
            .await
            .map_err(|e| e.to_string())?
            .path
        {
            attach(app, remote).await?;
        }
        Ok(())
    }
}

#[tauri::command]
pub async fn mobile_connect_vault(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    if recording::is_active()
        || state.worker.load(Ordering::Acquire)
        || state.model_busy.load(Ordering::Acquire)
    {
        return Err("Finish recording and pause processing before connecting a vault".into());
    }
    let selected = app
        .mobile_native()
        .select_icloud_vault()
        .await
        .map_err(|e| e.to_string())?;
    if let Some(path) = selected.path {
        attach(&app, path).await?;
    }
    drop(_guard);
    reconcile(&app).await
}

pub async fn reconcile(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _operation = state.operation.lock().await;
    if recording::is_active()
        || state.worker.load(Ordering::Acquire)
        || state.model_busy.load(Ordering::Acquire)
    {
        return Ok(());
    }
    let Some(mut engine) = state.sync.lock().unwrap().engine.take() else {
        return Ok(());
    };
    {
        let mut sync = state.sync.lock().unwrap();
        sync.last.connected = true;
        sync.last.state = "syncing".into();
        sync.error = None;
    }
    state.dirty.store(false, Ordering::Release);
    let task_app = app.clone();
    let (engine, result) = tokio::task::spawn_blocking(move || {
        let result = engine
            .reconcile(64)
            .map_err(|e| e.to_string())
            .and_then(|status| {
                let state = task_app.state::<MobileState>();
                jobs::prune_deleted(&mut state.jobs.lock().unwrap(), &state.state_dir, |id| {
                    engine.is_session_deleted(id).map_err(|e| e.to_string())
                })?;
                Ok(status)
            });
        (engine, result)
    })
    .await
    .map_err(|error| {
        let message = format!("Sync stopped unexpectedly. Reconnect the vault to retry: {error}");
        state.sync.lock().unwrap().disconnected(&message);
        state.changed(&app);
        message
    })?;
    {
        let mut sync = state.sync.lock().unwrap();
        sync.engine = Some(engine);
        match &result {
            Ok(status) => sync.last = status.clone(),
            Err(error) => sync.error = Some(error.clone()),
        }
    }
    // A pass can commit some files before a later provider request fails.
    if result.as_ref().map_or(true, |status| status.changed > 0) {
        state
            .store
            .rebuild_index()
            .await
            .map_err(|e| e.to_string())?;
        *state.settings.lock().unwrap() = super::Settings::load(state.store.vault_base())?;
    }
    match result {
        Ok(_) => {
            let _ = app.emit("mobile-state-changed", ());
            Ok(())
        }
        Err(error) => {
            state.changed(app);
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn mobile_sync(app: tauri::AppHandle) -> Result<(), String> {
    reconcile(&app).await
}

#[tauri::command]
pub async fn mobile_resolve_conflict(
    app: tauri::AppHandle,
    conflict_id: String,
    resolution: String,
) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _operation = state.operation.lock().await;
    if recording::is_active()
        || state.worker.load(Ordering::Acquire)
        || state.model_busy.load(Ordering::Acquire)
    {
        return Err("Finish recording and pause processing before resolving conflicts".into());
    }
    let choice = match resolution.as_str() {
        "local" => Resolution::KeepLocal,
        "remote" => Resolution::KeepRemote,
        "both" => Resolution::KeepBoth,
        _ => return Err("Unknown conflict resolution".into()),
    };
    let mut engine = state
        .sync
        .lock()
        .unwrap()
        .engine
        .take()
        .ok_or("Connect an iCloud vault first")?;
    let (engine, result) = tokio::task::spawn_blocking(move || {
        let result = engine
            .resolve(&conflict_id, choice)
            .map_err(|e| e.to_string());
        (engine, result)
    })
    .await
    .map_err(|error| {
        let message = format!("Sync stopped unexpectedly. Reconnect the vault to retry: {error}");
        state.sync.lock().unwrap().disconnected(&message);
        state.changed(&app);
        message
    })?;
    {
        let mut sync = state.sync.lock().unwrap();
        sync.last.conflicts = engine.conflicts();
        sync.engine = Some(engine);
    }
    state
        .store
        .rebuild_index()
        .await
        .map_err(|e| e.to_string())?;
    *state.settings.lock().unwrap() = super::Settings::load(state.store.vault_base())?;
    state.changed(&app);
    result?;
    Ok(())
}

#[tauri::command]
pub async fn mobile_download_audio(
    app: tauri::AppHandle,
    state: State<'_, MobileState>,
    session_id: String,
) -> Result<(), String> {
    let _operation = state.operation.lock().await;
    state.session_dir(&session_id)?;
    if recording::is_active() {
        return Err("Finish recording before downloading audio".into());
    }
    let mut engine = state
        .sync
        .lock()
        .unwrap()
        .engine
        .take()
        .ok_or("Connect an iCloud vault first")?;
    let (engine, result) = tokio::task::spawn_blocking(move || {
        let mut last = "No recording available in iCloud".to_string();
        let mut result = Err(last.clone());
        for filename in ["audio.wav", "audio.mp3", "audio.ogg"] {
            match engine.download_media(&session_id, filename) {
                Ok(_) => {
                    result = Ok(());
                    break;
                }
                Err(error) => {
                    last = error.to_string();
                    result = Err(last.clone());
                }
            }
        }
        (engine, result)
    })
    .await
    .map_err(|error| {
        let message = format!("Sync stopped unexpectedly. Reconnect the vault to retry: {error}");
        state.sync.lock().unwrap().disconnected(&message);
        state.changed(&app);
        message
    })?;
    state.sync.lock().unwrap().engine = Some(engine);
    result?;
    state.changed(&app);
    Ok(())
}

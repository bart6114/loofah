use super::{MobileState, Settings, jobs, persist, providers, recording};
use serde_json::{Value, json};
use tauri::{Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_mobile_native::MobileNativeExt;

#[tauri::command]
pub async fn mobile_snapshot(
    app: tauri::AppHandle,
    state: State<'_, MobileState>,
) -> Result<Value, String> {
    let selected = state.settings.lock().unwrap().clone();
    let model = app
        .mobile_native()
        .model_status(&selected.transcription_model)
        .await
        .unwrap_or_default();
    let has_api_key = if providers::definition(&selected.summary_provider).is_some() {
        app.mobile_native()
            .has_api_key(&selected.summary_provider)
            .await
            .unwrap_or(false)
    } else {
        false
    };
    let summary_error = providers::validate(&selected, has_api_key).err();
    let summary = json!({"available":summary_error.is_none(),"reason":summary_error});
    let config =
        hypr_storage::config::read_config(state.store.vault_base()).map_err(|e| e.to_string())?;
    let mut connections = serde_json::Map::new();
    for (id, provider) in config.ai_providers {
        if let Some(id) = id.strip_prefix("llm:") {
            if providers::definition(id).is_some() && provider.kind == "llm" {
                let has_key = app.mobile_native().has_api_key(id).await.unwrap_or(false);
                connections.insert(
                    id.into(),
                    json!({"base_url":provider.base_url,"has_api_key":has_key}),
                );
            }
        }
    }
    let mut settings = serde_json::to_value(selected).map_err(|e| e.to_string())?;
    settings["has_api_key"] = json!(has_api_key);
    settings["providers"] = Value::Object(connections);
    let (sync_status, cloud_folder) = {
        let sync = state.sync.lock().unwrap();
        (sync.status(), sync.folder.clone())
    };
    Ok(
        json!({ "vault": {"local_path": state.store.vault_base(), "icloud_path": cloud_folder}, "sessions": state.store.session_list_headers().into_iter().rev().collect::<Vec<_>>(),
        "recording": recording::status(), "jobs": state.jobs.lock().unwrap().iter().map(|j|json!({"session_id":j.session_id,"kind":j.kind,"state":j.state,"progress":j.progress,"error":j.error})).collect::<Vec<_>>(), "model": {"ready":model.ready,"downloading":model.downloading || state.model_busy.load(std::sync::atomic::Ordering::Acquire),"model_id":model.model_id,"revision":model.revision,"downloaded_bytes":model.downloaded_bytes,"total_bytes":model.total_bytes,"phase":model.phase},
        "startup_errors": *state.startup_errors.lock().unwrap(),
        "summary": summary, "sync": sync_status, "settings": settings }),
    )
}

#[tauri::command]
pub async fn mobile_session(
    state: State<'_, MobileState>,
    session_id: String,
) -> Result<Value, String> {
    let session = state
        .store
        .session_get(&session_id)
        .ok_or("Session not found")?;
    let transcripts = state
        .store
        .session_transcripts(&session_id)
        .await
        .map_err(|e| e.to_string())?;
    let audio =
        hypr_vault_read::audio::resolve_final_audio_path(state.store.vault_base(), &session_id);
    let has_audio = audio.is_some() || state.sync.lock().unwrap().has_audio(&session_id);
    let vault = state.store.vault_base().to_path_buf();
    let id = session_id.clone();
    let attachments = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
        hypr_vault_read::read_session_attachments(&vault, &id).map_err(|error| error.to_string())?
            .into_iter().map(|attachment| {
                let path = std::path::Path::new(&attachment.path);
                let relative_path = path.strip_prefix(&vault).map_err(|error| error.to_string())?;
                Ok(json!({"name": attachment.attachment_id, "relative_path": relative_path.to_string_lossy(), "url": attachment.path}))
            }).collect()
    }).await.map_err(|error| error.to_string())??;
    let tasks = hypr_vault_read::tasks::read_session_tasks(state.store.vault_base(), &session_id)
        .map_err(|e| e.to_string())?;
    Ok(
        json!({ "id": session_id, "title": session.meta.title, "notes": session.note_markdown.unwrap_or_default(),
        "summary": state.store.read_summary(&session_id).await.map_err(|e| e.to_string())?,
        "transcript": transcripts.into_iter().flat_map(|t| t.words).map(|w| json!({"text":w.text,"start":w.start_ms/1000.0,"end":w.end_ms/1000.0})).collect::<Vec<_>>(),
        "tasks": tasks.into_iter().map(|t| json!({"id":t.id,"title":t.text,"completed":t.status=="done"})).collect::<Vec<_>>(),
        "has_audio": has_audio, "attachments": attachments,
        "audio_url": audio.map(|p| p.to_string_lossy().to_string()) }),
    )
}

#[tauri::command]
pub async fn mobile_open_attachment(
    app: tauri::AppHandle,
    state: State<'_, MobileState>,
    session_id: String,
    relative_path: String,
) -> Result<(), String> {
    state.session_dir(&session_id)?;
    let vault = state.store.vault_base().to_path_buf();
    let path = tokio::task::spawn_blocking(move || {
        selected_attachment_path(&vault, &session_id, &relative_path)
    })
    .await
    .map_err(|error| error.to_string())??;
    app.mobile_native()
        .preview_file(path.to_string_lossy())
        .await
        .map_err(|error| error.to_string())
}

fn selected_attachment_path(
    vault: &std::path::Path,
    session_id: &str,
    relative_path: &str,
) -> Result<std::path::PathBuf, String> {
    let attachments = hypr_vault_read::read_session_attachments(vault, session_id)
        .map_err(|error| error.to_string())?;
    attachments
        .into_iter()
        .find_map(|attachment| {
            let path = std::path::PathBuf::from(attachment.path);
            let listed = path.strip_prefix(vault).ok()?;
            (listed.to_string_lossy() == relative_path).then_some(path)
        })
        .ok_or_else(|| {
            "This attachment is unavailable on this device. Sync the note and try again.".into()
        })
}

pub async fn create_session(app: &tauri::AppHandle, title: &str) -> Result<String, String> {
    let state = app.state::<MobileState>();
    let id = uuid::Uuid::new_v4().to_string();
    let meta = hypr_vault_read::SessionMeta {
        id: id.clone(),
        title: title.into(),
        started_at: None,
        ended_at: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        tags: vec![],
        tag_suggestions: None,
        tracking_id: None,
        folder: None,
        author: None,
        skill: None,
        extra: Default::default(),
    };
    state
        .store
        .create_session_meta(&meta)
        .await
        .map_err(|e| e.to_string())?;
    state
        .store
        .write_note(&id, "")
        .await
        .map_err(|e| e.to_string())?;
    state.changed(app);
    Ok(id)
}

#[tauri::command]
pub async fn mobile_create_session(app: tauri::AppHandle) -> Result<String, String> {
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    create_session(&app, "").await
}

#[tauri::command]
pub async fn mobile_copy_text(app: tauri::AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mobile_delete_session(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    if recording::status().session_id.as_deref() == Some(&session_id) {
        return Err("Stop this recording before deleting the note".into());
    }
    if state
        .jobs
        .lock()
        .unwrap()
        .iter()
        .any(|job| job.session_id == session_id && job.state == "running")
    {
        return Err("Pause this note’s processing before deleting it".into());
    }
    if state
        .store
        .delete_session(&session_id)
        .await
        .map_err(|e| e.to_string())?
        .is_none()
    {
        return Err("This note no longer exists".into());
    }
    let cleanup = jobs::prune_deleted(&mut state.jobs.lock().unwrap(), &state.state_dir, |id| {
        Ok(id == session_id)
    });
    if let Err(error) = cleanup {
        let restored = state
            .store
            .restore_session(&session_id)
            .await
            .map_err(|e| e.to_string());
        state.changed(&app);
        return Err(match restored {
            Ok(true) => format!(
                "The note was restored because its queued work could not be removed: {error}"
            ),
            _ => format!("The note is in trash, but queued work could not be removed: {error}"),
        });
    }
    state.changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn mobile_restore_session(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    if !state
        .store
        .restore_session(&session_id)
        .await
        .map_err(|e| e.to_string())?
    {
        return Err("This note can no longer be restored with Undo. Its files may still be in the vault’s trash folder.".into());
    }
    state.changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn mobile_generate_title(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let guard = state.operation.lock().await;
    jobs::enqueue(&app, &session_id, "title").await?;
    drop(guard);
    jobs::kick(&app);
    Ok(())
}

#[tauri::command]
pub async fn mobile_update_session(
    app: tauri::AppHandle,
    session_id: String,
    title: String,
    notes: String,
    expected_notes: String,
    expected_title: String,
) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    let meta = state
        .store
        .read_meta(&session_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Session not found")?;
    let current_note = state
        .store
        .read_note(&session_id)
        .await
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    if meta.title != expected_title || current_note != expected_notes {
        return Err(
            "This note changed on another device. Your draft is preserved; reload before saving."
                .into(),
        );
    }
    state
        .store
        .write_note(&session_id, &notes)
        .await
        .map_err(|e| e.to_string())?;
    if let Err(error) = state
        .store
        .update_meta(
            &session_id,
            hypr_vault_write::SessionMetaPatch {
                title: Some(title),
                ..Default::default()
            },
        )
        .await
    {
        state.changed(&app);
        return Err(format!(
            "Notes were saved, but the title could not be saved. Reload before retrying: {error}"
        ));
    }
    state.changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn mobile_start_recording(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    state.session_dir(&session_id)?;
    if recording::is_active() {
        return Err("Stop the current recording before starting another".into());
    }
    if hypr_vault_read::audio::resolve_final_audio_path(state.store.vault_base(), &session_id)
        .is_some()
        || state.sync.lock().unwrap().has_audio(&session_id)
    {
        return Err("This note already has a recording. Create a new note to record again.".into());
    }
    state
        .store
        .prepare_recording_with_layout(&session_id, hypr_vault_read::AudioLayout::Mixed)
        .await
        .map_err(|e| e.to_string())?;
    // An in-flight inference window may finish; its next checkpoint yields to capture.
    if let Err(error) = recording::begin(state.store.vault_base(), &state.state_dir, &session_id) {
        state
            .store
            .release_recording_prepare(&session_id)
            .await
            .map_err(|e| e.to_string())?;
        return Err(error);
    }
    let mut native_started = false;
    let start: Result<(), String> = async {
        app.mobile_native()
            .protect_recording_file(
                state
                    .session_dir(&session_id)?
                    .join("audio.wav.tmp")
                    .to_string_lossy()
                    .as_ref(),
            )
            .await
            .map_err(|e| e.to_string())?;
        app.mobile_native()
            .start_recording()
            .await
            .map_err(|e| e.to_string())?;
        native_started = true;
        state
            .store
            .mark_recording_started(&session_id, &chrono::Utc::now().to_rfc3339())
            .await
            .map_err(|e| e.to_string())
    }
    .await;
    if let Err(error) = start {
        if native_started {
            if let Err(stop_error) = app.mobile_native().stop_recording().await {
                recording::native_event("recording_error");
                state.changed(&app);
                return Err(format!(
                    "{error}; recording could not be stopped: {stop_error}"
                ));
            }
        }
        let _ = recording::finish().await;
        state
            .store
            .release_recording_prepare(&session_id)
            .await
            .map_err(|e| e.to_string())?;
        let directory = state.session_dir(&session_id)?;
        if let Ok(reader) = hound::WavReader::open(directory.join("audio.wav")) {
            if reader.duration() == 0 {
                let _ = std::fs::remove_file(directory.join("audio.wav"));
            }
        }
        state.changed(&app);
        return Err(error);
    }
    let title = state
        .store
        .read_meta(&session_id)
        .await
        .ok()
        .flatten()
        .map(|meta| meta.title)
        .unwrap_or_default();
    let _ = app
        .mobile_native()
        .begin_recording_activity(&session_id, &title)
        .await;
    state.changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn mobile_stop_recording(app: tauri::AppHandle) -> Result<(), String> {
    stop_recording(app, None).await
}

pub(crate) async fn stop_recording(
    app: tauri::AppHandle,
    expected_session: Option<String>,
) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    let current = recording::status().session_id;
    if let Some(expected) = expected_session.as_ref() {
        if current.as_ref() != Some(expected) {
            let _ = app.mobile_native().end_recording_activity(expected).await;
            return Ok(());
        }
    }
    let id = current.ok_or("No recording is active")?;
    struct Stopping(tauri::AppHandle);
    impl Drop for Stopping {
        fn drop(&mut self) {
            recording::set_stopping(None);
            self.0.state::<MobileState>().changed(&self.0);
        }
    }
    recording::set_stopping(Some(&id));
    let _stopping = Stopping(app.clone());
    state.changed(&app);
    if let Err(error) = app.mobile_native().stop_recording().await {
        recording::native_event("recording_error");
        return Err(error.to_string());
    }
    let result = async {
        let finalized = recording::finish().await;
        let ended = state
            .store
            .mark_recording_ended(&id, &chrono::Utc::now().to_rfc3339())
            .await
            .map_err(|e| e.to_string());
        finalized?;
        ended?;
        state
            .store
            .refresh_session(&id)
            .await
            .map_err(|e| e.to_string())?;
        if let Err(error) = jobs::enqueue(&app, &id, "transcribe").await {
            jobs::record_failed_transcription(&app, &id, &error)?;
            return Err(error);
        }
        Ok(())
    }
    .await;
    // The microphone has stopped even if saving or queuing failed.
    let _ = app.mobile_native().end_recording_activity(&id).await;
    result
}

#[tauri::command]
pub async fn mobile_download_model(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let model_id = {
        let _guard = state.operation.lock().await;
        if recording::is_active() || state.worker.load(std::sync::atomic::Ordering::Acquire) {
            return Err("Finish recording and pause processing before downloading a model".into());
        }
        if state
            .model_busy
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return Err("A model download is already in progress".into());
        }
        state.settings.lock().unwrap().transcription_model.clone()
    };
    struct Download<'a>(&'a MobileState, &'a tauri::AppHandle);
    impl Drop for Download<'_> {
        fn drop(&mut self) {
            self.0
                .model_busy
                .store(false, std::sync::atomic::Ordering::Release);
            self.0.changed(self.1);
        }
    }
    let download = Download(&state, &app);
    state.changed(&app);
    let result = app
        .mobile_native()
        .download_model(&model_id)
        .await
        .map_err(|e| e.to_string());
    drop(download);
    jobs::kick(&app);
    result.map(|_| ())
}

#[tauri::command]
pub async fn mobile_select_model(app: tauri::AppHandle, model_id: String) -> Result<(), String> {
    if !["parakeet-v2", "parakeet-v3"].contains(&model_id.as_str()) {
        return Err("Unsupported transcription model".into());
    }
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    if recording::is_active()
        || state.worker.load(std::sync::atomic::Ordering::Acquire)
        || state.model_busy.load(std::sync::atomic::Ordering::Acquire)
    {
        return Err("Finish recording, downloading and processing before changing models".into());
    }
    let mut settings = state.settings.lock().unwrap().clone();
    settings.transcription_model = model_id;
    save_settings(&state, &settings).await?;
    *state.settings.lock().unwrap() = Settings::load(state.store.vault_base())?;
    state.changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn mobile_delete_model(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    if recording::is_active()
        || state.worker.load(std::sync::atomic::Ordering::Acquire)
        || state.model_busy.load(std::sync::atomic::Ordering::Acquire)
    {
        return Err("Finish recording and pause processing before deleting the model".into());
    }
    let model_id = state.settings.lock().unwrap().transcription_model.clone();
    app.mobile_native()
        .delete_model(&model_id)
        .await
        .map_err(|e| e.to_string())?;
    state.changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn mobile_import_audio(app: tauri::AppHandle) -> Result<String, String> {
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    if recording::is_active() {
        return Err("Finish recording before importing audio".into());
    }
    let temporary = state
        .state_dir
        .join(format!("import-{}.wav", uuid::Uuid::new_v4()));
    let imported = app
        .mobile_native()
        .import_audio(temporary.to_string_lossy().as_ref())
        .await
        .map_err(|e| e.to_string());
    let imported = match imported {
        Ok(imported) => imported,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
    };
    let title = imported.title.trim();
    let title = if title.is_empty() {
        "Imported recording"
    } else {
        title
    };
    let id = create_session(&app, title).await?;
    state
        .store
        .store_audio(&id, &temporary.to_string_lossy())
        .await
        .map_err(|e| e.to_string())?;
    jobs::enqueue(&app, &id, "transcribe").await?;
    state.changed(&app);
    Ok(id)
}

#[tauri::command]
pub async fn mobile_transcribe(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    jobs::enqueue(&app, &session_id, "transcribe").await
}

#[tauri::command]
pub async fn mobile_summarize(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    jobs::enqueue(&app, &session_id, "summary").await
}

#[tauri::command]
pub async fn mobile_cancel_job(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _operation = state.operation.lock().await;
    let mut queue = state.jobs.lock().unwrap();
    for job in queue.iter_mut().filter(|j| j.session_id == session_id) {
        job.state = "paused".into();
        job.manual_pause = true;
    }
    persist::write_json(&state.state_dir.join("jobs.json"), &*queue)?;
    state.changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn mobile_dismiss_failed_jobs(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let _operation = state.operation.lock().await;
    let mut queue = state.jobs.lock().unwrap();
    jobs::dismiss_failed(&mut queue, &state.state_dir)?;
    drop(queue);
    state.changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn mobile_save_settings(
    app: tauri::AppHandle,
    summary_provider: String,
    summary_language: String,
    summary_model: String,
    summary_base_url: String,
    api_key: Option<String>,
) -> Result<(), String> {
    if summary_language.len() > 64
        || !summary_language.split('-').all(|part| {
            !part.is_empty()
                && part.len() <= 8
                && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    {
        return Err("Unsupported summary language".into());
    }
    let state = app.state::<MobileState>();
    let _guard = state.operation.lock().await;
    let mut settings = Settings::load(state.store.vault_base())?;
    settings.summary_provider = summary_provider;
    settings.summary_language = summary_language;
    settings.summary_model = summary_model.trim().into();
    settings.summary_base_url = summary_base_url.trim().trim_end_matches('/').into();
    let enabled = settings.summary_provider != "none";
    if enabled && providers::definition(&settings.summary_provider).is_none() {
        return Err("Unsupported summary provider".into());
    }
    let previous_key = if enabled {
        app.mobile_native()
            .read_api_key(&settings.summary_provider)
            .await
            .map_err(|e| e.to_string())?
    } else {
        None
    };
    let next_key = api_key
        .as_ref()
        .map(|key| key.trim().to_owned())
        .or_else(|| previous_key.clone())
        .filter(|key| !key.is_empty());
    if enabled {
        providers::validate(&settings, next_key.is_some())?;
    }
    let config =
        hypr_storage::config::read_config(state.store.vault_base()).map_err(|e| e.to_string())?;
    if enabled && api_key.is_some() {
        app.mobile_native()
            .set_api_key(&settings.summary_provider, next_key)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut entries = config.ai_providers;
    if enabled {
        let entry = entries
            .entry(format!("llm:{}", settings.summary_provider))
            .or_insert_with(|| hypr_storage::config::AiProviderEntry {
                kind: "llm".into(),
                base_url: String::new(),
                extra: Default::default(),
            });
        entry.kind = "llm".into();
        entry.base_url = settings.summary_base_url.clone();
    }
    let mut mobile = config
        .extra
        .get("mobile")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for key in [
        "summary_provider",
        "summary_language",
        "summary_model",
        "openrouter_model",
        "summary_base_url",
    ] {
        mobile.remove(key);
    }
    let result = hypr_storage::config::update_config_values(
        state.store.vault_base(),
        std::collections::HashMap::from([
            (
                "current_llm_provider".into(),
                if enabled {
                    json!(settings.summary_provider)
                } else {
                    Value::Null
                },
            ),
            (
                "current_llm_model".into(),
                if enabled {
                    json!(settings.summary_model)
                } else {
                    Value::Null
                },
            ),
            ("ai_language".into(), json!(settings.summary_language)),
            ("ai_providers".into(), json!(entries)),
            ("mobile".into(), json!(mobile)),
        ]),
    )
    .await
    .map_err(|e| e.to_string());
    if let Err(error) = result {
        if enabled && api_key.is_some() {
            app.mobile_native()
                .set_api_key(&settings.summary_provider, previous_key)
                .await
                .map_err(|rollback| {
                    format!("{error}; API key could not be restored: {rollback}")
                })?;
        }
        return Err(error);
    }
    *state.settings.lock().unwrap() = Settings::load(state.store.vault_base())?;
    state.changed(&app);
    Ok(())
}

async fn save_settings(state: &MobileState, settings: &Settings) -> Result<(), String> {
    let config =
        hypr_storage::config::read_config(state.store.vault_base()).map_err(|e| e.to_string())?;
    let mut mobile = config
        .extra
        .get("mobile")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    mobile.insert(
        "transcription_model".into(),
        json!(settings.transcription_model),
    );
    hypr_storage::config::update_config_values(
        state.store.vault_base(),
        std::collections::HashMap::from([("mobile".into(), Value::Object(mobile))]),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_selects_only_the_sessions_visible_attachment_inventory() {
        let vault = tempfile::tempdir().unwrap();
        let session = vault.path().join("sessions/s1");
        let attachments = session.join("attachments");
        std::fs::create_dir_all(attachments.join("nested")).unwrap();
        for path in [
            attachments.join("document.pdf"),
            attachments.join("user-file.xyz"),
            attachments.join(".hidden.pdf"),
            attachments.join("nested/private.pdf"),
            session.join("notes.md"),
            session.join("summary.md"),
            session.join("loose-user-file.pdf"),
            vault.path().join("private.pdf"),
        ] {
            std::fs::write(path, b"content").unwrap();
        }
        for name in ["document.pdf", "user-file.xyz"] {
            assert_eq!(
                selected_attachment_path(
                    vault.path(),
                    "s1",
                    &format!("sessions/s1/attachments/{name}"),
                )
                .unwrap(),
                attachments.join(name),
            );
        }
        for relative in [
            "sessions/s1/notes.md",
            "sessions/s1/summary.md",
            "sessions/s1/loose-user-file.pdf",
            "sessions/s1/attachments/.hidden.pdf",
            "sessions/s1/attachments/nested/private.pdf",
            "sessions/s1/attachments/../../../private.pdf",
            "../private.pdf",
            "private.pdf",
            "sessions/s2/attachments/document.pdf",
        ] {
            assert!(
                selected_attachment_path(vault.path(), "s1", relative).is_err(),
                "Unexpected preview access: {relative}",
            );
        }
        assert!(
            selected_attachment_path(
                vault.path(),
                "s1",
                &attachments.join("document.pdf").to_string_lossy(),
            )
            .is_err(),
        );
    }
}

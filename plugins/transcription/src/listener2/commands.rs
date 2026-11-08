use std::str::FromStr;

use hypr_transcription_core::listener2 as core;

use crate::TranscriptionParams;
use crate::listener2::Listener2PluginExt;

#[tauri::command]
#[specta::specta]
pub async fn start_transcription<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    params: TranscriptionParams,
    request_id: String,
) -> Result<(), String> {
    use tauri::Manager;
    let registry = app
        .try_state::<hypr_loofah_core::local_session_work::LocalSessionWorkRegistry>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| "Local session work is unavailable".to_string())?;
    tokio::spawn(async move {
        let work = registry
            .begin(params.session_id.clone(), request_id)
            .await
            .map_err(|cause| cause.to_string())?;
        app.listener2()
            .start_transcription_owned(params, Some(work))
            .await
            .map_err(|cause| cause.to_string())
    })
    .await
    .map_err(|cause| format!("Local transcription admission is unconfirmed: {cause}"))?
}

#[tauri::command]
#[specta::specta]
pub async fn stop_transcription<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<(), String> {
    app.listener2().stop_transcription(session_id).await;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn parse_subtitle<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: String,
) -> Result<core::Subtitle, String> {
    app.listener2().parse_subtitle(path)
}

#[tauri::command]
#[specta::specta]
pub async fn is_supported_languages_batch<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    provider: String,
    model: Option<String>,
    languages: Vec<String>,
) -> Result<bool, String> {
    let languages_parsed = languages
        .iter()
        .map(|s| hypr_language::Language::from_str(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("unknown_language: {}", e))?;

    core::is_supported_languages_batch(&provider, model.as_deref(), &languages_parsed)
}

#[tauri::command]
#[specta::specta]
pub async fn suggest_providers_for_languages_batch<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    languages: Vec<String>,
) -> Result<Vec<String>, String> {
    let languages_parsed = languages
        .iter()
        .map(|s| hypr_language::Language::from_str(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("unknown_language: {}", e))?;

    Ok(core::suggest_providers_for_languages_batch(
        &languages_parsed,
    ))
}

#[tauri::command]
#[specta::specta]
pub async fn list_documented_language_codes_batch<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
) -> Result<Vec<String>, String> {
    Ok(core::list_documented_language_codes_batch())
}

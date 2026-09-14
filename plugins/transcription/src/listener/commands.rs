use owhisper_client::AdapterKind;
use std::str::FromStr;

use crate::listener::ListenerPluginExt;
use crate::{CaptureConfigUpdate, CaptureParams, CaptureSnapshot, CaptureState};
use hypr_transcript::{RenderTranscriptRequest, RenderedTranscriptSegment};
use hypr_transcription_core::listener2 as listener2_core;

#[tauri::command]
#[specta::specta]
pub async fn list_microphone_devices<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Vec<String>, String> {
    app.listener()
        .list_microphone_devices()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn get_current_microphone_device<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Option<String>, String> {
    app.listener()
        .get_current_microphone_device()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn start_capture<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    params: CaptureParams,
) -> Result<(), String> {
    use tauri::Manager;

    // Reserve the session's directory before listener-core resolves it: the
    // recorder opens absolute paths strictly before the Started lifecycle event
    // registers the recording guard, so without this lease a first-title rename
    // could move the directory inside that window. Best-effort: a store that is
    // not managed (standalone plugin use) or a failed reservation never blocks
    // capture -- the Started-event guard still applies as before.
    let store = app
        .try_state::<std::sync::Arc<hypr_vault_write::SessionStore>>()
        .map(|state| state.inner().clone());
    let session_id = params.session_id.clone();
    let leased = match &store {
        Some(store) => store.prepare_recording(&session_id).await.is_ok(),
        None => false,
    };

    let result = app
        .listener()
        .start_capture(params)
        .await
        .map_err(|e| e.to_string());
    // Release only this command's own lease on startup failure: a concurrent
    // recording's reservation must stay intact.
    if result.is_err()
        && leased
        && let Some(store) = &store
    {
        let _ = store.release_recording_prepare(&session_id).await;
    }
    result
}

#[tauri::command]
#[specta::specta]
pub async fn stop_capture<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    app.listener().stop_capture().await;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn update_capture_config<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    update: CaptureConfigUpdate,
) -> Result<(), String> {
    app.listener().update_capture_config(update).await;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn get_capture_state<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<CaptureState, String> {
    Ok(app.listener().get_capture_state().await)
}

#[tauri::command]
#[specta::specta]
pub async fn get_capture_snapshot<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<CaptureSnapshot, String> {
    Ok(app.listener().get_capture_snapshot().await)
}

#[tauri::command]
#[specta::specta]
pub async fn is_supported_languages_live<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    provider: String,
    model: Option<String>,
    languages: Vec<String>,
) -> Result<bool, String> {
    if provider == "custom" {
        return Ok(true);
    }

    let languages_parsed = languages
        .iter()
        .map(|s| hypr_language::Language::from_str(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("unknown_language: {}", e))?;

    listener2_core::is_supported_languages_live(&provider, model.as_deref(), &languages_parsed)
}

#[tauri::command]
#[specta::specta]
pub async fn suggest_providers_for_languages_live<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    languages: Vec<String>,
) -> Result<Vec<String>, String> {
    let languages_parsed = languages
        .iter()
        .map(|s| hypr_language::Language::from_str(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("unknown_language: {}", e))?;

    let all_providers = [
        AdapterKind::Soniox,
        AdapterKind::Fireworks,
        AdapterKind::Deepgram,
        AdapterKind::AssemblyAI,
        AdapterKind::OpenAI,
        AdapterKind::Gladia,
        AdapterKind::ElevenLabs,
        AdapterKind::DashScope,
        AdapterKind::Mistral,
    ];

    let mut with_support: Vec<_> = all_providers
        .iter()
        .map(|kind| {
            let support = kind.language_support_live(&languages_parsed, None);
            (*kind, support)
        })
        .filter(|(_, support)| support.is_supported())
        .collect();

    with_support.sort_by(|(_, s1), (_, s2)| s2.cmp(s1));

    let supported: Vec<String> = with_support
        .into_iter()
        .map(|(kind, _)| format!("{:?}", kind).to_lowercase())
        .collect();

    Ok(supported)
}

#[tauri::command]
#[specta::specta]
pub async fn list_documented_language_codes_live<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
) -> Result<Vec<String>, String> {
    Ok(owhisper_client::documented_language_codes_live())
}

#[tauri::command]
#[specta::specta]
pub async fn render_transcript_segments(
    params: RenderTranscriptRequest,
) -> Result<Vec<RenderedTranscriptSegment>, String> {
    Ok(hypr_transcript::render_transcript_segments(params))
}

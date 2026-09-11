use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use serde_json::Value;
use tauri_plugin_notify::NotifyPluginExt;
use tauri_plugin_settings::SettingsPluginExt;

use crate::FsSyncPluginExt;
use crate::frontmatter::ParsedDocument;
use crate::session_content::load_session_content as load_session_content_from_fs;
use crate::types::{
    ListFoldersResult, MoveSessionResult, RenameFolderResult, ScanResult, SessionContentData,
};

macro_rules! spawn_blocking {
    ($body:expr) => {
        tokio::task::spawn_blocking(move || $body)
            .await
            .map_err(|e| e.to_string())?
    };
}

fn resolve_session_dir<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: &str,
) -> Result<PathBuf, String> {
    let base = app.settings().vault_base().map_err(|e| e.to_string())?;
    crate::ext::store_backed_core(app, base.into_std_path_buf())
        .resolve_session_dir(session_id)
        .map_err(|e| e.to_string())
}

fn resolve_vault_path(base: &Path, path: &str) -> Result<PathBuf, String> {
    crate::path::resolve_path_inside_base(base, Path::new(path)).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn deserialize(input: String) -> Result<ParsedDocument, String> {
    ParsedDocument::from_str(&input).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn write_json_batch<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    items: Vec<(Value, String)>,
) -> Result<(), String> {
    let base = app.settings().vault_base().map_err(|e| e.to_string())?;
    let base_path = base
        .as_std_path()
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let items: Vec<(Value, PathBuf, PathBuf)> = items
        .into_iter()
        .map(|(json, path)| {
            resolve_vault_path(&base_path, &path)
                .map(|resolved_path| {
                    let tmp_path = crate::export::tmp_sibling_path(&resolved_path);
                    (json, resolved_path, tmp_path)
                })
                .map_err(|e| format!("failed to resolve json path {path}: {e}"))
        })
        .collect::<Result<_, _>>()?;

    let relative_paths: Vec<String> = items
        .iter()
        .flat_map(|(_, path, tmp_path)| [path, tmp_path])
        .filter_map(|path| {
            path.strip_prefix(&base_path)
                .ok()
                .and_then(|p| p.to_str())
                .map(|s| s.to_string())
        })
        .collect();

    app.notify().mark_own_writes(&relative_paths);

    let base_path_for_writes = base_path.clone();
    spawn_blocking!({
        items
            .into_par_iter()
            .try_for_each(|(json, path, tmp_path)| {
                let content = crate::json::serialize(json)
                    .map_err(|e| format!("failed to serialize json for {}: {e}", path.display()))?;
                crate::export::write_file_atomic(
                    &base_path_for_writes,
                    &path,
                    &tmp_path,
                    content.as_bytes(),
                )
                .map(|_| ())
                .map_err(|e| format!("failed to write {}: {e}", path.display()))
            })
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn write_document_batch<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    items: Vec<(ParsedDocument, String)>,
) -> Result<(), String> {
    let base = app.settings().vault_base().map_err(|e| e.to_string())?;
    let base_path = base
        .as_std_path()
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let items: Vec<(ParsedDocument, PathBuf, PathBuf)> = items
        .into_iter()
        .map(|(doc, path)| {
            resolve_vault_path(&base_path, &path)
                .map(|resolved_path| {
                    let tmp_path = crate::export::tmp_sibling_path(&resolved_path);
                    (doc, resolved_path, tmp_path)
                })
                .map_err(|e| format!("failed to resolve document path {path}: {e}"))
        })
        .collect::<Result<_, _>>()?;

    let relative_paths: Vec<String> = items
        .iter()
        .flat_map(|(_, path, tmp_path)| [path, tmp_path])
        .filter_map(|path| {
            path.strip_prefix(&base_path)
                .ok()
                .and_then(|p| p.to_str())
                .map(|s| s.to_string())
        })
        .collect();

    app.notify().mark_own_writes(&relative_paths);

    let base_path_for_writes = base_path.clone();
    spawn_blocking!({
        items.into_par_iter().try_for_each(|(doc, path, tmp_path)| {
            let content = doc
                .render()
                .map_err(|e| format!("failed to render document for {}: {e}", path.display()))?;
            crate::export::write_file_atomic(
                &base_path_for_writes,
                &path,
                &tmp_path,
                content.as_bytes(),
            )
            .map(|_| ())
            .map_err(|e| format!("failed to write {}: {e}", path.display()))
        })
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn read_document_batch(
    dir_path: String,
) -> Result<HashMap<String, ParsedDocument>, String> {
    spawn_blocking!({
        let files = crate::session::list_uuid_files(&PathBuf::from(&dir_path), "md");
        let results: HashMap<_, _> = files
            .into_par_iter()
            .filter_map(|(id, path)| {
                let content = std::fs::read_to_string(&path).ok()?;
                let doc = ParsedDocument::from_str(&content).ok()?;
                Some((id, doc))
            })
            .collect();
        Ok(results)
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn list_folders<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<ListFoldersResult, String> {
    app.fs_sync().list_folders().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn move_session<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    from_folder_path: String,
    target_folder_path: String,
) -> Result<MoveSessionResult, String> {
    let result = app
        .fs_sync()
        .move_session(&session_id, &from_folder_path, &target_folder_path)
        .map_err(|e| e.to_string())?;
    refresh_session_catalog(&app).await;
    Ok(result)
}

/// A physical session-directory move/rename happened outside the session store's
/// write path, so its location catalog is stale until rediscovery. Waiting for the
/// watcher's coalesced rebuild (~2s) leaves a window where a store write would
/// recreate the old directory; rebuilding here closes it. Best-effort: the watcher
/// still covers a failure.
async fn refresh_session_catalog<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager;
    let Some(store) = app.try_state::<std::sync::Arc<hypr_vault_write::SessionStore>>() else {
        return;
    };
    if let Err(error) = store.rebuild_index().await {
        tracing::warn!(%error, "session catalog rebuild after folder move failed");
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn create_folder<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    folder_path: String,
) -> Result<(), String> {
    app.fs_sync()
        .create_folder(&folder_path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn rename_folder<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    old_path: String,
    new_path: String,
) -> Result<RenameFolderResult, String> {
    let result = app
        .fs_sync()
        .rename_folder(&old_path, &new_path)
        .map_err(|e| e.to_string())?;
    refresh_session_catalog(&app).await;
    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn delete_folder<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    folder_path: String,
) -> Result<(), String> {
    app.fs_sync()
        .delete_folder(&folder_path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn audio_exist<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<bool, String> {
    let session_dir = resolve_session_dir(&app, &session_id)?;
    crate::audio::exists(&session_dir).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn audio_delete<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<bool, String> {
    let session_dir = resolve_session_dir(&app, &session_id)?;
    crate::audio::delete(&session_dir).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn audio_metadata<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<Option<crate::audio::AudioFileMetadata>, String> {
    let session_dir = resolve_session_dir(&app, &session_id)?;
    spawn_blocking!({ crate::audio::metadata(&session_dir).map_err(|e| e.to_string()) })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn audio_peaks<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<Option<crate::audio::AudioPeaks>, String> {
    let session_dir = resolve_session_dir(&app, &session_id)?;
    spawn_blocking!({ crate::audio::peaks(&session_dir).map_err(|e| e.to_string()) })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn audio_import<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    source_path: String,
) -> Result<String, String> {
    let session_dir = resolve_session_dir(&app, &session_id)?;
    let source_path = PathBuf::from(&source_path);
    let runtime = crate::runtime::TauriAudioImportRuntime::new(app);
    spawn_blocking!({
        crate::audio::import_to_session(&runtime, &session_id, &session_dir, &source_path)
            .map(|path| path.to_string_lossy().to_string())
            .map_err(|e| e.to_string())
    })
}

fn audio_import_source_extension(filename: &str, content_type: Option<&str>) -> String {
    let extension = Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);

    match extension.as_deref() {
        Some("wav" | "mp3" | "ogg" | "mp4" | "m4a" | "flac" | "webm" | "aac") => {
            return extension.unwrap();
        }
        Some("qta") => return "m4a".to_string(),
        _ => {}
    }

    let content_type = content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .map(str::to_ascii_lowercase);

    match content_type.as_deref() {
        Some("audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave") => "wav",
        Some("audio/mpeg" | "audio/mp3") => "mp3",
        Some("audio/ogg") => "ogg",
        Some(
            "audio/mp4" | "audio/x-m4a" | "audio/m4a" | "audio/quicktime" | "audio/x-quicktime"
            | "video/mp4" | "video/quicktime",
        ) => "m4a",
        Some("audio/flac" | "audio/x-flac") => "flac",
        Some("audio/webm") => "webm",
        Some("audio/aac" | "audio/x-aac") => "aac",
        _ => "mp3",
    }
    .to_string()
}

fn audio_import_source_path(
    session_dir: &Path,
    filename: &str,
    content_type: Option<&str>,
) -> PathBuf {
    let extension = audio_import_source_extension(filename, content_type);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    session_dir.join(format!(
        "audio-upload-{}-{}.{}",
        std::process::id(),
        nonce,
        extension
    ))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn audio_import_data<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    data: Vec<u8>,
    filename: String,
    content_type: Option<String>,
) -> Result<String, String> {
    let session_dir = resolve_session_dir(&app, &session_id)?;
    let runtime = crate::runtime::TauriAudioImportRuntime::new(app);
    spawn_blocking!({
        std::fs::create_dir_all(&session_dir).map_err(|e| e.to_string())?;

        let source_path =
            audio_import_source_path(&session_dir, &filename, content_type.as_deref());
        std::fs::write(&source_path, data).map_err(|e| e.to_string())?;

        let result =
            crate::audio::import_to_session(&runtime, &session_id, &session_dir, &source_path)
                .map(|path| path.to_string_lossy().to_string())
                .map_err(|e| e.to_string());

        if source_path.exists() {
            let _ = std::fs::remove_file(&source_path);
        }

        result
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn audio_source_metadata(
    source_path: String,
) -> Result<crate::audio::AudioSourceMetadata, String> {
    spawn_blocking!({
        crate::audio::source_metadata(&PathBuf::from(source_path)).map_err(|e| e.to_string())
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn audio_path<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<String, String> {
    let session_dir = resolve_session_dir(&app, &session_id)?;
    crate::audio::path(&session_dir)
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "audio_path_not_found".to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn session_dir<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<String, String> {
    resolve_session_dir(&app, &session_id).map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn load_session_content<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<SessionContentData, String> {
    let session_dir = resolve_session_dir(&app, &session_id)?;
    spawn_blocking!({ Ok(load_session_content_from_fs(&session_id, &session_dir)) })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn delete_session_folder<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<(), String> {
    let session_dir = resolve_session_dir(&app, &session_id)?;
    crate::session::delete_session_dir(&session_dir).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn scan_and_read<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scan_dir: String,
    file_patterns: Vec<String>,
    recursive: bool,
    path_filter: Option<String>,
) -> Result<ScanResult, String> {
    let base = app.settings().vault_base().map_err(|e| e.to_string())?;
    let base_path = base
        .as_std_path()
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let scan_dir = resolve_vault_path(&base_path, &scan_dir)?;
    spawn_blocking!({
        Ok(crate::scan::scan_and_read(
            &scan_dir,
            &base_path,
            &file_patterns,
            recursive,
            path_filter.as_deref(),
        ))
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn chat_dir<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    chat_group_id: String,
) -> Result<String, String> {
    let base = app.settings().vault_base().map_err(|e| e.to_string())?;
    Ok(base.join("chats").join(&chat_group_id).to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn entity_dir<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    dir_name: String,
) -> Result<String, String> {
    let base = app.settings().vault_base().map_err(|e| e.to_string())?;
    Ok(base.join(&dir_name).to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn attachment_save<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    data: Vec<u8>,
    filename: String,
) -> Result<crate::AttachmentSaveResult, String> {
    spawn_blocking!({
        app.fs_sync()
            .attachment_save(&session_id, &data, &filename)
            .map_err(|e| e.to_string())
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn attachment_import_path<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    source_path: String,
) -> Result<crate::AttachmentInfo, String> {
    spawn_blocking!({
        app.fs_sync()
            .attachment_import_path(&session_id, std::path::Path::new(&source_path))
            .map_err(|e| e.to_string())
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn audio_collect_import_sources(
    paths: Vec<String>,
) -> Result<Vec<crate::AudioImportSourceInfo>, String> {
    spawn_blocking!({ collect_audio_import_sources(&paths).map_err(|e| e.to_string()) })
}

fn collect_audio_import_sources(
    paths: &[String],
) -> std::io::Result<Vec<crate::AudioImportSourceInfo>> {
    let mut sources = Vec::new();
    for path in paths {
        let path = std::path::PathBuf::from(path);
        if !path.is_absolute() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "audio source path must be absolute",
            ));
        }
        collect_audio_import_source(&path, &mut sources)?;
    }
    Ok(sources)
}

fn collect_audio_import_source(
    path: &std::path::Path,
    sources: &mut Vec<crate::AudioImportSourceInfo>,
) -> std::io::Result<()> {
    let link_metadata = std::fs::symlink_metadata(path)?;
    let metadata = std::fs::metadata(path)?;
    if metadata.is_file() {
        let name = match path.file_name().and_then(|name| name.to_str()) {
            Some(name) if !name.starts_with('.') => name,
            _ => return Ok(()),
        };
        if is_audio_import_source(name) {
            sources.push(crate::AudioImportSourceInfo {
                path: path.to_string_lossy().to_string(),
                name: name.to_string(),
                size: metadata.len(),
            });
        }
        return Ok(());
    }
    if !metadata.is_dir() || link_metadata.file_type().is_symlink() {
        return Ok(());
    }

    let mut entries = std::fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        collect_audio_import_source(&entry.path(), sources)?;
    }
    Ok(())
}

fn is_audio_import_source(name: &str) -> bool {
    matches!(
        std::path::Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("wav" | "mp3" | "ogg" | "mp4" | "m4a" | "flac" | "webm" | "aac" | "qta")
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn attachment_dir<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<String, String> {
    spawn_blocking!({
        app.fs_sync()
            .attachment_dir(&session_id)
            .map(|path| path.to_string_lossy().to_string())
            .map_err(|e| e.to_string())
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn attachment_list<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<Vec<crate::AttachmentInfo>, String> {
    spawn_blocking!({
        app.fs_sync()
            .attachment_list(&session_id)
            .map_err(|e| e.to_string())
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn attachment_read<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    attachment_id: String,
) -> Result<Vec<u8>, String> {
    spawn_blocking!({
        app.fs_sync()
            .attachment_read(&session_id, &attachment_id)
            .map_err(|e| e.to_string())
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn attachment_remove<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    attachment_id: String,
) -> Result<(), String> {
    spawn_blocking!({
        app.fs_sync()
            .attachment_remove(&session_id, &attachment_id)
            .map_err(|e| e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_import_source_extension_uses_supported_filename_extension() {
        assert_eq!(
            audio_import_source_extension("recording.WEBM", Some("audio/mp4")),
            "webm"
        );
        assert_eq!(audio_import_source_extension("recording.aac", None), "aac");
    }

    #[test]
    fn audio_import_source_extension_recognizes_voice_memos_transfers() {
        assert_eq!(audio_import_source_extension("Brian Shin.qta", None), "m4a");
        assert_eq!(
            audio_import_source_extension("Brian Shin", Some("audio/mp4; codecs=alac")),
            "m4a"
        );
        assert_eq!(
            audio_import_source_extension("Brian Shin", Some("audio/quicktime")),
            "m4a"
        );
    }

    #[test]
    fn audio_collect_import_sources_expands_folders_in_order() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("recordings");
        std::fs::create_dir_all(folder.join("nested")).unwrap();
        std::fs::write(folder.join("b.mp3"), b"bb").unwrap();
        std::fs::write(folder.join("a.wav"), b"a").unwrap();
        std::fs::write(folder.join("notes.txt"), b"skip").unwrap();
        std::fs::write(folder.join("nested").join("c.m4a"), b"ccc").unwrap();

        let sources =
            collect_audio_import_sources(&[folder.to_string_lossy().to_string()]).unwrap();

        assert_eq!(
            sources
                .iter()
                .map(|source| (source.name.as_str(), source.size))
                .collect::<Vec<_>>(),
            vec![("a.wav", 1), ("b.mp3", 2), ("c.m4a", 3)]
        );
    }
}

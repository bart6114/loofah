use std::path::PathBuf;

#[tauri::command]
#[specta::specta]
pub(crate) async fn export<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    path: PathBuf,
    input: crate::ExportInput,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || hypr_export_core::export_pdf(path, input))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn export_text(
    path: PathBuf,
    input: crate::ExportInput,
    format: crate::TextFormat,
    labels: crate::ExportLabels,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::write(path, hypr_export_core::render_text(&input, format, &labels))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

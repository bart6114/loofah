use std::path::PathBuf;

#[tauri::command]
#[specta::specta]
pub(crate) async fn export<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: PathBuf,
    mut input: crate::ExportInput,
) -> Result<(), String> {
    use tauri::Manager;
    let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
    if let Some(connection) =
        app.try_state::<std::sync::Arc<hypr_vault_access::connection::ConnectionState>>()
        && connection.active()
    {
        for (index, attachment) in input.attachments.iter_mut().enumerate() {
            if attachment.path.starts_with("https://") || attachment.path.starts_with("http://") {
                let suffix = std::path::Path::new(&attachment.src)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("bin");
                if !["png", "jpg", "jpeg", "gif", "svg", "webp"]
                    .contains(&suffix.to_ascii_lowercase().as_str())
                {
                    continue;
                }
                let bytes = connection
                    .read_media_url(&attachment.path)
                    .await
                    .map_err(|error| error.to_string())?;
                let local = temporary.path().join(format!("{index}.{suffix}"));
                std::fs::write(&local, bytes).map_err(|error| error.to_string())?;
                attachment.path = local.to_string_lossy().into_owned();
            }
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        let _temporary = temporary;
        hypr_export_core::export_pdf(path, input)
    })
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

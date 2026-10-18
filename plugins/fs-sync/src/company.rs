use std::sync::Arc;

use hypr_vault_access::connection::ConnectionState;
use tauri::Manager;

pub fn connection<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<Arc<ConnectionState>> {
    app.try_state::<Arc<ConnectionState>>()
        .filter(|state| state.active())
        .map(|state| state.inner().clone())
}

pub async fn attachment_info(
    connection: &ConnectionState,
    session_id: &str,
    artifact: &serde_json::Value,
) -> Result<crate::AttachmentInfo, String> {
    let name = artifact["filename"]
        .as_str()
        .ok_or("invalid attachment filename")?;
    let key = artifact["key"].as_str().ok_or("invalid attachment key")?;
    Ok(crate::AttachmentInfo {
        attachment_id: name.into(),
        path: connection
            .media_url(session_id, key)
            .await
            .map_err(|error| error.to_string())?,
        extension: name
            .rsplit_once('.')
            .map(|(_, extension)| extension)
            .unwrap_or("")
            .into(),
        size: artifact["size"].as_u64().unwrap_or(0),
        modified_at: String::new(),
    })
}

pub async fn save(
    connection: &ConnectionState,
    session_id: &str,
    source: &std::path::Path,
    filename: &str,
) -> Result<crate::AttachmentInfo, String> {
    hypr_vault_access::validate_id(session_id).map_err(|error| error.to_string())?;
    let filename = std::path::Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.starts_with('.'))
        .ok_or("invalid attachment filename")?;
    let artifacts = connection
        .artifacts(session_id)
        .await
        .map_err(|error| error.to_string())?;
    let mut name = filename.to_string();
    let mut counter = 1;
    while artifacts
        .iter()
        .any(|artifact| artifact["filename"] == name)
    {
        let path = std::path::Path::new(filename);
        let stem = path.file_stem().unwrap().to_string_lossy();
        name = match path.extension().and_then(|extension| extension.to_str()) {
            Some(extension) => format!("{stem} {counter}.{extension}"),
            None => format!("{stem} {counter}"),
        };
        counter += 1;
    }
    let base = connection.scratch_base().ok_or("company vault changed")?;
    let target = hypr_vault_access::filesystem::safe_path(
        &base,
        &format!("sessions/{session_id}/attachments/{name}"),
    )
    .map_err(|error| error.to_string())?;
    tokio::fs::create_dir_all(target.parent().unwrap())
        .await
        .map_err(|error| error.to_string())?;
    if source != target {
        tokio::fs::copy(source, &target)
            .await
            .map_err(|error| error.to_string())?;
    }
    connection
        .enqueue_upload(session_id, &target, &name, false)
        .map_err(|error| error.to_string())?;
    connection
        .upload(session_id, &target, &name, false)
        .await
        .map_err(|error| error.to_string())?;
    let artifact = connection
        .artifacts(session_id)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|artifact| artifact["filename"] == name)
        .ok_or("uploaded attachment was not found")?;
    attachment_info(connection, session_id, &artifact).await
}

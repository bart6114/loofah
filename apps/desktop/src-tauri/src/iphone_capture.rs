use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime};

use crate::session_store::{SessionStore, iphone_capture::CaptureStage};

#[derive(serde::Serialize, specta::Type)]
pub struct CaptureReceiverStatus {
    available: bool,
    enabled: bool,
    receiver: Option<String>,
    vault: String,
}

fn store<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<SessionStore>, String> {
    let store = app
        .try_state::<Arc<SessionStore>>()
        .ok_or("No destination vault is available.")?
        .inner()
        .clone();
    store.ensure_ready().map_err(|error| error.to_string())?;
    Ok(store)
}

#[tauri::command]
#[specta::specta]
pub async fn iphone_capture_receiver<R: Runtime>(
    app: AppHandle<R>,
) -> Result<CaptureReceiverStatus, String> {
    let store = store(&app)?;
    let vault = store.vault_base().to_string_lossy().into_owned();
    #[cfg(target_os = "macos")]
    {
        let reply = mac::request("status", &vault, serde_json::Value::Null).await?;
        Ok(CaptureReceiverStatus {
            available: reply.available,
            enabled: reply.enabled,
            receiver: reply.receiver,
            vault,
        })
    }
    #[cfg(not(target_os = "macos"))]
    Ok(CaptureReceiverStatus {
        available: false,
        enabled: false,
        receiver: None,
        vault,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn iphone_capture_set_receiver<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
    expected_receiver: Option<String>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let store = store(&app)?;
        mac::request(
            if enabled { "enable" } else { "disable" },
            &store.vault_base().to_string_lossy(),
            serde_json::json!({ "expectedReceiver": expected_receiver }),
        )
        .await?;
        mac::CHANGED.notify_one();
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, enabled, expected_receiver);
        Err("iPhone captures require a Mac.".into())
    }
}

#[tauri::command]
#[specta::specta]
pub fn iphone_capture_jobs<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<(String, CaptureStage)>, String> {
    Ok(store(&app)?.iphone_capture_jobs())
}

#[tauri::command]
#[specta::specta]
pub fn is_iphone_capture<R: Runtime>(app: AppHandle<R>, id: String) -> Result<bool, String> {
    Ok(store(&app)?.is_iphone_capture(&id))
}

#[tauri::command]
#[specta::specta]
pub async fn iphone_capture_audio<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<Option<String>, String> {
    let store = store(&app)?;
    if !store.is_iphone_capture(&id) {
        return Ok(None);
    }
    let path = store
        .vault_base()
        .join(
            store
                .session_dir(&id)
                .await
                .map_err(|error| error.to_string())?,
        )
        .join("audio.wav");
    let available = tokio::task::spawn_blocking({
        let path = path.clone();
        move || path.is_file()
    })
    .await
    .map_err(|error| error.to_string())?;
    Ok(available.then(|| path.to_string_lossy().into_owned()))
}

#[tauri::command]
#[specta::specta]
pub async fn iphone_capture_advance<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    expected: CaptureStage,
) -> Result<(), String> {
    store(&app)?
        .advance_iphone_capture(&id, expected)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
pub use mac::{is_entitled, spawn};

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{CStr, CString, c_char};
    use std::path::Path;
    use std::time::Duration;

    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};
    use tokio::sync::{Mutex, Notify};

    use super::*;

    unsafe extern "C" {
        fn loofah_capture_is_entitled() -> bool;
        fn loofah_capture_observe(callback: extern "C" fn());
        fn loofah_capture_request(request: *const c_char) -> *mut c_char;
        fn loofah_capture_free(reply: *mut c_char);
    }

    pub static CHANGED: Notify = Notify::const_new();
    static NATIVE: Mutex<()> = Mutex::const_new(());

    extern "C" fn changed() {
        CHANGED.notify_one();
    }

    #[derive(Deserialize)]
    pub struct Reply {
        pub available: bool,
        pub enabled: bool,
        pub receiver: Option<String>,
        captures: Vec<String>,
        receipt: Option<Receipt>,
        account: Option<String>,
        error: Option<String>,
    }

    #[derive(Serialize, Deserialize)]
    struct Receipt {
        id: String,
        digest: String,
    }

    pub async fn request(operation: &str, vault: &str, values: Value) -> Result<Reply, String> {
        let mut data = values.as_object().cloned().unwrap_or_default();
        data.insert("operation".into(), operation.into());
        data.insert("vault".into(), vault.into());
        let request =
            CString::new(Value::Object(data).to_string()).map_err(|error| error.to_string())?;
        let lock = NATIVE.lock().await;
        let result = tokio::task::spawn_blocking(move || {
            // The Swift bridge returns a strdup allocation owned by the caller.
            let raw = unsafe { loofah_capture_request(request.as_ptr()) };
            if raw.is_null() {
                return Err("iCloud did not return a response.".into());
            }
            let bytes = unsafe { CStr::from_ptr(raw) }.to_bytes();
            let reply: Result<Reply, _> = serde_json::from_slice(bytes);
            unsafe { loofah_capture_free(raw) };
            let reply = reply.map_err(|error| error.to_string())?;
            if let Some(error) = reply.error {
                return Err(error);
            }
            Ok(reply)
        })
        .await
        .map_err(|error| error.to_string())?;
        drop(lock);
        result
    }

    pub fn is_entitled() -> bool {
        unsafe { loofah_capture_is_entitled() }
    }

    pub fn spawn(app: AppHandle) {
        unsafe { loofah_capture_observe(changed) };
        tauri::async_runtime::spawn(async move {
            let mut retry = tokio::time::interval(Duration::from_secs(20));
            loop {
                tokio::select! {
                    _ = retry.tick() => {},
                    _ = CHANGED.notified() => {},
                }
                // Coalesce metadata notifications caused by the same transfer.
                tokio::time::sleep(Duration::from_millis(500)).await;
                if let Ok(store) = store(&app) {
                    if let Err(error) = receive(&store).await {
                        tracing::debug!(%error, "iPhone capture inbox is pending");
                    }
                }
            }
        });
    }

    async fn receive(store: &SessionStore) -> Result<(), String> {
        let vault = store
            .vault_base()
            .to_str()
            .ok_or("Invalid destination vault path.")?;
        let reply = request("list", vault, Value::Null).await?;
        for id in reply.captures {
            if let Err(error) = import(store, vault, &id).await {
                tracing::debug!(capture_id = %id, %error, "iPhone capture is pending");
            }
        }
        Ok(())
    }

    async fn import(store: &SessionStore, vault: &str, id: &str) -> Result<(), String> {
        let stage = tempfile::tempdir().map_err(|error| error.to_string())?;
        let reply = request(
            "stage",
            vault,
            json!({ "id": id, "destination": stage.path() }),
        )
        .await?;
        let Some(receipt) = reply.receipt else {
            return Ok(());
        };
        let account = reply.account.ok_or("iCloud account is unavailable.")?;
        let (meta, note, audio) = read_capture(stage.path(), &receipt).await?;
        if !request("status", vault, json!({ "account": account }))
            .await?
            .enabled
        {
            return Ok(());
        }
        store
            .commit_iphone_capture(meta, note, audio, receipt.digest.clone())
            .await
            .map_err(|error| error.to_string())?;
        request(
            "finish",
            vault,
            json!({ "receipt": receipt, "account": account }),
        )
        .await?;
        Ok(())
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Capture {
        id: String,
        created_at: f64,
        finished_at: f64,
        note: String,
        recording: bool,
    }

    async fn read_capture(
        stage: &Path,
        receipt: &Receipt,
    ) -> Result<
        (
            crate::session_store::SessionMeta,
            String,
            Option<std::path::PathBuf>,
        ),
        String,
    > {
        let stage = stage.to_owned();
        let digest = receipt.digest.clone();
        let id = receipt.id.to_ascii_lowercase();
        tokio::task::spawn_blocking(move || {
            if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("Invalid iPhone capture checksum.".into());
            }
            let bytes =
                std::fs::read(stage.join("capture.json")).map_err(|error| error.to_string())?;
            let capture: Capture =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            if !id.eq_ignore_ascii_case(&capture.id) {
                return Err("iPhone capture IDs do not match.".into());
            }
            let created_at = timestamp(capture.created_at)?;
            let finished_at = timestamp(capture.finished_at)?;
            let audio = if capture.recording {
                let path = stage.join("audio.wav");
                let reader = hound::WavReader::open(&path).map_err(|error| error.to_string())?;
                if reader.duration() == 0
                    || reader.spec().channels != 1
                    || reader.spec().bits_per_sample != 16
                {
                    return Err("Invalid iPhone capture audio.".into());
                }
                Some(path)
            } else {
                None
            };
            let meta = serde_json::from_value(json!({
                "id": id,
                "title": "",
                "created_at": created_at,
                "started_at": capture.recording.then_some(created_at),
                "ended_at": capture.recording.then_some(finished_at),
                "tags": [],
            }))
            .map_err(|error| error.to_string())?;
            Ok((meta, capture.note, audio))
        })
        .await
        .map_err(|error| error.to_string())?
    }

    fn timestamp(seconds_since_2001: f64) -> Result<String, String> {
        let millis = (seconds_since_2001 + 978_307_200.0) * 1_000.0;
        chrono::DateTime::from_timestamp_millis(millis.round() as i64)
            .map(|time| time.to_rfc3339())
            .ok_or_else(|| "Invalid iPhone capture timestamp.".into())
    }
}

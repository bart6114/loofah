use super::{MobileState, Settings, recording};
use serde_json::{Value, json};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use tauri::{Emitter, Manager};
use tauri_plugin_mobile_native::MobileNativeExt;
use tokio::sync::oneshot;

pub fn definitions() -> &'static Vec<Value> {
    static DEFINITIONS: OnceLock<Vec<Value>> = OnceLock::new();
    DEFINITIONS.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../../packages/ai-providers/src/providers.json"
        ))
        .expect("shared provider catalog must be valid")
    })
}

pub fn definition(id: &str) -> Option<&'static Value> {
    definitions().iter().find(|p| p["id"].as_str() == Some(id))
}

pub fn validate(settings: &Settings, has_key: bool) -> Result<(), String> {
    if settings.summary_provider == "none" || settings.summary_provider.is_empty() {
        return Err("Choose a summary provider in Settings".into());
    }
    let provider =
        definition(&settings.summary_provider).ok_or("Choose a supported summary provider")?;
    if provider["mobileSupported"] == false {
        return Err(
            "This connection requires the desktop app. Choose another provider on iPhone.".into(),
        );
    }
    if settings.summary_model.trim().is_empty() || settings.summary_model.len() > 512 {
        return Err("Choose a summary model".into());
    }
    let url = reqwest::Url::parse(&settings.summary_base_url)
        .map_err(|_| "Enter a valid HTTP or HTTPS server address")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Use an HTTP or HTTPS server address without credentials, query, or fragment".into(),
        );
    }
    let host = url.host_str().unwrap_or_default().trim_matches(['[', ']']);
    if host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|address| address.is_loopback())
    {
        return Err("Use your computer’s network address. Localhost points to this iPhone.".into());
    }
    let requires_key = provider["requirements"]
        .as_array()
        .is_some_and(|requirements| {
            requirements.iter().any(|requirement| {
                requirement["fields"]
                    .as_array()
                    .is_some_and(|fields| fields.iter().any(|field| field == "api_key"))
            })
        });
    if requires_key && !has_key {
        return Err("Enter this provider’s API key on your iPhone".into());
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq)]
struct Consent {
    version: u32,
    provider: String,
    base_url: String,
    model: String,
}

impl Consent {
    fn for_settings(settings: &Settings) -> Self {
        Self {
            version: 1,
            provider: settings.summary_provider.clone(),
            base_url: settings
                .summary_base_url
                .trim()
                .trim_end_matches('/')
                .into(),
            model: settings.summary_model.trim().into(),
        }
    }
}

pub fn has_consent(directory: &std::path::Path, settings: &Settings) -> bool {
    // Permission belongs to this installation, never to a synced vault or queued job.
    std::fs::read(directory.join("ai-consent.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Option<Consent>>(&bytes).ok())
        .flatten()
        .is_some_and(|consent| consent == Consent::for_settings(settings))
}

pub fn save_consent(
    directory: &std::path::Path,
    settings: &Settings,
    allowed: bool,
) -> Result<(), String> {
    super::persist::write_json(
        &directory.join("ai-consent.json"),
        &allowed.then(|| Consent::for_settings(settings)),
    )
}

pub fn require_consent(directory: &std::path::Path, settings: &Settings) -> Result<(), String> {
    if !has_consent(directory, settings) {
        return Err("Review AI data sharing and give permission in Settings before creating summaries or titles.".into());
    }
    Ok(())
}

pub async fn availability(app: &tauri::AppHandle, settings: &Settings) -> Result<(), String> {
    let state = app.state::<MobileState>();
    require_consent(&state.state_dir, settings)?;
    if Settings::load(state.store.vault_base())? != *settings {
        return Err("Summary settings changed. Retry with the selected provider.".into());
    }

    let key = if definition(&settings.summary_provider).is_some() {
        app.mobile_native()
            .has_api_key(&settings.summary_provider)
            .await
            .map_err(|e| e.to_string())?
    } else {
        false
    };
    validate(settings, key)
}

#[tauri::command]
pub async fn mobile_provider_api_key(
    app: tauri::AppHandle,
    provider_id: String,
) -> Result<Option<String>, String> {
    if !definition(&provider_id).is_some_and(|p| p["mobileSupported"] != false) {
        return Err("Unsupported summary provider".into());
    }
    app.mobile_native()
        .read_api_key(&provider_id)
        .await
        .map_err(|e| e.to_string())
}

struct Pending {
    id: String,
    reply: oneshot::Sender<Result<String, String>>,
}

#[derive(Default)]
pub struct Bridge {
    pub ready: AtomicBool,
    pending: Mutex<Option<Pending>>,
}

impl Bridge {
    fn complete(&self, id: &str, result: Result<String, String>) -> Result<(), String> {
        let mut pending = self.pending.lock().unwrap();
        if pending.as_ref().is_none_or(|p| p.id != id) {
            return Err("This summary request is no longer active".into());
        }
        let request = pending.take().unwrap();
        let _ = request.reply.send(result);
        Ok(())
    }
}

pub fn abort_active(app: &tauri::AppHandle) {
    let pending = app.state::<Bridge>().pending.lock().unwrap().take();
    if let Some(pending) = pending {
        let _ = app.emit("mobile-summary-abort", &pending.id);
    }
}

#[tauri::command]
pub fn mobile_summary_bridge_ready(app: tauri::AppHandle, ready: bool) {
    let bridge = app.state::<Bridge>();
    bridge.ready.store(ready, Ordering::Release);
    if !ready {
        bridge.pending.lock().unwrap().take();
    }
    if ready {
        super::jobs::kick(&app);
    }
}

#[tauri::command]
pub fn mobile_summary_complete(
    app: tauri::AppHandle,
    request_id: String,
    text: Option<String>,
    error: Option<String>,
) -> Result<(), String> {
    let result = match (text, error) {
        (Some(text), None) if !text.trim().is_empty() && text.len() <= 256_000 => Ok(text),
        (_, Some(error)) => Err(error.chars().take(1500).collect()),
        _ => Err("The provider returned no usable summary".into()),
    };
    app.state::<Bridge>().complete(&request_id, result)
}

pub async fn generate(
    app: &tauri::AppHandle,
    session_id: &str,
    settings: &Settings,
    instructions: &str,
    input: &str,
    max_output_tokens: Option<u32>,
) -> Result<Option<String>, String> {
    let state = app.state::<MobileState>();
    let operation = state.operation.lock().await;
    availability(app, settings).await?;
    let key = app
        .mobile_native()
        .read_api_key(&settings.summary_provider)
        .await
        .map_err(|e| e.to_string())?;
    let bridge = app.state::<Bridge>();
    if !bridge.ready.load(Ordering::Acquire) {
        return Ok(None);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let (reply, mut receiver) = oneshot::channel();
    {
        let mut pending = bridge.pending.lock().unwrap();
        if pending.is_some() {
            return Err("A summary request is already active".into());
        }
        *pending = Some(Pending {
            id: id.clone(),
            reply,
        });
    }
    struct Request<'a> {
        app: &'a tauri::AppHandle,
        id: String,
    }
    impl Drop for Request<'_> {
        fn drop(&mut self) {
            let bridge = self.app.state::<Bridge>();
            let mut pending = bridge.pending.lock().unwrap();
            if pending.as_ref().is_some_and(|p| p.id == self.id) {
                pending.take();
            }
            let _ = self.app.emit("mobile-summary-abort", &self.id);
        }
    }
    let _request = Request {
        app,
        id: id.clone(),
    };
    app.emit("mobile-summary-request", json!({
        "id": id, "instructions": instructions, "input": input, "maxOutputTokens": max_output_tokens,
        "connection": { "providerId": settings.summary_provider, "modelId": settings.summary_model,
            "baseUrl": settings.summary_base_url, "apiKey": key.unwrap_or_default() }
    })).map_err(|e| e.to_string())?;
    drop(operation);
    let timeout = tokio::time::sleep(std::time::Duration::from_secs(240));
    tokio::pin!(timeout);
    loop {
        tokio::select! {
            result = &mut receiver => return match result {
                Ok(result) => result.map(Some),
                Err(_) => Ok(None),
            },
            _ = &mut timeout => return Err("The summary provider took too long. Check the connection and retry.".into()),
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                let state = app.state::<MobileState>();
                if !bridge.ready.load(Ordering::Acquire) || recording::is_active()
                    || !recording::FOREGROUND.load(Ordering::Acquire)
                    || state.jobs.lock().unwrap().iter().any(|job| job.session_id == session_id && job.kind != "transcribe" && job.manual_pause)
                { return Ok(None); }
                if *state.settings.lock().unwrap() != *settings || !has_consent(&state.state_dir, settings) {
                    return Err("Summary settings changed. Retry with the selected provider.".into());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_is_explicit_local_revocable_and_bound_to_the_recipient() {
        let directory = tempfile::tempdir().unwrap();
        let settings = Settings {
            summary_provider: "openrouter".into(),
            summary_model: "provider/model".into(),
            summary_base_url: "https://openrouter.ai/api/v1".into(),
            ..Default::default()
        };
        assert!(require_consent(directory.path(), &settings).is_err());
        save_consent(directory.path(), &settings, true).unwrap();
        assert!(require_consent(directory.path(), &settings).is_ok());
        let other_device = tempfile::tempdir().unwrap();
        assert!(!has_consent(other_device.path(), &settings));
        let mut changed = settings.clone();
        changed.summary_model = "other/model".into();
        assert!(!has_consent(directory.path(), &changed));
        changed = settings.clone();
        changed.summary_base_url = "https://another.example/v1".into();
        assert!(!has_consent(directory.path(), &changed));
        changed = settings.clone();
        changed.summary_provider = "custom".into();
        assert!(!has_consent(directory.path(), &changed));
        save_consent(directory.path(), &settings, false).unwrap();
        assert!(require_consent(directory.path(), &settings).is_err());
        std::fs::write(directory.path().join("ai-consent.json"), b"broken").unwrap();
        assert!(!has_consent(directory.path(), &settings));
        let mut old = Consent::for_settings(&settings);
        old.version = 0;
        super::super::persist::write_json(&directory.path().join("ai-consent.json"), &Some(old))
            .unwrap();
        assert!(!has_consent(directory.path(), &settings));
    }

    #[test]
    fn stale_reply_cannot_complete_a_new_request() {
        let bridge = Bridge::default();
        let (reply, mut receiver) = oneshot::channel();
        *bridge.pending.lock().unwrap() = Some(Pending {
            id: "new".into(),
            reply,
        });
        assert!(bridge.complete("old", Ok("stale".into())).is_err());
        assert!(receiver.try_recv().is_err());
        bridge.complete("new", Ok("summary".into())).unwrap();
        assert_eq!(receiver.try_recv().unwrap().unwrap(), "summary");
    }

    #[test]
    fn validates_catalog_requirements_and_rejects_phone_localhost() {
        let mut settings = Settings {
            summary_provider: "openrouter".into(),
            summary_model: "provider/model".into(),
            summary_base_url: "https://openrouter.ai/api/v1".into(),
            ..Default::default()
        };
        assert!(validate(&settings, false).is_err());
        assert!(validate(&settings, true).is_ok());
        settings.summary_provider = "ollama".into();
        settings.summary_base_url = "http://127.0.0.1:11434/v1".into();
        assert!(
            validate(&settings, false)
                .unwrap_err()
                .contains("Localhost")
        );
        settings.summary_base_url = "http://192.168.1.20:11434/v1".into();
        assert!(validate(&settings, false).is_ok());
        settings.summary_base_url = "https://key:secret@example.com/v1".into();
        assert!(validate(&settings, true).is_err());
        settings.summary_provider = "chatgpt_subscription".into();
        assert!(validate(&settings, true).unwrap_err().contains("desktop"));
    }
}

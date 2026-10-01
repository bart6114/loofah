use serde::{Serialize, de::DeserializeOwned};
use tauri::{AppHandle, Runtime, plugin::PluginApi};

use crate::models::*;

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_mobile_native);

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> crate::Result<MobileNative<R>> {
    #[cfg(target_os = "ios")]
    {
        Ok(MobileNative(
            api.register_ios_plugin(init_plugin_mobile_native)?,
        ))
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = api;
        Ok(MobileNative(std::marker::PhantomData))
    }
}

pub struct MobileNative<R: Runtime>(
    #[cfg(target_os = "ios")] tauri::plugin::PluginHandle<R>,
    #[cfg(not(target_os = "ios"))] std::marker::PhantomData<fn() -> R>,
);

impl<R: Runtime> MobileNative<R> {
    async fn call<T: DeserializeOwned>(
        &self,
        command: &str,
        payload: impl Serialize,
    ) -> crate::Result<T> {
        #[cfg(target_os = "ios")]
        {
            self.0
                .run_mobile_plugin_async(command, payload)
                .await
                .map_err(Into::into)
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = (command, payload);
            Err(crate::Error::Unsupported)
        }
    }

    pub async fn start_recording(&self) -> crate::Result<()> {
        self.call("startRecording", serde_json::json!({})).await
    }

    pub async fn stop_recording(&self) -> crate::Result<()> {
        self.call("stopRecording", serde_json::json!({})).await
    }

    pub async fn begin_recording_activity(
        &self,
        session_id: impl AsRef<str>,
        title: impl AsRef<str>,
    ) -> crate::Result<()> {
        self.call(
            "beginRecordingActivity",
            serde_json::json!({ "sessionId": session_id.as_ref(), "title": title.as_ref() }),
        )
        .await
    }

    pub async fn end_recording_activity(&self, session_id: impl AsRef<str>) -> crate::Result<()> {
        self.call(
            "endRecordingActivity",
            serde_json::json!({ "sessionId": session_id.as_ref() }),
        )
        .await
    }

    pub async fn preview_file(&self, path: impl AsRef<str>) -> crate::Result<()> {
        self.call("previewFile", serde_json::json!({ "path": path.as_ref() }))
            .await
    }

    pub async fn model_status(&self, model_id: &str) -> crate::Result<NativeModelStatus> {
        self.call("modelStatus", serde_json::json!({ "modelId": model_id }))
            .await
    }

    pub async fn download_model(&self, model_id: &str) -> crate::Result<NativeModelStatus> {
        self.call("downloadModel", serde_json::json!({ "modelId": model_id }))
            .await
    }

    pub async fn delete_model(&self, model_id: &str) -> crate::Result<NativeModelStatus> {
        self.call("deleteModel", serde_json::json!({ "modelId": model_id }))
            .await
    }

    pub async fn transcribe_window(
        &self,
        model_id: &str,
        path: impl AsRef<str>,
        offset_seconds: f64,
        duration_seconds: f64,
    ) -> crate::Result<Vec<NativeWord>> {
        if !offset_seconds.is_finite()
            || !duration_seconds.is_finite()
            || offset_seconds < 0.0
            || duration_seconds <= 0.0
            || duration_seconds > 30.0
        {
            return Err(crate::Error::InvalidArgument(
                "Transcription windows require a nonnegative offset and 0–30 second duration"
                    .into(),
            ));
        }
        self.call(
            "transcribeWindow",
            serde_json::json!({ "modelId": model_id, "path": path.as_ref(), "offsetSeconds": offset_seconds, "durationSeconds": duration_seconds }),
        ).await
    }

    pub async fn import_audio(
        &self,
        destination_path: impl AsRef<str>,
    ) -> crate::Result<Option<NativeImportResult>> {
        self.call(
            "importAudio",
            serde_json::json!({ "destinationPath": destination_path.as_ref() }),
        )
        .await
    }

    pub async fn protect_recording_file(&self, path: impl AsRef<str>) -> crate::Result<()> {
        self.call(
            "protectRecordingFile",
            serde_json::json!({ "path": path.as_ref() }),
        )
        .await
    }

    pub async fn audio_duration(&self, path: impl AsRef<str>) -> crate::Result<f64> {
        self.call(
            "audioDuration",
            serde_json::json!({ "path": path.as_ref() }),
        )
        .await
    }

    pub async fn set_api_key(&self, provider_id: &str, value: Option<String>) -> crate::Result<()> {
        validate_provider_id(provider_id)?;
        self.call(
            "setApiKey",
            serde_json::json!({ "providerId": provider_id, "value": value }),
        )
        .await
    }

    pub async fn read_api_key(&self, provider_id: &str) -> crate::Result<Option<String>> {
        validate_provider_id(provider_id)?;
        self.call(
            "readApiKey",
            serde_json::json!({ "providerId": provider_id }),
        )
        .await
    }

    pub async fn has_api_key(&self, provider_id: &str) -> crate::Result<bool> {
        Ok(self.read_api_key(provider_id).await?.is_some())
    }

    pub async fn select_icloud_vault(&self) -> crate::Result<NativeVaultSelection> {
        self.call("selectICloudVault", serde_json::json!({})).await
    }

    pub async fn restore_icloud_vault(&self) -> crate::Result<NativeVaultSelection> {
        self.call("restoreICloudVault", serde_json::json!({})).await
    }

    pub async fn disconnect_icloud_vault(&self) -> crate::Result<()> {
        self.call("disconnectICloudVault", serde_json::json!({}))
            .await
    }

    pub async fn download_icloud_item(&self, relative_path: impl AsRef<str>) -> crate::Result<()> {
        self.call(
            "downloadICloudItem",
            serde_json::json!({ "relativePath": relative_path.as_ref() }),
        )
        .await
    }
}

fn validate_provider_id(provider_id: &str) -> crate::Result<()> {
    if provider_id.is_empty()
        || provider_id.len() > 64
        || !provider_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(crate::Error::InvalidArgument(
            "Invalid AI provider identifier".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_identifiers_are_bounded_and_cannot_select_other_keychain_namespaces() {
        for provider in ["openrouter", "azure_openai", "custom", "provider-2"] {
            assert!(validate_provider_id(provider).is_ok());
        }
        for provider in [
            "",
            "llm:openrouter",
            "../openrouter",
            "OpenAI",
            "a b",
            "🔑",
            &"a".repeat(65),
        ] {
            assert!(validate_provider_id(provider).is_err());
        }
    }
}

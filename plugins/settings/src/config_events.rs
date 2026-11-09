use std::sync::Arc;

use tauri::{Emitter, Manager};
use tokio::sync::watch;

pub(crate) struct ConfigEvents(watch::Sender<bool>);

impl ConfigEvents {
    pub fn stop(&self) {
        self.0.send_replace(true);
    }
}

impl Drop for ConfigEvents {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(crate) fn start<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    config: Arc<crate::ConfigState>,
) -> ConfigEvents {
    let mut changes = config.subscribe();
    let (stop, mut stopping) = watch::channel(false);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = stopping.changed() => break,
                change = changes.changed() => if change.is_err() { break; },
            }
            // One publisher reads the latest committed snapshot. Emitting the
            // snapshot returned to each caller can reorder concurrent writes.
            if let Err(error) = app.emit(crate::CONFIG_CHANGED_EVENT, config.snapshot()) {
                tracing::warn!(%error, "failed to emit settings change");
            }
        }
    });
    ConfigEvents(stop)
}

pub(crate) fn stop<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(events) = app.try_state::<ConfigEvents>() {
        events.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SettingsPluginExt;
    use serde_json::json;
    use std::collections::HashMap;
    use tauri::Listener;

    #[tokio::test]
    async fn legacy_shared_and_reset_writes_have_one_ordered_native_publisher() {
        let vault = tempfile::tempdir().unwrap();
        let config = Arc::new(crate::ConfigState::load_or_default(vault.path()));
        let app = tauri::test::mock_builder()
            .manage(config.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
        app.listen(crate::CONFIG_CHANGED_EVENT, move |event| {
            sent.send(serde_json::from_str::<crate::AppConfig>(event.payload()).unwrap())
                .unwrap();
        });
        let events = start(app.handle(), config.clone());
        for batch in 0..16 {
            let legacy = app.handle().clone();
            let shared = config.clone();
            let (one, two) = tokio::join!(
                async {
                    legacy
                        .settings()
                        .set_config_values(HashMap::from([(
                            "ai_language".into(),
                            json!(format!("legacy-{batch}")),
                        )]))
                        .await
                },
                shared.set_values(HashMap::from([(
                    "ai_language".into(),
                    json!(format!("shared-{batch}"))
                )]))
            );
            one.unwrap();
            two.unwrap();
        }
        config
            .set_values(HashMap::from([("ai_language".into(), json!("final"))]))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if received.recv().await.unwrap().ai_language == "final" {
                    break;
                }
            }
        })
        .await
        .unwrap();
        while let Ok(event) = received.try_recv() {
            assert_eq!(event.ai_language, "final");
        }
        config.reset().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if received.recv().await.unwrap() == crate::AppConfig::default() {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            crate::ConfigState::load_or_default(vault.path()).snapshot(),
            config.snapshot()
        );
        events.stop();
    }
}

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub struct StartupSnapshot {
    startup_vault_base: PathBuf,
    local_vault: bool,
    io_lock: Arc<Mutex<()>>,
    admission: Arc<tokio::sync::Semaphore>,
}

impl StartupSnapshot {
    pub fn new(startup_vault_base: PathBuf) -> Self {
        Self {
            startup_vault_base,
            local_vault: true,
            io_lock: Arc::new(Mutex::new(())),
            admission: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    fn settings_path(&self) -> PathBuf {
        hypr_storage::vault::compute_settings_path(&self.startup_vault_base)
    }

    pub fn device_only(base: PathBuf) -> Self {
        Self {
            startup_vault_base: base,
            local_vault: false,
            io_lock: Arc::new(Mutex::new(())),
            admission: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    pub fn startup_vault_base(&self) -> Option<&PathBuf> {
        self.local_vault.then_some(&self.startup_vault_base)
    }

    fn read_at(path: &Path) -> crate::Result<Option<serde_json::Value>> {
        match std::fs::read_to_string(path) {
            Ok(content) => Ok(Some(serde_json::from_str(&content)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn read_or_default_at(path: &Path) -> crate::Result<serde_json::Value> {
        Ok(Self::read_at(path)?.unwrap_or_else(|| serde_json::json!({})))
    }

    async fn io<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Path) -> crate::Result<T> + Send + 'static,
    ) -> crate::Result<T> {
        let path = self.settings_path();
        let permit = self
            .admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| std::io::Error::other("Settings I/O admission closed"))?;
        let lock = self.io_lock.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _guard = lock
                .lock()
                .map_err(|_| std::io::Error::other("Settings I/O lock failed"))?;
            operation(&path)
        })
        .await
        .map_err(|_| std::io::Error::other("Settings I/O worker failed"))?
    }

    pub async fn load(&self) -> crate::Result<serde_json::Value> {
        self.io(Self::read_or_default_at).await
    }

    pub async fn load_with_legacy_fallback(
        &self,
        legacy_base: &Path,
    ) -> crate::Result<serde_json::Value> {
        let legacy_path = hypr_storage::vault::compute_settings_path(legacy_base);
        let local_vault = self.local_vault;
        self.io(move |path| {
            if let Some(settings) = Self::read_at(path)? {
                return Ok(settings);
            }
            if !local_vault || legacy_path == path {
                return Ok(serde_json::json!({}));
            }
            Ok(match Self::read_or_default_at(&legacy_path) {
                Ok(legacy) if is_non_empty_object(&legacy) => legacy,
                _ => serde_json::json!({}),
            })
        })
        .await
    }

    pub async fn save(&self, settings: serde_json::Value) -> crate::Result<()> {
        self.io(move |path| {
            let existing = Self::read_or_default_at(path)?;
            let content = serde_json::to_string_pretty(&merge_settings(existing, settings))?;
            hypr_storage::fs::atomic_write(path, &content)?;
            Ok(())
        })
        .await
    }

    pub fn reset(&self) -> crate::Result<()> {
        let _guard = self
            .io_lock
            .lock()
            .map_err(|_| std::io::Error::other("Settings I/O lock failed"))?;
        hypr_storage::fs::atomic_write(&self.settings_path(), "{}")?;
        Ok(())
    }
}

fn is_non_empty_object(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|object| !object.is_empty())
}

fn merge_settings(existing: serde_json::Value, incoming: serde_json::Value) -> serde_json::Value {
    match (existing, incoming) {
        (serde_json::Value::Object(mut existing_map), serde_json::Value::Object(incoming_map)) => {
            for (key, value) in incoming_map {
                existing_map.insert(key, value);
            }
            serde_json::Value::Object(existing_map)
        }
        (_, incoming) => incoming,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    async fn block_io(
        snapshot: &StartupSnapshot,
    ) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        let lock = snapshot.io_lock.clone();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _guard = lock.lock().unwrap();
            entered.send(()).unwrap();
            let _ = wait.recv();
        });
        ready.await.unwrap();
        (release, worker)
    }

    #[tokio::test]
    async fn cancelled_snapshot_save_retains_its_write_and_merges_a_later_save() {
        let root = tempdir().unwrap();
        let snapshot = Arc::new(StartupSnapshot::device_only(root.path().into()));
        let (release, blocker) = block_io(&snapshot).await;
        let first = snapshot.clone();
        let operation = tokio::spawn(async move { first.save(json!({"first":1})).await });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while snapshot.admission.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        operation.abort();
        assert!(operation.await.unwrap_err().is_cancelled());
        assert_eq!(snapshot.admission.available_permits(), 0);
        let second = snapshot.clone();
        let next = tokio::spawn(async move { second.save(json!({"second":2})).await });
        release.send(()).unwrap();
        blocker.join().unwrap();
        next.await.unwrap().unwrap();
        assert_eq!(
            snapshot.load().await.unwrap(),
            json!({"first":1,"second":2})
        );
    }

    #[tokio::test]
    async fn snapshot_reset_waits_for_the_same_active_io_owner() {
        let root = tempdir().unwrap();
        let snapshot = Arc::new(StartupSnapshot::device_only(root.path().into()));
        snapshot.save(json!({"before":true})).await.unwrap();
        let (release, blocker) = block_io(&snapshot).await;
        let reset = snapshot.clone();
        let (started, running) = tokio::sync::oneshot::channel();
        let (done, mut finished) = tokio::sync::oneshot::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            done.send(reset.reset()).unwrap();
        });
        running.await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut finished)
                .await
                .is_err()
        );
        release.send(()).unwrap();
        blocker.join().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), finished)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert_eq!(snapshot.load().await.unwrap(), json!({}));
    }

    #[tokio::test]
    async fn load_uses_global_legacy_settings_when_custom_vault_is_missing() {
        let temp = tempdir().unwrap();
        let vault_base = temp.path().join("vault");
        let global_base = temp.path().join("global");
        std::fs::create_dir_all(&vault_base).unwrap();
        std::fs::create_dir_all(&global_base).unwrap();
        std::fs::write(
            hypr_storage::vault::compute_settings_path(&global_base),
            r#"{"ai":{"current_llm_provider":"openai"}}"#,
        )
        .unwrap();

        let snapshot = StartupSnapshot::new(vault_base);

        assert_eq!(
            snapshot
                .load_with_legacy_fallback(&global_base)
                .await
                .unwrap(),
            json!({"ai": {"current_llm_provider": "openai"}}),
        );
    }

    #[tokio::test]
    async fn load_preserves_an_explicit_custom_vault_reset() {
        let temp = tempdir().unwrap();
        let vault_base = temp.path().join("vault");
        let global_base = temp.path().join("global");
        std::fs::create_dir_all(&vault_base).unwrap();
        std::fs::create_dir_all(&global_base).unwrap();
        std::fs::write(
            hypr_storage::vault::compute_settings_path(&vault_base),
            "{}",
        )
        .unwrap();
        std::fs::write(
            hypr_storage::vault::compute_settings_path(&global_base),
            r#"{"general":{"theme":"light"}}"#,
        )
        .unwrap();

        let snapshot = StartupSnapshot::new(vault_base);

        assert_eq!(
            snapshot
                .load_with_legacy_fallback(&global_base)
                .await
                .unwrap(),
            json!({}),
        );
    }

    #[tokio::test]
    async fn load_prefers_non_empty_custom_vault_settings() {
        let temp = tempdir().unwrap();
        let vault_base = temp.path().join("vault");
        let global_base = temp.path().join("global");
        std::fs::create_dir_all(&vault_base).unwrap();
        std::fs::create_dir_all(&global_base).unwrap();
        std::fs::write(
            hypr_storage::vault::compute_settings_path(&vault_base),
            r#"{"general":{"theme":"dark"}}"#,
        )
        .unwrap();
        std::fs::write(
            hypr_storage::vault::compute_settings_path(&global_base),
            r#"{"general":{"theme":"light"}}"#,
        )
        .unwrap();

        let snapshot = StartupSnapshot::new(vault_base);

        assert_eq!(
            snapshot
                .load_with_legacy_fallback(&global_base)
                .await
                .unwrap(),
            json!({"general": {"theme": "dark"}}),
        );
    }

    #[test]
    fn merge_both_objects() {
        let existing = json!({"a": 1, "b": 2});
        let incoming = json!({"b": 3, "c": 4});
        let result = merge_settings(existing, incoming);
        assert_eq!(result, json!({"a": 1, "b": 3, "c": 4}));
    }

    #[test]
    fn merge_empty_existing() {
        let existing = json!({});
        let incoming = json!({"a": 1});
        let result = merge_settings(existing, incoming);
        assert_eq!(result, json!({"a": 1}));
    }

    #[test]
    fn merge_empty_incoming() {
        let existing = json!({"a": 1});
        let incoming = json!({});
        let result = merge_settings(existing, incoming);
        assert_eq!(result, json!({"a": 1}));
    }

    #[test]
    fn merge_incoming_replaces_non_object_existing() {
        let existing = json!(null);
        let incoming = json!({"a": 1});
        let result = merge_settings(existing, incoming);
        assert_eq!(result, json!({"a": 1}));
    }

    #[test]
    fn merge_non_object_incoming_replaces_existing() {
        let existing = json!({"a": 1});
        let incoming = json!([1, 2, 3]);
        let result = merge_settings(existing, incoming);
        assert_eq!(result, json!([1, 2, 3]));
    }
}

use std::path::PathBuf;

use camino::Utf8PathBuf;

pub struct Settings<'a, R: tauri::Runtime, M: tauri::Manager<R>> {
    manager: &'a M,
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<'a, R: tauri::Runtime, M: tauri::Manager<R>> Settings<'a, R, M> {
    fn settings_base_path(&self) -> Result<PathBuf, crate::Error> {
        let bundle_id: &str = self.manager.config().identifier.as_ref();
        let path = hypr_storage::global::compute_default_base(bundle_id)
            .ok_or(hypr_storage::Error::DataDirUnavailable)?;
        std::fs::create_dir_all(&path)?;
        Ok(path)
    }

    pub fn settings_base(&self) -> Result<Utf8PathBuf, crate::Error> {
        let path = self.settings_base_path()?;
        Utf8PathBuf::from_path_buf(path).map_err(|_| hypr_storage::Error::PathNotValidUtf8.into())
    }

    pub fn global_base(&self) -> Result<Utf8PathBuf, crate::Error> {
        self.settings_base()
    }

    pub fn settings_path(&self) -> Result<Utf8PathBuf, crate::Error> {
        let base = self.vault_base()?;
        Ok(base.join(hypr_storage::vault::SETTINGS_FILENAME))
    }

    pub fn vault_base(&self) -> Result<Utf8PathBuf, crate::Error> {
        let snapshot = self.manager.state::<crate::state::StartupSnapshot>();
        Utf8PathBuf::from_path_buf(snapshot.startup_vault_base().clone())
            .map_err(|_| hypr_storage::Error::PathNotValidUtf8.into())
    }

    pub fn resolve_startup_vault_base(&self) -> Result<PathBuf, crate::Error> {
        let settings_base = self.settings_base_path()?;
        Ok(hypr_storage::vault::resolve_base(
            &settings_base,
            &settings_base,
        ))
    }

    pub fn is_empty_or_missing_dir(&self, path: Utf8PathBuf) -> Result<bool, crate::Error> {
        hypr_storage::vault::fs::is_empty_or_missing_dir(path.as_ref()).map_err(Into::into)
    }

    pub fn classify_vault_dir(
        &self,
        path: Utf8PathBuf,
    ) -> Result<hypr_storage::vault::fs::VaultDirKind, crate::Error> {
        hypr_storage::vault::fs::classify_vault_dir(path.as_ref()).map_err(Into::into)
    }

    pub async fn load(&self) -> crate::Result<serde_json::Value> {
        let snapshot = self.manager.state::<crate::state::StartupSnapshot>();
        let legacy_base = self.settings_base_path()?;
        snapshot.load_with_legacy_fallback(&legacy_base).await
    }

    pub async fn save(&self, settings: serde_json::Value) -> crate::Result<()> {
        let snapshot = self.manager.state::<crate::state::StartupSnapshot>();
        snapshot.save(settings).await
    }

    pub fn reset(&self) -> crate::Result<()> {
        let snapshot = self.manager.state::<crate::state::StartupSnapshot>();
        snapshot.reset()
    }

    pub fn config(&self) -> crate::AppConfig {
        self.manager.state::<crate::ConfigState>().snapshot()
    }

    pub async fn set_config_values(
        &self,
        values: std::collections::HashMap<String, serde_json::Value>,
    ) -> crate::Result<()> {
        use tauri::Emitter;

        let state = self.manager.state::<crate::ConfigState>();
        let next = state.set_values(values).await?;

        // Broadcast the FULL new config to every webview, including the one
        // that issued the write: the FE store replaces its snapshot
        // idempotently, so the writer's own echo is harmless and keeps all
        // windows converged on the same state.
        self.manager
            .app_handle()
            .emit(crate::CONFIG_CHANGED_EVENT, &next)?;
        Ok(())
    }

    pub fn reset_config(&self) -> crate::Result<()> {
        self.manager.state::<crate::ConfigState>().reset()?;
        Ok(())
    }
}

impl<'a, R: tauri::Runtime, M: tauri::Manager<R>> Settings<'a, R, M> {
    pub async fn copy_vault(&self, new_path: Utf8PathBuf) -> Result<(), crate::Error> {
        let old_vault_base = self.vault_base()?;

        if new_path == old_vault_base {
            return Ok(());
        }

        hypr_storage::vault::validate_vault_base_change(
            old_vault_base.as_ref(),
            new_path.as_ref(),
        )?;
        hypr_storage::vault::ensure_vault_dir(new_path.as_ref())?;
        hypr_storage::vault::fs::copy_vault_items(old_vault_base.as_ref(), new_path.as_ref())
            .await?;

        Ok(())
    }

    pub async fn move_vault(&self, new_path: Utf8PathBuf) -> Result<(), crate::Error> {
        let old_vault_base = self.vault_base()?;

        if new_path == old_vault_base {
            return Ok(());
        }

        hypr_storage::vault::validate_vault_base_change(
            old_vault_base.as_ref(),
            new_path.as_ref(),
        )?;
        if !hypr_storage::vault::fs::is_empty_or_missing_dir(new_path.as_ref())? {
            return Err(hypr_storage::Error::VaultBaseIsNotEmpty.into());
        }
        hypr_storage::vault::ensure_vault_dir(new_path.as_ref())?;

        // 1. Copy items to new location
        hypr_storage::vault::fs::copy_vault_items(old_vault_base.as_ref(), new_path.as_ref())
            .await?;

        // 2. Persist the new vault path so config points to the copy
        self.set_vault_base(new_path).await?;

        // 3. Clean up old location (best-effort; data is already safe at the new path)
        let _ = hypr_storage::vault::fs::remove_vault_items(old_vault_base.as_ref()).await;

        Ok(())
    }

    pub async fn set_vault_base(&self, new_path: Utf8PathBuf) -> Result<(), crate::Error> {
        let settings_base = self.settings_base_path()?;
        hypr_storage::vault::persist_vault_path(&settings_base, &settings_base, new_path.as_ref())?;
        Ok(())
    }
}

pub trait SettingsPluginExt<R: tauri::Runtime> {
    fn settings(&self) -> Settings<'_, R, Self>
    where
        Self: tauri::Manager<R> + Sized;
}

impl<R: tauri::Runtime, T: tauri::Manager<R>> SettingsPluginExt<R> for T {
    fn settings(&self) -> Settings<'_, R, Self>
    where
        Self: Sized,
    {
        Settings {
            manager: self,
            _runtime: std::marker::PhantomData,
        }
    }
}

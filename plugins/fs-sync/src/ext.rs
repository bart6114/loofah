use tauri_plugin_settings::SettingsPluginExt;

pub struct FsSync<'a, R: tauri::Runtime, M: tauri::Manager<R>> {
    manager: &'a M,
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<'a, R: tauri::Runtime, M: tauri::Manager<R>> FsSync<'a, R, M> {
    fn core(&self) -> Result<hypr_fs_sync_core::FsSyncCore, crate::Error> {
        let app = self.manager.app_handle();
        use tauri::Manager;
        if let Some(store) = app.try_state::<std::sync::Arc<hypr_vault_write::SessionStore>>() {
            store
                .ensure_ready()
                .map_err(|e| crate::Error::Path(e.to_string()))?;
        }
        let base_dir = app
            .settings()
            .vault_base()
            .map(|p| p.into_std_path_buf())
            .map_err(|e| crate::Error::Path(e.to_string()))?;

        Ok(hypr_fs_sync_core::FsSyncCore::new(base_dir))
    }

    fn notify_artifacts_changed(&self, id: &str) {
        use tauri::Manager;
        if let Some(store) = self
            .manager
            .app_handle()
            .try_state::<std::sync::Arc<hypr_vault_write::SessionStore>>()
        {
            store.notify_artifacts_changed(id);
        }
    }

    pub fn attachment_save(
        &self,
        session_id: &str,
        data: &[u8],
        filename: &str,
    ) -> Result<crate::AttachmentSaveResult, crate::Error> {
        let result = self.core()?.attachment_save(session_id, data, filename)?;
        self.notify_artifacts_changed(session_id);
        Ok(result)
    }

    pub fn attachment_import_path(
        &self,
        session_id: &str,
        source_path: &std::path::Path,
    ) -> Result<crate::AttachmentInfo, crate::Error> {
        let result = self
            .core()?
            .attachment_import_path(session_id, source_path)?;
        self.notify_artifacts_changed(session_id);
        Ok(result)
    }

    pub fn attachment_dir(&self, session_id: &str) -> Result<std::path::PathBuf, crate::Error> {
        self.core()?.attachment_dir(session_id)
    }

    pub fn attachment_list(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::AttachmentInfo>, crate::Error> {
        self.core()?.attachment_list(session_id)
    }

    pub fn attachment_read(
        &self,
        session_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, crate::Error> {
        self.core()?.attachment_read(session_id, attachment_id)
    }

    pub fn attachment_remove(
        &self,
        session_id: &str,
        attachment_id: &str,
    ) -> Result<(), crate::Error> {
        self.core()?.attachment_remove(session_id, attachment_id)?;
        self.notify_artifacts_changed(session_id);
        Ok(())
    }
}

pub trait FsSyncPluginExt<R: tauri::Runtime> {
    fn fs_sync(&self) -> FsSync<'_, R, Self>
    where
        Self: tauri::Manager<R> + Sized;
}

impl<R: tauri::Runtime, T: tauri::Manager<R>> FsSyncPluginExt<R> for T {
    fn fs_sync(&self) -> FsSync<'_, R, Self>
    where
        Self: Sized,
    {
        FsSync {
            manager: self,
            _runtime: std::marker::PhantomData,
        }
    }
}

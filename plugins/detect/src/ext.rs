pub struct Detect<'a, R: tauri::Runtime, M: tauri::Manager<R>> {
    manager: &'a M,
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<'a, R: tauri::Runtime, M: tauri::Manager<R>> Detect<'a, R, M> {
    pub fn list_installed_applications(&self) -> Vec<hypr_detect::InstalledApp> {
        hypr_detect::list_installed_apps()
    }

    pub fn list_mic_using_applications(
        &self,
    ) -> Result<Vec<hypr_detect::InstalledApp>, crate::Error> {
        Ok(hypr_detect::list_mic_using_apps()?)
    }

    pub fn list_default_ignored_bundle_ids(&self) -> Vec<String> {
        crate::policy::default_ignored_bundle_ids()
    }

    pub fn set_ignored_bundle_ids(&self, bundle_ids: Vec<String>) {
        let state = self.manager.state::<crate::ProcessorState>();
        let mut state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
        state_guard.policy.user_ignored_bundle_ids = bundle_ids.into_iter().collect();
    }

    pub fn set_included_bundle_ids(&self, bundle_ids: Vec<String>) {
        let state = self.manager.state::<crate::ProcessorState>();
        let mut state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
        state_guard.policy.user_included_bundle_ids = bundle_ids.into_iter().collect();
    }

    pub fn set_respect_do_not_disturb(&self, enabled: bool) {
        let state = self.manager.state::<crate::ProcessorState>();
        let mut state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
        state_guard.policy.respect_dnd = enabled;
    }
}

pub trait DetectPluginExt<R: tauri::Runtime> {
    fn detect(&self) -> Detect<'_, R, Self>
    where
        Self: tauri::Manager<R> + Sized;
}

impl<R: tauri::Runtime, T: tauri::Manager<R>> DetectPluginExt<R> for T {
    fn detect(&self) -> Detect<'_, R, Self>
    where
        Self: Sized,
    {
        Detect {
            manager: self,
            _runtime: std::marker::PhantomData,
        }
    }
}

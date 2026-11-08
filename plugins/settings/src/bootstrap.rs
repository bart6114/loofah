use hypr_loofah_client::{ConnectionProfiles, DesktopStartupSelection, DesktopStartupSnapshot};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct DesktopBootstrap {
    profiles: Arc<ConnectionProfiles>,
    startup: hypr_loofah_client::Result<DesktopStartupSnapshot>,
    device_base: PathBuf,
}

pub(crate) fn acquire_local_vault(base: &Path) -> crate::Result<hypr_storage::vault::VaultLease> {
    std::fs::create_dir_all(base)?;
    match hypr_storage::vault::VaultLease::try_acquire_existing(base)? {
        hypr_storage::vault::LeaseAttempt::Acquired(lease) => Ok(lease),
        hypr_storage::vault::LeaseAttempt::Busy => Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "This vault is already open in another process. Close its current owner before opening Loofah.",
        ).into()),
    }
}

impl DesktopBootstrap {
    pub fn load(global_base: &Path) -> crate::Result<Self> {
        let profiles = ConnectionProfiles::new(global_base.join("connections"))
            .map_err(|cause| crate::Error::StartupPreferences(cause.message))?;
        let startup = profiles.desktop_startup();
        Ok(Self {
            profiles,
            startup,
            device_base: global_base.join("device-preferences"),
        })
    }

    pub fn is_local(&self) -> bool {
        matches!(&self.startup, Ok(snapshot) if matches!(snapshot.selection, DesktopStartupSelection::Local {}))
    }

    pub fn startup(&self) -> hypr_loofah_client::Result<DesktopStartupSnapshot> {
        self.startup.clone()
    }

    pub fn profiles(&self) -> Arc<ConnectionProfiles> {
        self.profiles.clone()
    }

    pub(crate) fn settings_location(
        &self,
        local_vault: impl FnOnce() -> crate::Result<PathBuf>,
    ) -> crate::Result<(PathBuf, crate::StartupSnapshot)> {
        if self.is_local() {
            let base = local_vault()?;
            return Ok((base.clone(), crate::StartupSnapshot::new(base)));
        }
        for (path, directory) in [
            (self.device_base.clone(), true),
            (
                hypr_storage::vault::compute_config_path(&self.device_base),
                false,
            ),
            (
                hypr_storage::vault::compute_settings_path(&self.device_base),
                false,
            ),
        ] {
            match std::fs::symlink_metadata(path) {
                Ok(meta)
                    if meta.file_type().is_symlink()
                        || meta.is_dir() != directory
                        || (!directory && !meta.is_file()) =>
                {
                    return Err(crate::Error::StartupPreferences(
                        "Device preferences contain an unsupported filesystem entry".into(),
                    ));
                }
                Ok(_) => {}
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                Err(cause) => return Err(cause.into()),
            }
        }
        Ok((
            self.device_base.clone(),
            crate::StartupSnapshot::device_only(self.device_base.clone()),
        ))
    }
}

#[cfg(test)]
mod tests;

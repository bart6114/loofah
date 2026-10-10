#[cfg(not(target_os = "windows"))]
use tauri_plugin_autostart::ManagerExt;

#[cfg(target_os = "windows")]
#[path = "autostart/windows.rs"]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::uninstall as uninstall_windows;

const LEGACY_DEV_NAME: &str = "Free Meeting Transcriber Dev";
const LEGACY_STABLE_NAME: &str = "Free Meeting Transcriber";
const LEGACY_STAGING_NAME: &str = "Free Meeting Transcriber Staging";
const DEV_NAME: &str = "Loofah Dev";
const STABLE_NAME: &str = "Loofah";
const STAGING_NAME: &str = "Loofah Staging";

fn current_name(identifier: &str) -> &str {
    match identifier {
        "io.loofah.dev" => DEV_NAME,
        "io.loofah.staging" => STAGING_NAME,
        "io.loofah.stable" => STABLE_NAME,
        _ => identifier,
    }
}

fn legacy_name(identifier: &str) -> Option<&'static str> {
    match identifier {
        "io.loofah.dev" => Some(LEGACY_DEV_NAME),
        "io.loofah.staging" => Some(LEGACY_STAGING_NAME),
        "io.loofah.stable" => Some(LEGACY_STABLE_NAME),
        _ => None,
    }
}

pub fn plugin<R: tauri::Runtime>(identifier: &str) -> tauri::plugin::TauriPlugin<R> {
    #[cfg(target_os = "windows")]
    {
        let _ = identifier;
        return windows::plugin();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let builder = tauri_plugin_autostart::Builder::new()
            .app_name(current_name(identifier))
            .arg("--background");
        #[cfg(target_os = "macos")]
        let builder = builder.macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent);
        builder.build()
    }
}

pub fn migrate<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    #[cfg(target_os = "windows")]
    let _ = app;
    #[cfg(not(target_os = "windows"))]
    {
        let Some(legacy_name) = legacy_name(&app.config().identifier) else {
            return;
        };

        match remove_legacy_entry(legacy_name) {
            Ok(true) => {
                if let Err(error) = app.autolaunch().enable() {
                    tracing::warn!(%error, "failed to migrate the legacy autostart entry");
                }
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "failed to remove the legacy autostart entry"),
        }
    }
}

#[cfg(target_os = "macos")]
fn remove_legacy_entry(name: &str) -> std::io::Result<bool> {
    remove_file_if_present(
        dirs::home_dir()
            .ok_or_else(|| std::io::Error::other("home directory is unavailable"))?
            .join("Library/LaunchAgents")
            .join(format!("{name}.plist")),
    )
}

#[cfg(target_os = "linux")]
fn remove_legacy_entry(name: &str) -> std::io::Result<bool> {
    remove_file_if_present(
        dirs::home_dir()
            .ok_or_else(|| std::io::Error::other("home directory is unavailable"))?
            .join(".config/autostart")
            .join(format!("{name}.desktop")),
    )
}

#[cfg(windows)]
fn remove_legacy_entry(name: &str) -> std::io::Result<bool> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

    const RUN_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run";
    const APPROVED_KEY: &str =
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";

    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let run = current_user.open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_SET_VALUE)?;
    let existed = run.get_raw_value(name).is_ok();
    if existed {
        run.delete_value(name)?;
    }
    if let Ok(approved) = current_user.open_subkey_with_flags(APPROVED_KEY, KEY_SET_VALUE) {
        let _ = approved.delete_value(name);
    }
    Ok(existed)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn remove_legacy_entry(_name: &str) -> std::io::Result<bool> {
    Ok(false)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn remove_file_if_present(path: std::path::PathBuf) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_channel_to_its_previous_autostart_name() {
        assert_eq!(current_name("io.loofah.dev"), DEV_NAME);
        assert_eq!(current_name("io.loofah.staging"), STAGING_NAME);
        assert_eq!(current_name("io.loofah.stable"), STABLE_NAME);
        assert_eq!(legacy_name("io.loofah.dev"), Some(LEGACY_DEV_NAME));
        assert_eq!(legacy_name("io.loofah.staging"), Some(LEGACY_STAGING_NAME));
        assert_eq!(legacy_name("io.loofah.stable"), Some(LEGACY_STABLE_NAME));
    }
}

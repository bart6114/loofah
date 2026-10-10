#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;

use serde::Serialize;

#[cfg(any(target_os = "windows", test))]
mod windows;

const DEV_BUNDLE_ID: &str = "io.loofah.dev";
#[cfg(target_os = "macos")]
const MANAGED_CLI_DIR: &str = ".loof-cli";
#[cfg(target_os = "macos")]
const LEGACY_MANAGED_CLI_DIRS: [&str; 2] = [".loofah-cli", ".fmtr-cli"];
const STABLE_BUNDLE_ID: &str = "io.loofah.stable";
const STAGING_BUNDLE_ID: &str = "io.loofah.staging";

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddedCliState {
    Installed,
    Missing,
    Conflict,
    Unsupported,
    ResourceMissing,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedCliStatus {
    pub supported: bool,
    pub command_name: String,
    pub install_path: String,
    pub state: EmbeddedCliState,
    pub details: Option<String>,
}

pub fn check<R: tauri::Runtime, T: tauri::Manager<R>>(manager: &T) -> EmbeddedCliStatus {
    #[cfg(target_os = "windows")]
    return windows::check(manager);

    #[cfg(not(target_os = "windows"))]
    {
        let command_name = command_name_from_identifier(manager.config().identifier.as_ref());
        let Some(install_path) = install_path_for_command(command_name) else {
            return unavailable_status(command_name, "Loofah could not find your home directory.");
        };

        #[cfg(not(target_os = "macos"))]
        {
            let _ = manager;
            return EmbeddedCliStatus {
                supported: false,
                command_name: command_name.to_string(),
                install_path: install_path.display().to_string(),
                state: EmbeddedCliState::Unsupported,
                details: Some(
                    "Bundled CLI installation is currently available on macOS.".to_string(),
                ),
            };
        }

        #[cfg(target_os = "macos")]
        {
            let Some(resource_path) = resolve_resource_path(manager) else {
                return EmbeddedCliStatus {
                    supported: true,
                    command_name: command_name.to_string(),
                    install_path: install_path.display().to_string(),
                    state: EmbeddedCliState::ResourceMissing,
                    details: Some("The CLI is not included in this build of Loofah.".to_string()),
                };
            };

            classify_status(command_name, install_path, &resource_path)
        }
    }
}

pub fn install<R: tauri::Runtime, T: tauri::Manager<R>>(
    manager: &T,
) -> Result<EmbeddedCliStatus, String> {
    #[cfg(target_os = "windows")]
    return windows::install(manager);

    #[cfg(not(target_os = "windows"))]
    {
        let status = check(manager);

        #[cfg(not(target_os = "macos"))]
        {
            Ok(status)
        }

        #[cfg(target_os = "macos")]
        {
            match status.state {
                EmbeddedCliState::Unsupported | EmbeddedCliState::ResourceMissing => {
                    return Ok(status);
                }
                EmbeddedCliState::Conflict => {
                    return Err(format!(
                        "Another file already exists at {}. Move it before installing the loof CLI.",
                        status.install_path
                    ));
                }
                EmbeddedCliState::Installed | EmbeddedCliState::Missing => {}
            }

            let resource_path = resolve_resource_path(manager)
                .ok_or_else(|| "The bundled CLI could not be found.".to_string())?;
            let install_path = PathBuf::from(&status.install_path);

            install_symlink(&resource_path, &install_path)?;
            remove_legacy_managed_copies(&install_path, &status.command_name);
            for legacy_command in
                legacy_command_names_from_identifier(manager.config().identifier.as_ref())
            {
                let Some(legacy_path) = install_path_for_command(legacy_command) else {
                    continue;
                };
                if matches!(
                    classify_installation(&legacy_path, &resource_path),
                    Ok(EmbeddedCliState::Installed | EmbeddedCliState::Missing)
                ) && std::fs::symlink_metadata(&legacy_path)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    std::fs::remove_file(&legacy_path).map_err(|error| {
                        format!(
                            "Could not remove the legacy command at {}: {error}",
                            legacy_path.display()
                        )
                    })?;
                }
                remove_legacy_managed_copies(&legacy_path, legacy_command);
            }
            Ok(classify_status(
                &status.command_name,
                install_path,
                &resource_path,
            ))
        }
    }
}

/// Re-points a previously installed CLI symlink at the current app bundle.
/// Runs at startup so the command on PATH follows app updates and moves,
/// and so old command names and pre-symlink installs migrate to `loof`.
/// Never installs for users who haven't opted in via Settings -> Agents.
pub fn sync_installed<R: tauri::Runtime, T: tauri::Manager<R>>(manager: &T) {
    #[cfg(target_os = "windows")]
    windows::sync_installed(manager);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = manager;
    }

    #[cfg(target_os = "macos")]
    {
        let status = check(manager);
        if status.state != EmbeddedCliState::Missing {
            return;
        }
        let primary_is_symlink = std::fs::symlink_metadata(PathBuf::from(&status.install_path))
            .is_ok_and(|metadata| metadata.file_type().is_symlink());
        let legacy_is_symlink =
            legacy_command_names_from_identifier(manager.config().identifier.as_ref())
                .iter()
                .filter_map(|command| install_path_for_command(command))
                .any(|path| {
                    std::fs::symlink_metadata(path)
                        .is_ok_and(|metadata| metadata.file_type().is_symlink())
                });
        if !primary_is_symlink && !legacy_is_symlink {
            return;
        }

        match install(manager) {
            Ok(status) if status.state == EmbeddedCliState::Installed => {
                tracing::info!(
                    command = status.command_name,
                    "relinked the loof CLI to the current app"
                );
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "failed to relink the loof CLI"),
        }
    }
}

fn unavailable_status(command_name: &str, details: &str) -> EmbeddedCliStatus {
    EmbeddedCliStatus {
        supported: false,
        command_name: command_name.to_string(),
        install_path: String::new(),
        state: EmbeddedCliState::Unsupported,
        details: Some(details.to_string()),
    }
}

fn command_name_from_identifier(identifier: &str) -> &'static str {
    match identifier {
        STABLE_BUNDLE_ID => "loof",
        STAGING_BUNDLE_ID => "loof-staging",
        DEV_BUNDLE_ID => "loof-dev",
        _ => "loof-dev",
    }
}

fn legacy_command_names_from_identifier(identifier: &str) -> &'static [&'static str] {
    match identifier {
        STABLE_BUNDLE_ID => &["loofah", "fmtr"],
        STAGING_BUNDLE_ID => &["loofah-staging", "fmtr-staging"],
        DEV_BUNDLE_ID => &["loofah-dev", "fmtr-dev"],
        _ => &["loofah-dev", "fmtr-dev"],
    }
}

fn install_path_for_command(command_name: &str) -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".local/bin").join(command_name))
}

#[cfg(target_os = "macos")]
fn resolve_resource_path<R: tauri::Runtime, T: tauri::Manager<R>>(manager: &T) -> Option<PathBuf> {
    use tauri::path::BaseDirectory;

    if let Some(sidecar_path) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("loof")))
        .filter(|path| path.is_file())
    {
        return Some(sidecar_path);
    }

    let file_name = bundled_binary_name()?;

    if let Some(bundled_resource_path) = manager
        .path()
        .resolve(format!("cli/{file_name}"), BaseDirectory::Resource)
        .ok()
        .filter(|path| path.exists())
    {
        return Some(bundled_resource_path);
    }

    let debug_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("cli")
        .join(file_name);
    debug_path.exists().then_some(debug_path)
}

#[cfg(target_os = "macos")]
fn bundled_binary_name() -> Option<&'static str> {
    #[cfg(target_arch = "aarch64")]
    {
        return Some("loof-aarch64-apple-darwin");
    }

    #[cfg(target_arch = "x86_64")]
    {
        return Some("loof-x86_64-apple-darwin");
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(target_os = "macos")]
fn classify_status(
    command_name: &str,
    install_path: PathBuf,
    resource_path: &Path,
) -> EmbeddedCliStatus {
    match classify_installation(&install_path, resource_path) {
        Ok(state) => EmbeddedCliStatus {
            supported: true,
            command_name: command_name.to_string(),
            install_path: install_path.display().to_string(),
            state,
            details: details_for_state(state, &install_path),
        },
        Err(error) => EmbeddedCliStatus {
            supported: true,
            command_name: command_name.to_string(),
            install_path: install_path.display().to_string(),
            state: EmbeddedCliState::Conflict,
            details: Some(error),
        },
    }
}

#[cfg(target_os = "macos")]
fn classify_installation(
    install_path: &Path,
    resource_path: &Path,
) -> Result<EmbeddedCliState, String> {
    let metadata = match std::fs::symlink_metadata(install_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(EmbeddedCliState::Missing);
        }
        Err(error) => {
            return Err(format!(
                "Failed to inspect {}: {error}",
                install_path.display()
            ));
        }
    };

    if !metadata.file_type().is_symlink() {
        return Ok(EmbeddedCliState::Conflict);
    }

    let target = std::fs::read_link(install_path).map_err(|error| {
        format!(
            "Failed to inspect the installed command at {}: {error}",
            install_path.display()
        )
    })?;
    let target = if target.is_absolute() {
        target
    } else {
        install_path
            .parent()
            .map(|dir| dir.join(&target))
            .unwrap_or(target)
    };

    if points_at(&target, resource_path) {
        return Ok(EmbeddedCliState::Installed);
    }
    if is_replaceable_symlink_target(&target) {
        return Ok(EmbeddedCliState::Missing);
    }
    Ok(EmbeddedCliState::Conflict)
}

#[cfg(target_os = "macos")]
fn points_at(target: &Path, resource_path: &Path) -> bool {
    if target == resource_path {
        return true;
    }
    matches!(
        (
            std::fs::canonicalize(target),
            std::fs::canonicalize(resource_path),
        ),
        (Ok(target), Ok(resource)) if target == resource
    )
}

/// Targets a previous install could have left behind: the legacy versioned
/// copies under the old or current managed CLI directory, a sidecar inside an app bundle (older app
/// location or channel), the dev `resources/cli` tree, or a dangling link
/// (replacing one cannot lose anything). Anything else is someone else's
/// file and must not be overwritten.
#[cfg(target_os = "macos")]
fn is_replaceable_symlink_target(target: &Path) -> bool {
    if target.components().any(|component| {
        component.as_os_str() == MANAGED_CLI_DIR
            || LEGACY_MANAGED_CLI_DIRS
                .iter()
                .any(|directory| component.as_os_str() == *directory)
    }) {
        return true;
    }
    if !target.exists() {
        return true;
    }
    let path = target.to_string_lossy();
    path.contains(".app/Contents/MacOS/") || path.contains("/resources/cli/")
}

#[cfg(target_os = "macos")]
fn details_for_state(state: EmbeddedCliState, install_path: &Path) -> Option<String> {
    match state {
        EmbeddedCliState::Installed => Some(format!(
            "Installed at {} and linked to this app, so it updates together with the app.",
            install_path.display()
        )),
        EmbeddedCliState::Missing => Some(format!(
            "Install the command at {}.",
            install_path.display()
        )),
        EmbeddedCliState::Conflict => Some(format!(
            "Another file already exists at {}.",
            install_path.display()
        )),
        EmbeddedCliState::Unsupported => None,
        EmbeddedCliState::ResourceMissing => None,
    }
}

#[cfg(target_os = "macos")]
fn install_symlink(resource_path: &Path, install_path: &Path) -> Result<(), String> {
    let install_dir = install_path
        .parent()
        .ok_or_else(|| "The CLI install directory is invalid.".to_string())?;
    std::fs::create_dir_all(install_dir)
        .map_err(|error| format!("Could not create {}: {error}", install_dir.display()))?;

    let file_name = install_path
        .file_name()
        .ok_or_else(|| "The CLI install path is invalid.".to_string())?;
    let temp_path = install_path.with_file_name(format!(
        ".{}.tmp-{}",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    if std::fs::symlink_metadata(&temp_path).is_ok() {
        std::fs::remove_file(&temp_path).map_err(|error| {
            format!(
                "Could not prepare the command update at {}: {error}",
                temp_path.display()
            )
        })?;
    }

    std::os::unix::fs::symlink(resource_path, &temp_path).map_err(|error| {
        format!(
            "Could not prepare the command at {}: {error}",
            temp_path.display()
        )
    })?;
    if let Err(error) = ensure_install_path_replaceable(install_path, resource_path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temp_path, install_path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!(
            "Could not install the command at {}: {error}",
            install_path.display()
        ));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn ensure_install_path_replaceable(
    install_path: &Path,
    resource_path: &Path,
) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(install_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Failed to inspect {}: {error}",
                install_path.display()
            ));
        }
    };

    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(install_path).map_err(|error| {
            format!(
                "Failed to inspect the installed command at {}: {error}",
                install_path.display()
            )
        })?;
        if points_at(&target, resource_path) || is_replaceable_symlink_target(&target) {
            return Ok(());
        }
    }

    Err(format!(
        "Another file already exists at {}.",
        install_path.display()
    ))
}

/// Earlier releases copied the CLI to a versioned managed directory and symlinked to the copy.
#[cfg(target_os = "macos")]
fn remove_legacy_managed_copies(install_path: &Path, command_name: &str) {
    let Some(install_dir) = install_path.parent() else {
        return;
    };
    for directory in std::iter::once(MANAGED_CLI_DIR).chain(LEGACY_MANAGED_CLI_DIRS) {
        let managed_dir = install_dir.join(directory);
        let _ = std::fs::remove_dir_all(managed_dir.join(command_name));
        let _ = std::fs::remove_dir(managed_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_bundle_id_to_command_name() {
        assert_eq!(command_name_from_identifier(STABLE_BUNDLE_ID), "loof");
        assert_eq!(
            command_name_from_identifier(STAGING_BUNDLE_ID),
            "loof-staging"
        );
        assert_eq!(command_name_from_identifier(DEV_BUNDLE_ID), "loof-dev");
        assert_eq!(command_name_from_identifier("unknown"), "loof-dev");
        assert_eq!(
            legacy_command_names_from_identifier(STABLE_BUNDLE_ID),
            &["loofah", "fmtr"]
        );
    }

    #[cfg(target_os = "macos")]
    fn write_app_bundle_cli(dir: &Path, app_name: &str) -> PathBuf {
        let resource_path = dir.join(app_name).join("Contents/MacOS/loof");
        std::fs::create_dir_all(resource_path.parent().unwrap()).unwrap();
        std::fs::write(&resource_path, app_name).unwrap();
        resource_path
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn classifies_missing_install() {
        let dir = tempfile::tempdir().unwrap();
        let resource_path = write_app_bundle_cli(dir.path(), "Loofah.app");

        let state = classify_installation(&dir.path().join("loof"), &resource_path).unwrap();
        assert_eq!(state, EmbeddedCliState::Missing);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn classifies_symlink_into_current_app_as_installed() {
        let dir = tempfile::tempdir().unwrap();
        let resource_path = write_app_bundle_cli(dir.path(), "Loofah.app");
        let install_path = dir.path().join("loof");
        std::os::unix::fs::symlink(&resource_path, &install_path).unwrap();

        let state = classify_installation(&install_path, &resource_path).unwrap();
        assert_eq!(state, EmbeddedCliState::Installed);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn classifies_legacy_managed_copy_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let resource_path = write_app_bundle_cli(dir.path(), "Loofah.app");
        let legacy_path = dir.path().join(".fmtr-cli/fmtr/1.2.0");
        std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        std::fs::write(&legacy_path, "old cli").unwrap();
        let install_path = dir.path().join("loof");
        std::os::unix::fs::symlink(&legacy_path, &install_path).unwrap();

        let state = classify_installation(&install_path, &resource_path).unwrap();
        assert_eq!(state, EmbeddedCliState::Missing);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn classifies_dangling_symlink_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let resource_path = write_app_bundle_cli(dir.path(), "Loofah.app");
        let install_path = dir.path().join("loof");
        std::os::unix::fs::symlink(dir.path().join("gone"), &install_path).unwrap();

        let state = classify_installation(&install_path, &resource_path).unwrap();
        assert_eq!(state, EmbeddedCliState::Missing);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn classifies_symlink_into_old_app_bundle_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let old_resource_path = write_app_bundle_cli(dir.path(), "Old.app");
        let new_resource_path = write_app_bundle_cli(dir.path(), "New.app");
        let install_path = dir.path().join("loof");
        std::os::unix::fs::symlink(&old_resource_path, &install_path).unwrap();

        let state = classify_installation(&install_path, &new_resource_path).unwrap();
        assert_eq!(state, EmbeddedCliState::Missing);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn classifies_foreign_symlink_as_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let resource_path = write_app_bundle_cli(dir.path(), "Loofah.app");
        let foreign_target = dir.path().join("other-tool");
        std::fs::write(&foreign_target, "not ours").unwrap();
        let install_path = dir.path().join("loof");
        std::os::unix::fs::symlink(&foreign_target, &install_path).unwrap();

        let state = classify_installation(&install_path, &resource_path).unwrap();
        assert_eq!(state, EmbeddedCliState::Conflict);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn classifies_regular_file_as_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let resource_path = write_app_bundle_cli(dir.path(), "Loofah.app");
        let install_path = dir.path().join("loof");
        std::fs::write(&install_path, "other").unwrap();

        let state = classify_installation(&install_path, &resource_path).unwrap();
        assert_eq!(state, EmbeddedCliState::Conflict);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn install_symlinks_directly_into_the_app_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let resource_path = write_app_bundle_cli(dir.path(), "Loofah.app");
        let install_path = dir.path().join("home/.local/bin/loof");

        install_symlink(&resource_path, &install_path).unwrap();

        assert_eq!(std::fs::read_link(&install_path).unwrap(), resource_path);
        assert_eq!(
            classify_installation(&install_path, &resource_path).unwrap(),
            EmbeddedCliState::Installed
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reinstall_repoints_symlink_after_app_move() {
        let dir = tempfile::tempdir().unwrap();
        let old_resource_path = write_app_bundle_cli(dir.path(), "Old.app");
        let new_resource_path = write_app_bundle_cli(dir.path(), "New.app");
        let install_path = dir.path().join("home/.local/bin/loof");
        install_symlink(&old_resource_path, &install_path).unwrap();
        std::fs::remove_dir_all(dir.path().join("Old.app")).unwrap();

        assert_eq!(
            classify_installation(&install_path, &new_resource_path).unwrap(),
            EmbeddedCliState::Missing
        );

        install_symlink(&new_resource_path, &install_path).unwrap();
        assert_eq!(std::fs::read_to_string(&install_path).unwrap(), "New.app");
        assert_eq!(
            classify_installation(&install_path, &new_resource_path).unwrap(),
            EmbeddedCliState::Installed
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn installer_refuses_to_replace_foreign_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let resource_path = write_app_bundle_cli(dir.path(), "Loofah.app");
        let foreign_target = dir.path().join("other-tool");
        std::fs::write(&foreign_target, "not ours").unwrap();
        let install_path = dir.path().join("loof");
        std::os::unix::fs::symlink(&foreign_target, &install_path).unwrap();

        assert!(install_symlink(&resource_path, &install_path).is_err());
        assert_eq!(std::fs::read_link(&install_path).unwrap(), foreign_target);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn install_prunes_legacy_managed_copies() {
        let dir = tempfile::tempdir().unwrap();
        let resource_path = write_app_bundle_cli(dir.path(), "Loofah.app");
        let install_path = dir.path().join("home/.local/bin/loof");
        let legacy_path = dir.path().join("home/.local/bin/.fmtr-cli/fmtr/1.2.0");
        std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        std::fs::write(&legacy_path, "old cli").unwrap();
        std::fs::create_dir_all(install_path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&legacy_path, &install_path).unwrap();

        install_symlink(&resource_path, &install_path).unwrap();
        remove_legacy_managed_copies(&install_path, "fmtr");

        assert_eq!(std::fs::read_link(&install_path).unwrap(), resource_path);
        assert!(!dir.path().join("home/.local/bin/.fmtr-cli").exists());
    }
}

#[cfg(target_os = "windows")]
pub fn uninstall_windows(identifier: &str) -> Result<(), String> {
    windows::uninstall(identifier)
}

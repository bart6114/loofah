use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MANIFEST: &str = ".loofah-cli.json";

#[derive(Deserialize, Serialize, PartialEq, Eq)]
struct Installation {
    files: BTreeMap<String, String>,
}

fn digest(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn inventory(dir: &Path) -> Result<Installation, String> {
    if std::fs::symlink_metadata(dir)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("The CLI directory is a link owned by another installation.".into());
    }
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            return Err("The CLI directory contains an unmanaged directory or link.".into());
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "Invalid CLI file name")?;
        if name != MANIFEST {
            files.insert(name, digest(&entry.path())?);
        }
    }
    Ok(Installation { files })
}

fn owned_installation(dir: &Path) -> Result<Option<Installation>, String> {
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
        Ok(_) => {}
    }
    let actual = inventory(dir)?;
    let bytes = std::fs::read(dir.join(MANIFEST))
        .map_err(|_| "The CLI directory contains files not managed by Loofah.")?;
    let expected: Installation = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if actual != expected || actual.files.is_empty() {
        return Err(
            "The installed CLI files were changed outside Loofah. Move them before reinstalling."
                .into(),
        );
    }
    Ok(Some(actual))
}

fn source_files(source: &Path, command: &str) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut files = BTreeMap::from([(format!("{command}.exe"), source.to_owned())]);
    for entry in std::fs::read_dir(source.parent().ok_or("Invalid bundled CLI path")?)
        .map_err(|e| e.to_string())?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
        {
            files.insert(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "Invalid DLL name")?,
                entry.path(),
            );
        }
    }
    Ok(files)
}

fn install_files(source: &Path, bin: &Path, command: &str) -> Result<(), String> {
    let previous = owned_installation(bin)?;
    let parent = bin.parent().ok_or("Invalid CLI install path")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(parent)
        .map_err(|e| e.to_string())?;
    let prepared = staging.path().join("bin");
    std::fs::create_dir(&prepared).map_err(|e| e.to_string())?;
    for (name, path) in source_files(source, command)? {
        let destination = prepared.join(name);
        std::fs::copy(path, &destination).map_err(|e| e.to_string())?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(destination)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())?;
    }
    let manifest = inventory(&prepared)?;
    let bytes = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(prepared.join(MANIFEST), bytes).map_err(|e| e.to_string())?;

    // Recheck ownership after copying; never remove a foreign command or DLL.
    if owned_installation(bin)? != previous {
        return Err("The CLI installation changed while preparing its update. Try again.".into());
    }
    let backup = staging.path().join("previous");
    if previous.is_some() {
        std::fs::rename(bin, &backup)
            .map_err(|e| format!("Close running CLI commands and try again: {e}"))?;
    }
    if let Err(error) = std::fs::rename(&prepared, bin) {
        if previous.is_some() && std::fs::rename(&backup, bin).is_err() {
            let recovery = staging.keep();
            return Err(format!(
                "CLI update failed: {error}. The previous installation is preserved in {}.",
                recovery.join("previous").display()
            ));
        }
        return Err(error.to_string());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn bin_dir(command: &str) -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("Loofah").join("cli").join(command).join("bin"))
}

#[cfg(target_os = "windows")]
fn resource_path<R: tauri::Runtime, T: tauri::Manager<R>>(manager: &T) -> Option<PathBuf> {
    let sidecar = std::env::current_exe().ok()?.parent()?.join("loof.exe");
    if sidecar.is_file() {
        return Some(sidecar);
    }
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let name = format!("cli/loof-{arch}-pc-windows-msvc.exe");
    manager
        .path()
        .resolve(&name, tauri::path::BaseDirectory::Resource)
        .ok()
        .filter(|path| path.is_file())
        .or_else(|| {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join(name);
            path.is_file().then_some(path)
        })
}

#[cfg(target_os = "windows")]
pub(super) fn check<R: tauri::Runtime, T: tauri::Manager<R>>(
    manager: &T,
) -> super::EmbeddedCliStatus {
    use super::{EmbeddedCliState as State, EmbeddedCliStatus};
    let command = super::command_name_from_identifier(manager.config().identifier.as_ref());
    let Some(bin) = bin_dir(command) else {
        return super::unavailable_status(
            command,
            "Loofah could not find your local application directory.",
        );
    };
    let result = (|| {
        let Some(source) = resource_path(manager) else {
            return Ok((
                State::ResourceMissing,
                "The CLI is not included in this build of Loofah.".to_string(),
            ));
        };
        let Some(installed) = owned_installation(&bin)? else {
            return Ok((
                State::Missing,
                "Install the CLI for your Windows account.".to_string(),
            ));
        };
        let expected = source_files(&source, command)?
            .into_iter()
            .map(|(name, path)| digest(&path).map(|hash| (name, hash)))
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        if installed.files != expected {
            return Ok((
                State::Missing,
                "Update the installed CLI to match this app.".to_string(),
            ));
        }
        Ok((State::Installed, "Installed for your Windows account. Open a new terminal to use the command; it updates with this app.".to_string()))
    })();
    let (state, details) = result.unwrap_or_else(|error: String| (State::Conflict, error));
    EmbeddedCliStatus {
        supported: true,
        command_name: command.to_string(),
        install_path: bin.join(format!("{command}.exe")).display().to_string(),
        state,
        details: Some(details),
    }
}

fn with_path_entry(current: &str, entry: &str) -> String {
    if current.split(';').any(|part| {
        part.trim()
            .trim_matches('"')
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(entry.trim_end_matches('\\'))
    }) {
        return current.to_string();
    }
    if current.is_empty() {
        entry.to_string()
    } else {
        format!("{};{entry}", current.trim_end_matches(';'))
    }
}

fn without_path_entry(current: &str, entry: &str) -> String {
    current
        .split(';')
        .filter(|part| {
            !part
                .trim()
                .trim_matches('"')
                .trim_end_matches('\\')
                .eq_ignore_ascii_case(entry.trim_end_matches('\\'))
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn uninstall_files(bin: &Path) -> Result<(), String> {
    let Some(previous) = owned_installation(bin)? else {
        return Ok(());
    };
    let staging = tempfile::Builder::new()
        .prefix(".uninstall-")
        .tempdir_in(bin.parent().ok_or("Invalid CLI install path")?)
        .map_err(|e| e.to_string())?;
    let moved = staging.path().join("bin");
    std::fs::rename(bin, &moved)
        .map_err(|e| format!("Close running CLI commands and try again: {e}"))?;
    // Verify after the rename, before deleting anything, to preserve concurrent edits.
    if owned_installation(&moved).ok().flatten().as_ref() != Some(&previous) {
        if std::fs::rename(&moved, bin).is_err() {
            let recovery = staging.keep();
            return Err(format!(
                "Changed CLI files were preserved at {}",
                recovery.display()
            ));
        }
        return Err("The CLI installation changed during uninstall; its files were kept.".into());
    }
    if let Err(error) = std::fs::remove_dir_all(&moved) {
        let recovery = staging.keep();
        return Err(format!(
            "CLI cleanup failed: {error}. Remaining files are at {}",
            recovery.display()
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub(super) fn uninstall(identifier: &str) -> Result<(), String> {
    let command = super::command_name_from_identifier(identifier);
    let bin = bin_dir(command).ok_or("The local application directory could not be found.")?;
    uninstall_files(&bin)?;
    update_user_path(&bin, false)
}

#[cfg(target_os = "windows")]
fn update_user_path(bin: &Path, add: bool) -> Result<(), String> {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::{
            HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
        },
    };
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ},
    };
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        .map_err(|e| e.to_string())?;
    let old = match key.get_raw_value("Path") {
        Ok(value) if value.vtype == REG_SZ || value.vtype == REG_EXPAND_SZ => Some(value),
        Ok(_) => return Err("Your user PATH uses an unsupported registry type.".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let current: String = if old.is_some() {
        key.get_value("Path").map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    let updated = if add {
        with_path_entry(&current, &bin.display().to_string())
    } else {
        without_path_entry(&current, &bin.display().to_string())
    };
    if updated != current {
        let bytes = updated
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        key.set_raw_value(
            "Path",
            &winreg::RegValue {
                bytes,
                vtype: old.map_or(REG_EXPAND_SZ, |value| value.vtype),
            },
        )
        .map_err(|e| e.to_string())?;
        let environment = windows::core::w!("Environment");
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(environment.as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                1000,
                None,
            );
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub(super) fn install<R: tauri::Runtime, T: tauri::Manager<R>>(
    manager: &T,
) -> Result<super::EmbeddedCliStatus, String> {
    static INSTALL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = INSTALL.lock().map_err(|e| e.to_string())?;
    let command = super::command_name_from_identifier(manager.config().identifier.as_ref());
    let source = resource_path(manager).ok_or("The bundled CLI could not be found.")?;
    let bin = bin_dir(command).ok_or("The local application directory could not be found.")?;
    install_files(&source, &bin, command)?;
    update_user_path(&bin, true)?;
    Ok(check(manager))
}

#[cfg(target_os = "windows")]
pub(super) fn sync_installed<R: tauri::Runtime, T: tauri::Manager<R>>(manager: &T) {
    let command = super::command_name_from_identifier(manager.config().identifier.as_ref());
    if bin_dir(command).is_some_and(|bin| bin.join(MANIFEST).is_file()) {
        if matches!(check(manager).state, super::EmbeddedCliState::Installed) {
            if let Some(bin) = bin_dir(command) {
                if let Err(error) = update_user_path(&bin, true) {
                    tracing::warn!(%error, "failed to refresh the Windows CLI PATH entry");
                }
            }
            return;
        }
        if let Err(error) = install(manager) {
            tracing::warn!(%error, "failed to update the installed Windows CLI");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_and_updates_executable_and_runtime_dlls() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundled");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("loof.exe"), "first").unwrap();
        std::fs::write(source.join("runtime.dll"), "library").unwrap();
        let bin = dir.path().join("managed/bin");
        install_files(&source.join("loof.exe"), &bin, "loof-staging").unwrap();
        assert_eq!(
            std::fs::read_to_string(bin.join("loof-staging.exe")).unwrap(),
            "first"
        );
        assert!(bin.join("runtime.dll").is_file());
        std::fs::write(source.join("loof.exe"), "second").unwrap();
        install_files(&source.join("loof.exe"), &bin, "loof-staging").unwrap();
        assert_eq!(
            std::fs::read_to_string(bin.join("loof-staging.exe")).unwrap(),
            "second"
        );
        assert_eq!(std::fs::read_dir(bin.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn preserves_modified_installations_and_unknown_files() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("loof.exe");
        std::fs::write(&source, "bundled").unwrap();
        let bin = dir.path().join("managed/bin");
        install_files(&source, &bin, "loof").unwrap();
        std::fs::write(bin.join("loof.exe"), "user changes").unwrap();
        assert!(install_files(&source, &bin, "loof").is_err());
        assert_eq!(
            std::fs::read_to_string(bin.join("loof.exe")).unwrap(),
            "user changes"
        );
        std::fs::write(bin.join("loof.exe"), "bundled").unwrap();
        std::fs::write(bin.join("personal.txt"), "keep").unwrap();
        assert!(install_files(&source, &bin, "loof").is_err());
        assert!(bin.join("personal.txt").exists());
    }

    #[test]
    fn uninstall_preserves_foreign_files_and_removes_only_owned_installations() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("loof.exe");
        std::fs::write(&source, "bundled").unwrap();
        let bin = dir.path().join("managed/bin");
        install_files(&source, &bin, "loof").unwrap();
        std::fs::write(bin.join("personal.txt"), "keep").unwrap();
        assert!(uninstall_files(&bin).is_err());
        assert!(bin.join("loof.exe").is_file());
        std::fs::remove_file(bin.join("personal.txt")).unwrap();
        uninstall_files(&bin).unwrap();
        assert!(!bin.exists());
        uninstall_files(&bin).unwrap();
        assert_eq!(
            without_path_entry(
                r#"C:\Tools;"C:\LOOFAH\bin\";%USERPROFILE%\bin"#,
                r"C:\Loofah\bin"
            ),
            r"C:\Tools;%USERPROFILE%\bin"
        );
    }

    #[test]
    fn user_path_is_preserved_and_case_insensitively_deduplicated() {
        assert_eq!(
            with_path_entry(r"%USERPROFILE%\bin;C:\Tools", r"C:\Loofah\bin"),
            r"%USERPROFILE%\bin;C:\Tools;C:\Loofah\bin"
        );
        assert_eq!(
            with_path_entry(r#"C:\Tools;"C:\LOOFAH\bin\""#, r"C:\Loofah\bin"),
            r#"C:\Tools;"C:\LOOFAH\bin\""#
        );
    }
}

use winreg::{
    RegKey,
    enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_BINARY},
};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

fn command(exe: &std::path::Path) -> String {
    format!("\"{}\" --background", exe.display())
}

fn owned(value: &str, exe: &std::path::Path) -> bool {
    value == command(exe) || value == format!("{} --background", exe.display())
}

#[tauri::command]
fn enable<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    let name = super::current_name(&app.config().identifier);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let user = RegKey::predef(HKEY_CURRENT_USER);
    user.create_subkey(RUN)
        .map_err(|e| e.to_string())?
        .0
        .set_value(name, &command(&exe))
        .map_err(|e| e.to_string())?;
    if let Ok(key) = user.open_subkey_with_flags(APPROVED, KEY_SET_VALUE) {
        key.set_raw_value(
            name,
            &winreg::RegValue {
                vtype: REG_BINARY,
                bytes: vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            },
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn disable<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    uninstall(&app.config().identifier)
}

#[tauri::command]
fn is_enabled<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<bool, String> {
    let name = super::current_name(&app.config().identifier);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let value = user
        .open_subkey(RUN)
        .and_then(|key| key.get_value::<String, _>(name));
    match value {
        Ok(value) if owned(&value, &exe) => {}
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.to_string()),
    }
    let approval = user
        .open_subkey(APPROVED)
        .and_then(|key| key.get_raw_value(name))
        .ok();
    Ok(approval.is_none_or(|value| {
        value.bytes.len() < 8 || value.bytes.iter().rev().take(8).all(|byte| *byte == 0)
    }))
}

pub fn uninstall(identifier: &str) -> Result<(), String> {
    let name = super::current_name(identifier);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let key = match user.open_subkey_with_flags(RUN, KEY_READ | KEY_SET_VALUE) {
        Ok(key) => key,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    match key.get_value::<String, _>(name) {
        Ok(value) if owned(&value, &exe) => {
            key.delete_value(name).map_err(|e| e.to_string())?;
            if let Ok(approval) = user.open_subkey_with_flags(APPROVED, KEY_SET_VALUE) {
                let _ = approval.delete_value(name);
            }
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    Ok(())
}

pub(super) fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("autostart")
        .invoke_handler(tauri::generate_handler![enable, disable, is_enabled])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_command_quotes_spaces_and_cleanup_checks_ownership() {
        let exe = std::path::Path::new(r"C:\Users\Test User\Loofah\loofah.exe");
        assert_eq!(
            command(exe),
            r#""C:\Users\Test User\Loofah\loofah.exe" --background"#
        );
        assert!(owned(&command(exe), exe));
        assert!(owned(&format!("{} --background", exe.display()), exe));
        assert!(!owned(r#""C:\Other\loofah.exe" --background"#, exe));
    }
}

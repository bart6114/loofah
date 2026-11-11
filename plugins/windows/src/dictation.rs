use std::sync::OnceLock;

use tauri_specta::Event;

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

pub fn set_app_handle(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

#[cfg(target_os = "macos")]
mod platform {
    use swift_rs::{Bool, Int, SRString, swift};

    swift!(fn _dictation_shortcut_configure(shortcut: &SRString) -> Bool);
    swift!(fn _dictation_capture_target() -> Int);
    swift!(fn _dictation_insert(text: &SRString) -> Bool);

    pub fn configure(shortcut: &str) -> Result<(), String> {
        let shortcut = SRString::from(shortcut);
        if unsafe { _dictation_shortcut_configure(&shortcut) } {
            Ok(())
        } else {
            Err("Could not register the global shortcut. Check Input Monitoring permission.".into())
        }
    }

    pub fn capture_target() -> Result<(), String> {
        match unsafe { _dictation_capture_target() } {
            0 => Ok(()),
            1 => Err("accessibility_permission_required".into()),
            _ => Err("Focus an editable text field before dictating.".into()),
        }
    }

    pub fn insert(text: &str) -> Result<(), String> {
        let text = SRString::from(text);
        if unsafe { _dictation_insert(&text) } {
            Ok(())
        } else {
            Err("The original text field is no longer focused. Use Copy Last Dictation in the menu bar.".into())
        }
    }
}

#[cfg(target_os = "windows")]
#[path = "dictation_windows.rs"]
mod platform;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    pub fn configure(_shortcut: &str) -> Result<(), String> {
        Err("Unsupported platform".into())
    }
    pub fn capture_target() -> Result<(), String> {
        Err("Unsupported platform".into())
    }
    pub fn insert(_text: &str) -> Result<(), String> {
        Err("Unsupported platform".into())
    }
}

pub fn configure(shortcut: &str) -> Result<(), String> {
    platform::configure(shortcut)
}

pub fn dictation_capture_target() -> Result<(), String> {
    platform::capture_target()
}

pub fn dictation_insert(text: &str) -> Result<(), String> {
    platform::insert(text)
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_on_dictation_key_down() {
    if let Some(app) = APP.get() {
        let _ = crate::events::DictationShortcutDown {}.emit(app);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_on_dictation_key_up() {
    if let Some(app) = APP.get() {
        let _ = crate::events::DictationShortcutUp {}.emit(app);
    }
}

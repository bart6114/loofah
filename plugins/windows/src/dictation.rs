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
    swift!(fn _dictation_set_active(active: Bool));
    swift!(fn _dictation_insert(text: &SRString) -> Int);

    pub fn configure(shortcut: &str) -> Result<(), String> {
        let shortcut = SRString::from(shortcut);
        if unsafe { _dictation_shortcut_configure(&shortcut) } {
            Ok(())
        } else {
            Err("Could not register the global shortcut. Check Input Monitoring permission.".into())
        }
    }

    pub fn set_active(active: bool) {
        unsafe { _dictation_set_active(active.into()) };
    }

    pub fn insert(text: &str) -> Result<(), String> {
        let text = SRString::from(text);
        match unsafe { _dictation_insert(&text) } {
            0 => Ok(()),
            1 => Err("accessibility_permission_required".into()),
            2 => Err("target_unavailable".into()),
            _ => Err("Could not insert dictation into this field".into()),
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
    pub fn set_active(_active: bool) {}
    pub fn insert(_text: &str) -> Result<(), String> {
        Err("Unsupported platform".into())
    }
}

pub fn configure(shortcut: &str) -> Result<(), String> {
    platform::configure(shortcut)
}

pub fn dictation_set_active(active: bool) {
    platform::set_active(active);
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

#[unsafe(no_mangle)]
pub extern "C" fn rust_on_dictation_cancel() {
    if let Some(app) = APP.get() {
        let _ = crate::events::DictationCancel {}.emit(app);
    }
}

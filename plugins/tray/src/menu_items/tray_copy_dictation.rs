use tauri::{
    AppHandle, Result,
    menu::{MenuItem, MenuItemKind},
};
use tauri_plugin_clipboard_manager::ClipboardExt;

use super::MenuItemHandler;

pub struct TrayCopyDictation;

impl MenuItemHandler for TrayCopyDictation {
    const ID: &'static str = "hypr_tray_copy_dictation";

    fn build(app: &AppHandle<tauri::Wry>) -> Result<MenuItemKind<tauri::Wry>> {
        Ok(MenuItemKind::MenuItem(MenuItem::with_id(
            app,
            Self::ID,
            "Copy Last Dictation",
            true,
            None::<&str>,
        )?))
    }

    fn handle(app: &AppHandle<tauri::Wry>) {
        if let Some(text) = crate::ext::latest_dictation() {
            let _ = app.clipboard().write_text(text);
        }
    }
}

use serde::{Deserialize, Serialize};

use super::{floating_bar::FloatingBarState, live_caption::LiveCaptionState};

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(tag = "type", content = "state", rename_all = "camelCase")]
pub enum OverlayState {
    FloatingBar(FloatingBarState),
    Transcript(FloatingBarState),
    LiveCaption(LiveCaptionState),
    Devtools,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, specta::Type)]
pub struct OverlaySnapshot {
    pub revision: u32,
    pub state: Option<OverlayState>,
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use crate::{Error, window::live_caption::LiveCaptionPosition};
    use std::{
        collections::HashMap,
        sync::{LazyLock, Mutex, OnceLock},
    };
    use tauri::{Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
    static STATES: LazyLock<Mutex<HashMap<String, OverlaySnapshot>>> =
        LazyLock::new(Mutex::default);

    pub fn set_app_handle(app: tauri::AppHandle) {
        let _ = APP.set(app);
    }

    fn app() -> Result<&'static tauri::AppHandle, Error> {
        APP.get()
            .ok_or_else(|| Error::PanelError("Windows overlays are not initialized".into()))
    }

    fn failure(error: impl std::fmt::Display) -> Error {
        Error::PanelError(error.to_string())
    }

    pub fn snapshot(label: &str) -> OverlaySnapshot {
        STATES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(label)
            .cloned()
            .unwrap_or_default()
    }

    fn window(label: &str) -> Result<WebviewWindow, Error> {
        let app = app()?;
        if let Some(window) = app.get_webview_window(label) {
            return Ok(window);
        }
        WebviewWindowBuilder::new(app, label, WebviewUrl::App("overlay.html".into()))
            .title("Loofah")
            .inner_size(520.0, 76.0)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .visible(false)
            .build()
            .map_err(failure)
    }

    pub fn show(label: &str) -> Result<(), Error> {
        let window = window(label)?;
        layout(&window, &snapshot(label).state, true)?;
        window.show().map_err(failure)
    }

    pub fn hide(label: &str) -> Result<(), Error> {
        if let Some(window) = app()?.get_webview_window(label) {
            window.hide().map_err(failure)?;
        }
        if label == "recording-bar" {
            hide("recording-captions")?;
        }
        Ok(())
    }

    pub fn update(label: &str, state: OverlayState) -> Result<(), Error> {
        let snapshot = {
            let mut states = STATES.lock().unwrap_or_else(|e| e.into_inner());
            let snapshot = states.entry(label.into()).or_default();
            snapshot.revision = snapshot.revision.wrapping_add(1);
            snapshot.state = Some(state.clone());
            snapshot.clone()
        };
        if let Some(window) = app()?.get_webview_window(label) {
            layout(&window, &snapshot.state, false)?;
            window
                .emit("loofah-overlay-state", snapshot)
                .map_err(failure)?;
        }
        if let OverlayState::FloatingBar(state) = state {
            let visible = app()?
                .get_webview_window("recording-bar")
                .is_some_and(|w| w.is_visible().unwrap_or(false));
            if visible && state.live_caption_toggle_visible && !state.live_caption_minimized {
                let existing = app()?
                    .get_webview_window("recording-captions")
                    .is_some_and(|w| w.is_visible().unwrap_or(false));
                update("recording-captions", OverlayState::Transcript(state))?;
                if !existing {
                    show("recording-captions")?;
                }
            } else {
                hide("recording-captions")?;
            }
        }
        Ok(())
    }

    fn layout(
        window: &WebviewWindow,
        state: &Option<OverlayState>,
        force: bool,
    ) -> Result<(), Error> {
        let monitor = window
            .current_monitor()
            .map_err(failure)?
            .or_else(|| {
                app()
                    .ok()?
                    .get_webview_window("main")?
                    .current_monitor()
                    .ok()
                    .flatten()
            })
            .or_else(|| window.primary_monitor().ok().flatten())
            .ok_or_else(|| failure("No display available"))?;
        let scale = monitor.scale_factor();
        let area = monitor.work_area();
        let width_limit = area.size.width as f64 / scale;
        let height_limit = area.size.height as f64 / scale;
        let (width, height, position) = match state {
            Some(OverlayState::Transcript(s)) => (
                s.live_caption_width,
                58.0 + s.live_caption_line_count.clamp(1, 12) as f64 * 30.0,
                s.live_caption_position,
            ),
            Some(OverlayState::LiveCaption(s)) => (
                s.width,
                if s.minimized {
                    52.0
                } else {
                    32.0 + s.line_count.clamp(1, 12) as f64 * 30.0
                },
                s.position,
            ),
            Some(OverlayState::Devtools) => (350.0, 360.0, LiveCaptionPosition::TopRight),
            _ => (520.0, 76.0, LiveCaptionPosition::BottomCenter),
        };
        let width = width
            .clamp(280.0, 1200.0)
            .min((width_limit - 24.0).max(1.0));
        let height = height.min((height_limit - 24.0).max(1.0));
        let size = tauri::LogicalSize::new(width, height).to_physical::<u32>(scale);
        let current_size = window.inner_size().map_err(failure)?;
        let current_position = window.outer_position().map_err(failure)?;
        let outside = current_position.x < area.position.x
            || current_position.y < area.position.y
            || current_position.x as i64 + size.width as i64
                > area.position.x as i64 + area.size.width as i64
            || current_position.y as i64 + size.height as i64
                > area.position.y as i64 + area.size.height as i64;
        if current_size != size {
            window.set_size(size).map_err(failure)?;
        }
        if force || current_size != size || outside || window.label() == "recording-captions" {
            let x = match position {
                LiveCaptionPosition::TopLeft | LiveCaptionPosition::BottomLeft => 12.0,
                LiveCaptionPosition::TopRight | LiveCaptionPosition::BottomRight => {
                    width_limit - width - 12.0
                }
                _ => (width_limit - width) / 2.0,
            };
            let y = match position {
                LiveCaptionPosition::TopLeft
                | LiveCaptionPosition::TopCenter
                | LiveCaptionPosition::TopRight => 12.0,
                _ => (height_limit
                    - height
                    - if window.label() == "recording-captions" {
                        100.0
                    } else {
                        12.0
                    })
                .max(12.0),
            };
            window
                .set_position(tauri::PhysicalPosition::new(
                    area.position.x + (x * scale) as i32,
                    area.position.y + (y * scale) as i32,
                ))
                .map_err(failure)?;
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub use platform::*;

#[cfg(not(target_os = "windows"))]
pub fn snapshot(_: &str) -> OverlaySnapshot {
    OverlaySnapshot::default()
}

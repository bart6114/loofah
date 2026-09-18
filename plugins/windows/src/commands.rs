use crate::{AppWindow, SavedFrames, WindowImpl, WindowsPluginExt, events};

use tauri::Manager;

#[tauri::command]
#[specta::specta]
pub async fn window_show(
    app: tauri::AppHandle<tauri::Wry>,
    window: AppWindow,
) -> Result<(), String> {
    app.windows()
        .show_async(window)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn window_hide(
    app: tauri::AppHandle<tauri::Wry>,
    window: AppWindow,
) -> Result<(), String> {
    app.windows().hide(window).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn window_destroy(
    app: tauri::AppHandle<tauri::Wry>,
    window: AppWindow,
) -> Result<(), String> {
    app.windows().destroy(window).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_show_app_in_dock(
    app: tauri::AppHandle<tauri::Wry>,
    show: bool,
) -> Result<(), String> {
    app.windows()
        .set_show_app_in_dock(show)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn window_navigate(
    app: tauri::AppHandle<tauri::Wry>,
    window: AppWindow,
    path: String,
) -> Result<(), String> {
    app.windows()
        .navigate(window, path)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn window_emit_navigate(
    app: tauri::AppHandle<tauri::Wry>,
    window: AppWindow,
    event: events::Navigate,
) -> Result<(), String> {
    app.windows()
        .emit_navigate(window, event)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(serde::Deserialize, specta::Type)]
pub enum Anchor {
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
    Center,
}

#[tauri::command]
#[specta::specta]
pub async fn window_set_frame_animated(
    app: tauri::AppHandle<tauri::Wry>,
    window: AppWindow,
    anchor: Anchor,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let visible_frame = app
        .windows()
        .visible_frame(window.clone())
        .map_err(|e| e.to_string())?;

    if let Some(screen) = visible_frame {
        if matches!(window, AppWindow::Main)
            && let Some(window_handle) = window.get(&app)
        {
            window_handle
                .set_always_on_top(true)
                .map_err(|e| e.to_string())?;
        }

        let margin = 8.0_f64;
        let (x, y) = match anchor {
            Anchor::TopRight => (
                screen.x + screen.w - width - margin,
                screen.y + screen.h - height - margin,
            ),
            Anchor::TopLeft => (screen.x + margin, screen.y + screen.h - height - margin),
            Anchor::BottomRight => (screen.x + screen.w - width - margin, screen.y + margin),
            Anchor::BottomLeft => (screen.x + margin, screen.y + margin),
            Anchor::Center => (
                screen.x + (screen.w - width) / 2.0,
                screen.y + (screen.h - height) / 2.0,
            ),
        };

        let frame = crate::SavedFrame {
            x,
            y: if cfg!(target_os = "windows") {
                screen.y + screen.h - (y - screen.y) - height
            } else {
                y
            },
            w: width,
            h: height,
        };

        app.windows()
            .set_frame_animated(window, frame)
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn window_save_frame(
    app: tauri::AppHandle<tauri::Wry>,
    window: AppWindow,
) -> Result<(), String> {
    let frame = app
        .windows()
        .frame(window.clone())
        .map_err(|e| e.to_string())?;

    if let Some(frame) = frame {
        app.state::<SavedFrames>()
            .0
            .lock()
            .unwrap()
            .insert(window.label(), frame);
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn window_restore_frame_animated(
    app: tauri::AppHandle<tauri::Wry>,
    window: AppWindow,
) -> Result<(), String> {
    let saved = app
        .state::<SavedFrames>()
        .0
        .lock()
        .unwrap()
        .get(&window.label())
        .copied();

    if let Some(saved) = saved {
        app.windows()
            .set_frame_animated(window.clone(), saved)
            .map_err(|e| e.to_string())?;
    }

    if matches!(window, AppWindow::Main)
        && let Some(window_handle) = window.get(&app)
    {
        window_handle
            .set_always_on_top(false)
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn window_expand_width(
    app: tauri::AppHandle<tauri::Wry>,
    window: tauri::Window<tauri::Wry>,
    expansion_px: u32,
    max_current_width: Option<u32>,
    check_monitor_space: bool,
    expand_left: bool,
    restore_on_close: bool,
) -> Result<(), String> {
    if check_monitor_space {
        let outer_size = window.outer_size().map_err(|e| e.to_string())?;
        let outer_position = window.outer_position().map_err(|e| e.to_string())?;
        let monitor = window.current_monitor().map_err(|e| e.to_string())?;

        if let Some(monitor) = monitor {
            #[cfg(target_os = "windows")]
            {
                let area = monitor.work_area();
                let required = (f64::from(expansion_px) * monitor.scale_factor()).round() as i64;
                let available = if expand_left {
                    i64::from(outer_position.x) - i64::from(area.position.x)
                } else {
                    i64::from(area.position.x) + i64::from(area.size.width)
                        - i64::from(outer_position.x)
                        - i64::from(outer_size.width)
                };
                if available < required {
                    return Ok(());
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                let window_right = i64::from(outer_position.x) + i64::from(outer_size.width);
                let monitor_right =
                    i64::from(monitor.position().x) + i64::from(monitor.size().width);

                if monitor_right - window_right < i64::from(expansion_px) {
                    return Ok(());
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        use crate::ext::run_on_main_thread;
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        let expansion = f64::from(expansion_px);
        let label = window.label().to_string();
        let app_clone = app.clone();

        let pushed = run_on_main_thread(&app, move || {
            let Some(win) = app_clone.get_webview_window(&label) else {
                return None;
            };
            let Ok(ns_win_ptr) = win.ns_window() else {
                return None;
            };
            let ns_window = unsafe { &*(ns_win_ptr as *mut objc2_app_kit::NSWindow) };

            let frame = ns_window.frame();

            if max_current_width.is_some_and(|max| frame.size.width >= f64::from(max)) {
                return None;
            }

            let new_width = frame.size.width + expansion;
            let new_origin_x = if expand_left {
                frame.origin.x - expansion
            } else {
                frame.origin.x
            };
            ns_window.setFrame_display(
                NSRect::new(
                    NSPoint::new(new_origin_x, frame.origin.y),
                    NSSize::new(new_width, frame.size.height),
                ),
                false,
            );
            Some((frame.size.width, new_width, expand_left))
        })
        .map_err(|e| e.to_string())?;

        if restore_on_close && let Some(entry) = pushed {
            app.state::<crate::WindowExpansions>()
                .0
                .lock()
                .unwrap()
                .entry(window.label().to_string())
                .or_default()
                .push(entry);
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let scale = window.scale_factor().map_err(|e| e.to_string())?;
        let size = window
            .inner_size()
            .map_err(|e| e.to_string())?
            .to_logical::<f64>(scale);

        if max_current_width.is_some_and(|max| size.width >= f64::from(max)) {
            return Ok(());
        }

        let new_width = size.width + f64::from(expansion_px);
        if expand_left {
            let position = window.outer_position().map_err(|e| e.to_string())?;
            window
                .set_position(tauri::PhysicalPosition::new(
                    position.x - (f64::from(expansion_px) * scale).round() as i32,
                    position.y,
                ))
                .map_err(|e| e.to_string())?;
        }
        window
            .set_size(tauri::LogicalSize {
                width: new_width,
                height: size.height,
            })
            .map_err(|e| e.to_string())?;

        if restore_on_close {
            app.state::<crate::WindowExpansions>()
                .0
                .lock()
                .unwrap()
                .entry(window.label().to_string())
                .or_default()
                .push((size.width, new_width, expand_left));
        }
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn window_restore_width(
    app: tauri::AppHandle<tauri::Wry>,
    window: tauri::Window<tauri::Wry>,
) -> Result<(), String> {
    let entry = app
        .state::<crate::WindowExpansions>()
        .0
        .lock()
        .unwrap()
        .get_mut(&window.label().to_string())
        .and_then(|stack| stack.pop());

    let Some((previous_w, expanded_w, expand_left)) = entry else {
        return Ok(());
    };

    #[cfg(target_os = "macos")]
    {
        use crate::ext::run_on_main_thread;
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        let label = window.label().to_string();
        let app_clone = app.clone();

        run_on_main_thread(&app, move || {
            let Some(win) = app_clone.get_webview_window(&label) else {
                return;
            };
            let Ok(ns_win_ptr) = win.ns_window() else {
                return;
            };
            let ns_window = unsafe { &*(ns_win_ptr as *mut objc2_app_kit::NSWindow) };

            let frame = ns_window.frame();
            if (frame.size.width - expanded_w).abs() < 1.0 {
                let restore_origin_x = if expand_left {
                    frame.origin.x + (expanded_w - previous_w)
                } else {
                    frame.origin.x
                };
                ns_window.setFrame_display(
                    NSRect::new(
                        NSPoint::new(restore_origin_x, frame.origin.y),
                        NSSize::new(previous_w, frame.size.height),
                    ),
                    false,
                );
            }
        })
        .map_err(|e| e.to_string())?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        let scale = window.scale_factor().map_err(|e| e.to_string())?;
        let size = window
            .inner_size()
            .map_err(|e| e.to_string())?
            .to_logical::<f64>(scale);
        if (size.width - expanded_w).abs() < 1.0 {
            if expand_left {
                let position = window.outer_position().map_err(|e| e.to_string())?;
                window
                    .set_position(tauri::PhysicalPosition::new(
                        position.x + ((expanded_w - previous_w) * scale).round() as i32,
                        position.y,
                    ))
                    .map_err(|e| e.to_string())?;
            }
            window
                .set_size(tauri::LogicalSize {
                    width: previous_w,
                    height: size.height,
                })
                .map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn floating_bar_show() -> Result<(), String> {
    crate::window::floating_bar::show().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn floating_bar_hide() -> Result<(), String> {
    crate::window::floating_bar::hide().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn floating_bar_update(
    state: crate::window::floating_bar::FloatingBarState,
) -> Result<(), String> {
    crate::window::floating_bar::update(state).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn live_caption_show() -> Result<(), String> {
    crate::window::live_caption::show().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn live_caption_hide() -> Result<(), String> {
    crate::window::live_caption::hide().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn live_caption_update(
    state: crate::window::live_caption::LiveCaptionState,
) -> Result<(), String> {
    crate::window::live_caption::update(state).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn devtools_panel_show() -> Result<(), String> {
    crate::window::devtools_panel::show().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn devtools_panel_hide() -> Result<(), String> {
    crate::window::devtools_panel::hide().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn window_is_exists(
    app: tauri::AppHandle<tauri::Wry>,
    window: AppWindow,
) -> Result<bool, String> {
    let exists = app.windows().is_exists(window).map_err(|e| e.to_string())?;
    Ok(exists)
}

#[tauri::command]
#[specta::specta]
pub async fn window_is_occluded(
    app: tauri::AppHandle<tauri::Wry>,
    window: AppWindow,
) -> Result<bool, String> {
    let occluded = app
        .windows()
        .is_occluded(window)
        .map_err(|e| e.to_string())?;
    Ok(occluded)
}

#[tauri::command]
#[specta::specta]
pub async fn overlay_snapshot(
    window: tauri::WebviewWindow,
) -> crate::window::overlay::OverlaySnapshot {
    crate::window::overlay::snapshot(window.label())
}

#[tauri::command]
#[specta::specta]
pub async fn overlay_set_settings_open(open: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    return crate::window::overlay::set_settings_open(open).map_err(|error| error.to_string());
    #[cfg(not(target_os = "windows"))]
    {
        let _ = open;
        Ok(())
    }
}

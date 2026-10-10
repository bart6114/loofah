fn overlay(recording: bool, count: Option<u8>) -> Option<tauri::image::Image<'static>> {
    let label = count.and_then(crate::overlay::notification_badge_label);
    if !recording && label.is_none() {
        return None;
    }
    let mut rgba = vec![0; 32 * 32 * 4];
    for y in 0..32 {
        for x in 0..32 {
            let distance = ((x as f32 - 15.5).powi(2) + (y as f32 - 15.5).powi(2)).sqrt();
            let alpha = ((15.0 - distance).clamp(0.0, 1.0) * 255.0) as u8;
            rgba[(y * 32 + x) * 4..(y * 32 + x + 1) * 4].copy_from_slice(&[220, 38, 38, alpha]);
            if recording && (7.0..9.0).contains(&distance) {
                rgba[(y * 32 + x) * 4..(y * 32 + x + 1) * 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    if !recording {
        if let Some(label) = label {
            let scale = if label.len() > 2 { 2 } else { 3 };
            let width = label.len() * 4 * scale - scale;
            let left = (32 - width) / 2;
            let top = (32 - 5 * scale) / 2;
            for (index, character) in label.chars().enumerate() {
                let rows = match character {
                    '0' => [7, 5, 5, 5, 7],
                    '1' => [2, 6, 2, 2, 7],
                    '2' => [7, 1, 7, 4, 7],
                    '3' => [7, 1, 7, 1, 7],
                    '4' => [5, 5, 7, 1, 1],
                    '5' => [7, 4, 7, 1, 7],
                    '6' => [7, 4, 7, 5, 7],
                    '7' => [7, 1, 1, 1, 1],
                    '8' => [7, 5, 7, 5, 7],
                    '9' => [7, 5, 7, 1, 7],
                    '+' => [0, 2, 7, 2, 0],
                    _ => [0; 5],
                };
                for (row, bits) in rows.into_iter().enumerate() {
                    for column in 0..3 {
                        if bits & (1 << (2 - column)) == 0 {
                            continue;
                        }
                        for dy in 0..scale {
                            for dx in 0..scale {
                                let x = left + index * 4 * scale + column * scale + dx;
                                let y = top + row * scale + dy;
                                rgba[(y * 32 + x) * 4..(y * 32 + x + 1) * 4]
                                    .copy_from_slice(&[255; 4]);
                            }
                        }
                    }
                }
            }
        }
    }
    Some(tauri::image::Image::new_owned(rgba, 32, 32))
}

#[cfg(target_os = "windows")]
mod runtime {
    use std::sync::Mutex;
    use tauri::Manager;

    const LIGHT: &[u8] = include_bytes!("../../../apps/desktop/src-tauri/icons/stable/128x128.png");
    const DARK: &[u8] = include_bytes!("../../../apps/desktop/src-tauri/icons/windows-dark.png");

    #[derive(Clone, Copy, Default)]
    pub(crate) struct State {
        pub dark: bool,
        pub recording: bool,
        pub count: Option<u8>,
    }

    static STATE: Mutex<State> = Mutex::new(State {
        dark: false,
        recording: false,
        count: None,
    });

    pub(crate) fn apply<R: tauri::Runtime>(window: &tauri::Window<R>) -> tauri::Result<()> {
        let state = *STATE.lock().unwrap_or_else(|e| e.into_inner());
        window.set_icon(tauri::image::Image::from_bytes(if state.dark {
            DARK
        } else {
            LIGHT
        })?)?;
        window.set_overlay_icon(super::overlay(state.recording, state.count))
    }

    pub(crate) fn update<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        change: impl FnOnce(&mut State),
    ) -> crate::Result<()> {
        change(&mut STATE.lock().unwrap_or_else(|e| e.into_inner()));
        for window in app.webview_windows().values() {
            apply(&window.as_ref().window())?;
        }
        Ok(())
    }

    pub(crate) fn get_icon() -> String {
        use base64::Engine;
        let dark = STATE.lock().unwrap_or_else(|e| e.into_inner()).dark;
        base64::engine::general_purpose::STANDARD.encode(if dark { DARK } else { LIGHT })
    }
}

#[cfg(target_os = "windows")]
pub(crate) use runtime::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_takes_priority_and_badges_cover_the_full_count_range() {
        assert!(overlay(false, None).is_none());
        assert!(overlay(false, Some(0)).is_none());
        let recording = overlay(true, None).unwrap();
        assert_eq!(recording.rgba(), overlay(true, Some(99)).unwrap().rgba());
        for count in 1..=255 {
            let badge = overlay(false, Some(count)).unwrap();
            assert_eq!(badge.rgba().len(), 32 * 32 * 4);
            assert_eq!(badge.rgba()[3], 0);
        }
    }
}

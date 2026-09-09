use windows::Win32::{
    Foundation::POINT,
    UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, GetWindowThreadProcessId, WindowFromPoint},
};

pub(crate) fn mostly_covered(window: &tauri::WebviewWindow) -> bool {
    let (Ok(position), Ok(size)) = (window.outer_position(), window.outer_size()) else {
        return false;
    };
    let mut covered = 0;
    // Sample the visible surface instead of treating loss of keyboard focus as
    // occlusion: side-by-side meeting and note windows should not show an overlay.
    for row in 0..5 {
        for column in 0..5 {
            let point = POINT {
                x: position.x + ((column * 2 + 1) * size.width as i64 / 10) as i32,
                y: position.y + ((row * 2 + 1) * size.height as i64 / 10) as i32,
            };
            let mut process = 0;
            unsafe {
                let top = GetAncestor(WindowFromPoint(point), GA_ROOT);
                GetWindowThreadProcessId(top, Some(&mut process));
            }
            if process != std::process::id() {
                covered += 1;
            }
        }
    }
    covered >= 15
}

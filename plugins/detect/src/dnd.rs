#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(target_os = "macos")]
pub fn is_do_not_disturb() -> bool {
    match Command::new("defaults")
        .args([
            "read",
            "com.apple.controlcenter",
            "NSStatusItem Visible FocusModes",
        ])
        .output()
    {
        Ok(output) => {
            if output.status.success() {
                let out = String::from_utf8_lossy(&output.stdout);
                out.trim() == "1"
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

#[cfg(target_os = "windows")]
pub fn is_do_not_disturb() -> bool {
    use windows::Win32::UI::Shell::{QUNS_ACCEPTS_NOTIFICATIONS, SHQueryUserNotificationState};
    unsafe { SHQueryUserNotificationState() }.is_ok_and(|state| state != QUNS_ACCEPTS_NOTIFICATIONS)
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn is_do_not_disturb() -> bool {
    false
}

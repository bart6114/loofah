use std::sync::{
    LazyLock, Mutex,
    atomic::{AtomicBool, Ordering},
};

use tauri_specta::Event;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, MSG, SetWindowsHookExW,
    TranslateMessage, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

#[derive(Clone, Copy)]
struct Shortcut {
    key: u32,
    control: bool,
    alt: bool,
    shift: bool,
    command: bool,
    pressed: bool,
}

static SHORTCUT: LazyLock<Mutex<Shortcut>> = LazyLock::new(|| {
    Mutex::new(Shortcut {
        key: 0x20,
        control: true,
        alt: true,
        shift: false,
        command: false,
        pressed: false,
    })
});
static HOOK_STARTED: AtomicBool = AtomicBool::new(false);
static TARGET: Mutex<Option<(i32, Vec<i32>)>> = Mutex::new(None);

pub fn configure(shortcut: &str) -> Result<(), String> {
    let parts = shortcut
        .split('+')
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let key = match parts.last().map(String::as_str) {
        Some("space") => 0x20,
        Some(value) if value.len() == 1 && value.as_bytes()[0].is_ascii_alphanumeric() => {
            value.as_bytes()[0].to_ascii_uppercase() as u32
        }
        _ => return Err("Choose a letter, digit, or Space for the shortcut".into()),
    };
    let mut parsed = Shortcut {
        key,
        control: false,
        alt: false,
        shift: false,
        command: false,
        pressed: false,
    };
    for part in parts.iter().take(parts.len() - 1) {
        match part.as_str() {
            "ctrl" | "control" => parsed.control = true,
            "alt" | "option" => parsed.alt = true,
            "shift" => parsed.shift = true,
            "cmd" | "command" => parsed.command = true,
            _ => return Err("Unsupported shortcut modifier".into()),
        }
    }
    if [parsed.control, parsed.alt, parsed.shift, parsed.command]
        .into_iter()
        .filter(|value| *value)
        .count()
        < 2
    {
        return Err("Use at least two modifier keys".into());
    }
    *SHORTCUT.lock().unwrap() = parsed;
    if HOOK_STARTED.load(Ordering::SeqCst) {
        return Ok(());
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || unsafe {
        let module = GetModuleHandleW(None).ok();
        let hook = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_hook),
            module.map(|module| HINSTANCE(module.0)),
            0,
        );
        let _ = sender.send(hook.is_ok());
        if hook.is_err() {
            return;
        }
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    });
    if receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap_or(false)
    {
        HOOK_STARTED.store(true, Ordering::SeqCst);
        Ok(())
    } else {
        Err("Could not register the global shortcut".into())
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) }.vkCode;
        let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
        let mut shortcut = SHORTCUT.lock().unwrap();
        if key == shortcut.key && down && shortcut.pressed {
            return LRESULT(1);
        }
        if key == shortcut.key && down && modifiers_down(&shortcut) {
            if !shortcut.pressed {
                shortcut.pressed = true;
                if let Some(app) = super::APP.get() {
                    let _ = crate::events::DictationShortcutDown {}.emit(app);
                }
            }
            return LRESULT(1);
        }
        if shortcut.pressed && up && (key == shortcut.key || released_modifier(key, &shortcut)) {
            shortcut.pressed = false;
            if let Some(app) = super::APP.get() {
                let _ = crate::events::DictationShortcutUp {}.emit(app);
            }
            if key == shortcut.key {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn released_modifier(key: u32, shortcut: &Shortcut) -> bool {
    (shortcut.control && matches!(key, 0x11 | 0xA2 | 0xA3))
        || (shortcut.alt && matches!(key, 0x12 | 0xA4 | 0xA5))
        || (shortcut.shift && matches!(key, 0x10 | 0xA0 | 0xA1))
        || (shortcut.command && matches!(key, 0x5B | 0x5C))
}

fn modifiers_down(shortcut: &Shortcut) -> bool {
    let down = |key: VIRTUAL_KEY| unsafe { GetAsyncKeyState(i32::from(key.0)) } < 0;
    down(VK_CONTROL) == shortcut.control
        && down(VK_MENU) == shortcut.alt
        && down(VK_SHIFT) == shortcut.shift
        && (down(VK_LWIN) || down(VK_RWIN)) == shortcut.command
}

fn focused_target() -> Result<(i32, Vec<i32>), String> {
    std::thread::spawn(|| unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|error| error.to_string())?;
        let result = (|| {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                    .map_err(|error| error.to_string())?;
            let element = automation
                .GetFocusedElement()
                .map_err(|error| error.to_string())?;
            let kind = element
                .CurrentControlType()
                .map_err(|error| error.to_string())?;
            if kind != UIA_EditControlTypeId && kind != UIA_DocumentControlTypeId {
                return Err("Focus an editable text field before dictating".into());
            }
            if element
                .CurrentIsPassword()
                .map_err(|error| error.to_string())?
                .as_bool()
                || !element
                    .CurrentIsEnabled()
                    .map_err(|error| error.to_string())?
                    .as_bool()
            {
                return Err("This field cannot accept dictation".into());
            }
            let process = element
                .CurrentProcessId()
                .map_err(|error| error.to_string())?;
            let array = element.GetRuntimeId().map_err(|error| error.to_string())?;
            if array.is_null() || SafeArrayGetDim(array) != 1 {
                return Err("Could not identify the focused text field".into());
            }
            let id = (|| {
                let start = SafeArrayGetLBound(array, 1).map_err(|error| error.to_string())?;
                let end = SafeArrayGetUBound(array, 1).map_err(|error| error.to_string())?;
                let mut values = Vec::new();
                for index in start..=end {
                    let mut value = 0i32;
                    SafeArrayGetElement(array, &index, (&mut value as *mut i32).cast())
                        .map_err(|error| error.to_string())?;
                    values.push(value);
                }
                Ok::<_, String>(values)
            })();
            let _ = SafeArrayDestroy(array);
            Ok((process, id?))
        })();
        CoUninitialize();
        result
    })
    .join()
    .map_err(|_| "Could not inspect the focused field".to_string())?
}

pub fn capture_target() -> Result<(), String> {
    *TARGET.lock().unwrap() = Some(focused_target()?);
    Ok(())
}

pub fn insert(text: &str) -> Result<(), String> {
    let original = TARGET.lock().unwrap().take().ok_or("No dictation target")?;
    if focused_target()? != original {
        return Err(
            "The original text field is no longer focused. Use Copy Last Dictation in the taskbar."
                .into(),
        );
    }
    let mut inputs = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: unit,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }
    }
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err("Could not insert dictation into this field".into())
    }
}

use std::{ptr, sync::Mutex, thread, time::Duration};

use tauri::Manager;
use windows::Win32::Foundation::{
    ERROR_SUCCESS, GetLastError, GlobalFree, HANDLE, HGLOBAL, HWND, SetLastError,
};
use windows::Win32::Graphics::Gdi::{
    CopyEnhMetaFileW, DeleteEnhMetaFile, DeleteMetaFile, DeleteObject, HENHMETAFILE, HGDIOBJ,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardOwner,
    GetClipboardSequenceNumber, METAFILEPICT, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{
    CF_BITMAP, CF_DSPBITMAP, CF_DSPENHMETAFILE, CF_DSPMETAFILEPICT, CF_ENHMETAFILE, CF_GDIOBJLAST,
    CF_METAFILEPICT, CF_OWNERDISPLAY, CF_PALETTE, CF_PRIVATEFIRST, CF_UNICODETEXT,
    CLIPBOARD_FORMAT, OleDuplicateData,
};
use windows::core::{Error, PCWSTR, w};

static PASTE_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn paste(text: &str) -> Result<(), String> {
    let text = text.to_owned();
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::Builder::new()
        .name("dictation-paste".into())
        .spawn(move || {
            let _lock = PASTE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
            let paste = (|| {
                super::wait_for_paste_keys_released()?;
                let app = super::super::APP.get().ok_or("Loofah is unavailable")?;
                let window = app
                    .get_webview_window("main")
                    .ok_or("Loofah's main window is unavailable")?;
                let owner = HWND(window.hwnd().map_err(|error| error.to_string())?.0);
                let (saved, sequence) = stage(owner, &text)?;
                Ok::<_, String>((owner, saved, sequence))
            })();
            match paste {
                Ok((owner, saved, sequence)) => {
                    let result = if unsafe { GetClipboardSequenceNumber() } == sequence {
                        super::send_paste()
                    } else {
                        Err("The clipboard changed before Loofah could paste".into())
                    };
                    let _ = sender.send(result);
                    thread::sleep(Duration::from_millis(350));
                    if let Err(error) = restore(owner, saved, sequence) {
                        tracing::warn!(%error, "dictation_clipboard_restore_failed");
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error));
                }
            }
        })
        .map_err(|error| error.to_string())?;
    receiver
        .recv()
        .map_err(|_| "Could not prepare the clipboard for dictation".to_string())?
}

fn stage(owner: HWND, text: &str) -> Result<(Vec<ClipboardEntry>, u32), String> {
    if text.contains('\0') {
        return Err("Dictation contains text Windows cannot paste".into());
    }
    let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    let text: Vec<u8> = text
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    let text = ClipboardEntry::from_bytes(u32::from(CF_UNICODETEXT.0), &text)?;
    let exclude = register_format(w!("ExcludeClipboardContentFromMonitorProcessing"))?;
    let exclude = ClipboardEntry::from_bytes(exclude, &[0])?;
    let ole_object = register_format(w!("DataObject"))?;

    let clipboard = OpenClipboardGuard::new(owner, 5)?;
    let saved = clipboard.snapshot(ole_object)?;
    unsafe { EmptyClipboard() }.map_err(|error| error.to_string())?;
    if let Err(error) = exclude.write().and_then(|()| text.write()) {
        if let Err(restore_error) = clipboard.replace(saved) {
            tracing::warn!(%restore_error, "dictation_clipboard_restore_failed");
        }
        return Err(error);
    }
    drop(clipboard);
    let sequence = unsafe { GetClipboardSequenceNumber() };
    if unsafe { GetClipboardOwner() }.ok() != Some(owner) {
        return Err("The clipboard changed before Loofah could paste".into());
    }
    Ok((saved, sequence))
}

fn restore(owner: HWND, saved: Vec<ClipboardEntry>, sequence: u32) -> Result<(), String> {
    if unsafe { GetClipboardSequenceNumber() } != sequence {
        return Ok(());
    }
    let clipboard = OpenClipboardGuard::new(owner, 50)?;
    if unsafe { GetClipboardSequenceNumber() } == sequence {
        clipboard.replace(saved)?;
    }
    Ok(())
}

fn register_format(name: PCWSTR) -> Result<u32, String> {
    let format = unsafe { RegisterClipboardFormatW(name) };
    if format == 0 {
        return Err(Error::from_thread().to_string());
    }
    Ok(format)
}

struct OpenClipboardGuard;

impl OpenClipboardGuard {
    fn new(owner: HWND, attempts: usize) -> Result<Self, String> {
        for _ in 1..attempts {
            if unsafe { OpenClipboard(Some(owner)) }.is_ok() {
                return Ok(Self);
            }
            thread::sleep(Duration::from_millis(10));
        }
        unsafe { OpenClipboard(Some(owner)) }.map_err(|error| error.to_string())?;
        Ok(Self)
    }

    fn snapshot(&self, ole_object: u32) -> Result<Vec<ClipboardEntry>, String> {
        let mut saved = Vec::new();
        let mut format = 0;
        loop {
            unsafe { SetLastError(ERROR_SUCCESS) };
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                return if unsafe { GetLastError() } == ERROR_SUCCESS {
                    Ok(saved)
                } else {
                    Err(Error::from_thread().to_string())
                };
            }
            if format == ole_object
                || format == u32::from(CF_OWNERDISPLAY.0)
                || (u32::from(CF_PRIVATEFIRST.0)..=u32::from(CF_GDIOBJLAST.0)).contains(&format)
            {
                return Err("Could not preserve application-owned clipboard data".into());
            }
            saved.push(ClipboardEntry::duplicate(format)?);
        }
    }

    fn replace(&self, entries: Vec<ClipboardEntry>) -> Result<(), String> {
        unsafe { EmptyClipboard() }.map_err(|error| error.to_string())?;
        let mut result = Ok(());
        for entry in entries {
            if let Err(error) = entry.write() {
                result = Err(error);
            }
        }
        result
    }
}

impl Drop for OpenClipboardGuard {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

struct ClipboardEntry {
    format: u32,
    handle: HANDLE,
}

impl ClipboardEntry {
    fn from_bytes(format: u32, bytes: &[u8]) -> Result<Self, String> {
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }
            .map_err(|error| error.to_string())?;
        let entry = Self {
            format,
            handle: HANDLE(memory.0),
        };
        let target = unsafe { GlobalLock(memory) }.cast::<u8>();
        if target.is_null() {
            return Err(Error::from_thread().to_string());
        }
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
            let _ = GlobalUnlock(memory);
        }
        Ok(entry)
    }

    fn duplicate(format: u32) -> Result<Self, String> {
        let original = unsafe { GetClipboardData(format) }.map_err(|error| error.to_string())?;
        let kind = match CLIPBOARD_FORMAT(format as u16) {
            CF_DSPBITMAP => CF_BITMAP,
            CF_DSPMETAFILEPICT => CF_METAFILEPICT,
            CF_DSPENHMETAFILE => CF_ENHMETAFILE,
            kind => kind,
        };
        let handle = match kind {
            CF_ENHMETAFILE => {
                HANDLE(unsafe { CopyEnhMetaFileW(HENHMETAFILE(original.0), PCWSTR::null()) }.0)
            }
            CF_BITMAP | CF_PALETTE | CF_METAFILEPICT => unsafe {
                OleDuplicateData(original, kind, GMEM_MOVEABLE)
            },
            _ if unsafe { GlobalSize(HGLOBAL(original.0)) } > 0 => unsafe {
                OleDuplicateData(original, kind, GMEM_MOVEABLE)
            },
            _ => return Err("Could not preserve a clipboard format".into()),
        };
        if handle.is_invalid() {
            return Err("Could not duplicate a clipboard format".into());
        }
        Ok(Self { format, handle })
    }

    fn write(mut self) -> Result<(), String> {
        unsafe { SetClipboardData(self.format, Some(self.handle)) }
            .map_err(|error| error.to_string())?;
        self.handle = HANDLE::default();
        Ok(())
    }
}

impl Drop for ClipboardEntry {
    fn drop(&mut self) {
        if self.handle.is_invalid() {
            return;
        }
        unsafe {
            match CLIPBOARD_FORMAT(self.format as u16) {
                CF_BITMAP | CF_DSPBITMAP | CF_PALETTE => {
                    let _ = DeleteObject(HGDIOBJ(self.handle.0));
                }
                CF_ENHMETAFILE | CF_DSPENHMETAFILE => {
                    let _ = DeleteEnhMetaFile(Some(HENHMETAFILE(self.handle.0)));
                }
                CF_METAFILEPICT | CF_DSPMETAFILEPICT => {
                    let memory = HGLOBAL(self.handle.0);
                    let picture = GlobalLock(memory).cast::<METAFILEPICT>();
                    if let Some(picture) = picture.as_ref() {
                        let _ = DeleteMetaFile(picture.hMF);
                        let _ = GlobalUnlock(memory);
                    }
                    let _ = GlobalFree(Some(memory));
                }
                _ => {
                    let _ = GlobalFree(Some(HGLOBAL(self.handle.0)));
                }
            }
        }
    }
}

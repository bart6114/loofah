use std::{collections::BTreeMap, path::Path};

use windows::Win32::{
    Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HANDLE},
    Media::Audio::{
        AudioSessionStateActive, DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2,
        IMMDeviceEnumerator, MMDeviceEnumerator, eCapture,
    },
    Storage::{
        EnhancedStorage::{PKEY_AppUserModel_ID, PKEY_Link_TargetParsingPath},
        Packaging::Appx::GetApplicationUserModelId,
    },
    System::{
        Com::{
            CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
            CoUninitialize,
        },
        Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
    },
    UI::Shell::{
        BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItem2,
        KF_FLAG_DEFAULT, SHGetKnownFolderItem, SIGDN_NORMALDISPLAY,
    },
};
use windows::core::{Interface, PWSTR, Result};

use super::InstalledApp;

struct Apartment;

impl Apartment {
    fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn owned_string(raw: PWSTR) -> Result<String> {
    let result = unsafe { raw.to_string() };
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    Ok(result?)
}

fn executable_id(path: &str) -> String {
    format!("exe:{}", path.trim_start_matches(r"\\?\").to_lowercase())
}

fn process_app(pid: u32) -> Result<InstalledApp> {
    let process = Process(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)? });
    let mut buffer = vec![0_u16; 32768];
    let mut size = buffer.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )?
    };
    let path = String::from_utf16_lossy(&buffer[..size as usize]);
    let name = Path::new(&path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut id = executable_id(&path);
    let mut length = 0;
    if unsafe { GetApplicationUserModelId(process.0, &mut length, None) }
        == ERROR_INSUFFICIENT_BUFFER
        && length > 0
        && length <= 32768
    {
        let mut buffer = vec![0_u16; length as usize];
        if unsafe {
            GetApplicationUserModelId(process.0, &mut length, Some(PWSTR(buffer.as_mut_ptr())))
        } == ERROR_SUCCESS
        {
            let value = String::from_utf16_lossy(&buffer[..length.saturating_sub(1) as usize]);
            id = format!("aumid:{}", value.to_lowercase());
        }
    }
    Ok(InstalledApp { id, name })
}

pub fn list_mic_using_apps() -> std::result::Result<Vec<InstalledApp>, crate::Error> {
    list_active_sessions().map_err(|error| crate::Error::AudioProcessQuery(error.to_string()))
}

fn list_active_sessions() -> Result<Vec<InstalledApp>> {
    let _apartment = Apartment::new()?;
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let devices = unsafe { enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)? };
    let mut apps = BTreeMap::new();
    for index in 0..unsafe { devices.GetCount()? } {
        let result = (|| -> Result<()> {
            let device = unsafe { devices.Item(index)? };
            let manager = unsafe { device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)? };
            let sessions = unsafe { manager.GetSessionEnumerator()? };
            for index in 0..unsafe { sessions.GetCount()? } {
                let result = (|| -> Result<Option<InstalledApp>> {
                    let session = unsafe { sessions.GetSession(index)? };
                    if unsafe { session.GetState()? } != AudioSessionStateActive {
                        return Ok(None);
                    }
                    let control: IAudioSessionControl2 = session.cast()?;
                    let pid = unsafe { control.GetProcessId()? };
                    if pid == 0 || pid == std::process::id() {
                        return Ok(None);
                    }
                    Ok(Some(process_app(pid)?))
                })();
                if let Ok(Some(app)) = result {
                    apps.insert(app.id.clone(), app);
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            tracing::debug!(%error, "capture_endpoint_inspection_failed");
        }
    }
    Ok(apps.into_values().collect())
}

pub fn list_installed_apps() -> Vec<InstalledApp> {
    match installed_apps() {
        Ok(apps) => apps,
        Err(error) => {
            tracing::warn!(%error, "windows_app_list_failed");
            Vec::new()
        }
    }
}

fn installed_apps() -> Result<Vec<InstalledApp>> {
    let _apartment = Apartment::new()?;
    let folder: IShellItem =
        unsafe { SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)? };
    let items: IEnumShellItems = unsafe { folder.BindToHandler(None, &BHID_EnumItems)? };
    let mut apps = BTreeMap::new();
    loop {
        let mut item = [None];
        let mut count = 0;
        unsafe { items.Next(&mut item, Some(&mut count))? };
        if count == 0 {
            break;
        }
        let Some(item) = item[0].take() else {
            continue;
        };
        let result = (|| -> Result<_> {
            let name = owned_string(unsafe { item.GetDisplayName(SIGDN_NORMALDISPLAY)? })?;
            let properties: IShellItem2 = item.cast()?;
            let target = unsafe { properties.GetString(&PKEY_Link_TargetParsingPath) }
                .and_then(owned_string)
                .ok()
                .filter(|path| path.to_lowercase().ends_with(".exe"));
            let id = if let Some(path) = target {
                executable_id(&path)
            } else {
                let id = owned_string(unsafe { properties.GetString(&PKEY_AppUserModel_ID)? })?;
                format!("aumid:{}", id.to_lowercase())
            };
            Ok(InstalledApp { id, name })
        })();
        if let Ok(app) = result {
            apps.insert(app.id.clone(), app);
        }
    }
    let mut apps: Vec<_> = apps.into_values().collect();
    apps.sort_by_key(|app| app.name.to_lowercase());
    Ok(apps)
}

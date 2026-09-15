use std::{ffi::c_void, sync::Mutex};

use ::windows::{
    Win32::{
        Foundation::{CLASS_E_NOAGGREGATION, E_POINTER},
        System::Com::{
            CLSCTX_LOCAL_SERVER, CoRegisterClassObject, CoRevokeClassObject, IClassFactory,
            IClassFactory_Impl, REGCLS_MULTIPLEUSE,
        },
        UI::Notifications::{
            INotificationActivationCallback, INotificationActivationCallback_Impl,
            NOTIFICATION_USER_INPUT_DATA,
        },
    },
    core::{BOOL, GUID, IUnknown, Interface, PCWSTR, Ref, Result, implement},
};

static STOP: Mutex<Option<std::sync::mpsc::Sender<()>>> = Mutex::new(None);

#[implement(INotificationActivationCallback)]
struct Activator;

#[allow(non_snake_case)]
impl INotificationActivationCallback_Impl for Activator_Impl {
    fn Activate(
        &self,
        app_id: &PCWSTR,
        arguments: &PCWSTR,
        data: *const NOTIFICATION_USER_INPUT_DATA,
        count: u32,
    ) -> Result<()> {
        if app_id.is_null() || arguments.is_null() || (count > 0 && data.is_null()) || count > 128 {
            return Err(E_POINTER.into());
        }
        let app_id = unsafe { app_id.to_string()? };
        if super::APP_ID.get() != Some(&app_id) {
            return Ok(());
        }
        let arguments = unsafe { arguments.to_string()? };
        let Some((token, action)) = arguments.split_once(':') else {
            return Ok(());
        };
        let mut choice = None;
        if count > 0 {
            for input in unsafe { std::slice::from_raw_parts(data, count as usize) } {
                if !input.Key.is_null()
                    && !input.Value.is_null()
                    && unsafe { input.Key.to_string()? } == "choice"
                {
                    choice = unsafe { input.Value.to_string()? }.parse::<usize>().ok();
                }
            }
        }
        let (action, option) = match action {
            "confirm" => (super::Action::Confirm, -1),
            "accept" => (super::Action::Accept, -1),
            "footer" => (super::Action::Footer, -1),
            "dismiss" => (super::Action::Dismiss, -1),
            "option" => {
                let count = super::TOASTS
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(token)
                    .map_or(0, |record| record.2);
                let Some(choice) = choice.filter(|i| *i < count) else {
                    return Ok(());
                };
                (super::Action::Option, choice as i32)
            }
            _ => return Ok(()),
        };
        super::dispatch(token, action, option);
        Ok(())
    }
}

#[implement(IClassFactory)]
struct Factory;

#[allow(non_snake_case)]
impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        if iid.is_null() || object.is_null() {
            return Err(E_POINTER.into());
        }
        let instance: INotificationActivationCallback = Activator.into();
        unsafe {
            *object = std::ptr::null_mut();
            instance.query(iid, object).ok()
        }
    }
    fn LockServer(&self, _: BOOL) -> Result<()> {
        Ok(())
    }
}

pub(super) fn register(id: &str) -> std::result::Result<(), String> {
    let (clsid, guid) = activator_id(id);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let register = || -> std::result::Result<(), windows_core::Error> {
        let key = windows_registry::CURRENT_USER
            .create(format!(r"Software\Classes\AppUserModelId\{id}"))?;
        key.set_string(
            "DisplayName",
            if id.contains("staging") {
                "Loofah Staging"
            } else {
                "Loofah"
            },
        )?;
        key.set_string("IconUri", exe.to_string_lossy())?;
        key.set_string("CustomActivator", &guid)?;
        windows_registry::CURRENT_USER
            .create(format!(r"Software\Classes\CLSID\{guid}\LocalServer32"))?
            .set_string("", format!("\"{}\" -ToastActivated", exe.display()))?;
        Ok(())
    };
    register().map_err(|e| e.to_string())?;
    let (stop, receiver) = std::sync::mpsc::channel();
    let (ready, result) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let registration = (|| -> Result<_> {
            let apartment = super::Apartment::new()?;
            let factory: IClassFactory = Factory.into();
            let cookie = unsafe {
                CoRegisterClassObject(&clsid, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)?
            };
            Ok((apartment, cookie))
        })();
        match registration {
            Ok((_apartment, cookie)) => {
                let _ = ready.send(Ok(()));
                let _ = receiver.recv();
                let _ = unsafe { CoRevokeClassObject(cookie) };
            }
            Err(error) => {
                let _ = ready.send(Err(error.to_string()));
            }
        }
    });
    result.recv().map_err(|e| e.to_string())??;
    *STOP.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop);
    Ok(())
}

pub(crate) fn shutdown() {
    STOP.lock().unwrap_or_else(|e| e.into_inner()).take();
}

fn activator_id(id: &str) -> (GUID, String) {
    let hash = id.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    });
    let clsid = GUID::from_u128(0x1b769a0c_6bc1_4e0f_8000_000000000000_u128 | hash as u128);
    let guid = format!("{{{clsid:?}}}");
    (clsid, guid)
}

pub fn uninstall(id: &str) -> std::result::Result<(), String> {
    let (_, guid) = activator_id(id);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let class_path = format!(r"Software\Classes\CLSID\{guid}");
    let app_path = format!(r"Software\Classes\AppUserModelId\{id}");
    let expected = format!("\"{}\" -ToastActivated", exe.display());
    let registry = windows_registry::CURRENT_USER;
    if let Ok(key) = registry.open(format!(r"{class_path}\LocalServer32")) {
        if key.get_string("").map_err(|e| e.to_string())? == expected {
            registry
                .remove_tree(&class_path)
                .map_err(|e| e.to_string())?;
        }
    }
    if let Ok(key) = registry.open(&app_path) {
        if key.get_string("IconUri").map_err(|e| e.to_string())? == exe.to_string_lossy()
            && key
                .get_string("CustomActivator")
                .map_err(|e| e.to_string())?
                == guid
        {
            registry.remove_tree(&app_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

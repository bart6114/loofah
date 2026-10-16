use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use windows::Win32::{
    Foundation::{ERROR_SUCCESS, HANDLE},
    System::Power::{
        DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY, PowerRegisterSuspendResumeNotification,
        PowerUnregisterSuspendResumeNotification,
    },
    UI::WindowsAndMessaging::{
        DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
    },
};

use crate::{DetectCallback, DetectEvent, Observer};

struct Callback {
    running: AtomicBool,
    callback: DetectCallback,
}

static CALLBACKS: LazyLock<Mutex<HashMap<usize, Arc<Callback>>>> = LazyLock::new(Mutex::default);
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

// The context is an opaque registry key, never a dereferenced pointer. An in-flight
// Windows callback cannot access freed memory when a detector is stopped.
unsafe extern "system" fn notify(
    context: *const std::ffi::c_void,
    event: u32,
    _: *const std::ffi::c_void,
) -> u32 {
    let callback = CALLBACKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(context as usize))
        .cloned();
    if let Some(callback) = callback.filter(|c| c.running.load(Ordering::Acquire)) {
        let sleeping = match event {
            PBT_APMSUSPEND => Some(true),
            PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => Some(false),
            _ => None,
        };
        if let Some(value) = sleeping {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (callback.callback)(DetectEvent::SleepStateChanged { value });
            }));
        }
    }
    0
}

#[derive(Default)]
pub struct SleepDetector {
    registration: Option<(usize, usize)>,
}

impl SleepDetector {
    pub fn subscribe(callback: DetectCallback) -> Self {
        let mut detector = Self::default();
        detector.start(callback);
        detector
    }
}

impl Observer for SleepDetector {
    fn start(&mut self, callback: DetectCallback) {
        self.stop();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        CALLBACKS.lock().unwrap_or_else(|e| e.into_inner()).insert(
            id,
            Arc::new(Callback {
                running: AtomicBool::new(true),
                callback,
            }),
        );
        let parameters = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(notify),
            Context: id as *mut _,
        };
        let mut handle = std::ptr::null_mut();
        let result = unsafe {
            PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                HANDLE(
                    (&parameters as *const DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS)
                        .cast_mut()
                        .cast(),
                ),
                &mut handle,
            )
        };
        if result == ERROR_SUCCESS {
            self.registration = Some((id, handle as usize));
        } else {
            CALLBACKS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            tracing::error!(code = result.0, "windows_sleep_monitor_failed");
        }
    }

    fn stop(&mut self) {
        if let Some((id, handle)) = self.registration.take() {
            if let Some(callback) = CALLBACKS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id)
            {
                callback.running.store(false, Ordering::Release);
            }
            let result =
                unsafe { PowerUnregisterSuspendResumeNotification(HPOWERNOTIFY(handle as isize)) };
            if result != ERROR_SUCCESS {
                tracing::warn!(code = result.0, "windows_sleep_monitor_unregister_failed");
            }
        }
    }
}

impl Drop for SleepDetector {
    fn drop(&mut self) {
        self.stop();
    }
}

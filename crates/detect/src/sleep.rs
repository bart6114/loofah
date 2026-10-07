use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use block2::RcBlock;
use objc2::{msg_send, rc::Retained};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObject, NSString};

use crate::{DetectCallback, DetectEvent};

struct SleepObserver {
    center: Retained<NSNotificationCenter>,
    will_sleep_observer: Retained<NSObject>,
    did_wake_observer: Retained<NSObject>,
}

impl Drop for SleepObserver {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![&*self.center, removeObserver: &*self.will_sleep_observer];
            let _: () = msg_send![&*self.center, removeObserver: &*self.did_wake_observer];
        }
    }
}

pub struct SleepDetector {
    running: Arc<AtomicBool>,
    thread_handle: Option<JoinHandle<()>>,
}

impl Default for SleepDetector {
    fn default() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
        }
    }
}

impl crate::Observer for SleepDetector {
    fn start(&mut self, f: DetectCallback) {
        if self.running.load(Ordering::SeqCst) {
            return;
        }

        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();

        self.thread_handle = Some(std::thread::spawn(move || {
            let will_sleep_callback = Arc::downgrade(&f);
            let will_sleep_running = running.clone();
            let will_sleep_block = RcBlock::new(move |_notification: *const NSNotification| {
                if will_sleep_running.load(Ordering::SeqCst) {
                    if let Some(callback) = will_sleep_callback.upgrade() {
                        callback(DetectEvent::SleepStateChanged { value: true });
                    }
                }
            });

            let did_wake_callback = Arc::downgrade(&f);
            let did_wake_running = running.clone();
            let did_wake_block = RcBlock::new(move |_notification: *const NSNotification| {
                if did_wake_running.load(Ordering::SeqCst) {
                    if let Some(callback) = did_wake_callback.upgrade() {
                        callback(DetectEvent::SleepStateChanged { value: false });
                    }
                }
            });

            let _observer = unsafe {
                let workspace = NSWorkspace::sharedWorkspace();
                let center = workspace.notificationCenter();

                let will_sleep_name = NSString::from_str("NSWorkspaceWillSleepNotification");
                let did_wake_name = NSString::from_str("NSWorkspaceDidWakeNotification");

                let will_sleep_observer: Retained<NSObject> = msg_send![
                    &*center,
                    addObserverForName: &*will_sleep_name,
                    object: std::ptr::null::<NSObject>(),
                    queue: std::ptr::null::<NSObject>(),
                    usingBlock: &*will_sleep_block
                ];

                let did_wake_observer: Retained<NSObject> = msg_send![
                    &*center,
                    addObserverForName: &*did_wake_name,
                    object: std::ptr::null::<NSObject>(),
                    queue: std::ptr::null::<NSObject>(),
                    usingBlock: &*did_wake_block
                ];

                SleepObserver {
                    center,
                    will_sleep_observer,
                    did_wake_observer,
                }
            };

            while running.load(Ordering::SeqCst) {
                std::thread::park_timeout(Duration::from_millis(500));
            }
        }));
    }

    fn stop(&mut self) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }

        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.thread_handle.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

impl Drop for SleepDetector {
    fn drop(&mut self) {
        crate::Observer::stop(self);
    }
}

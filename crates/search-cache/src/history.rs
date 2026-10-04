//! The callback only copies events; cache progress is persisted together with
//! queued repairs by the caller, never from the FSEvents thread.
use fsevent_sys::core_foundation::*;
use fsevent_sys::*;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

pub struct Event {
    pub path: PathBuf,
    pub cursor: u64,
    pub rescan: bool,
}

struct Callback {
    tx: mpsc::SyncSender<Event>,
    overflow: Arc<AtomicBool>,
}

pub struct History {
    pub events: mpsc::Receiver<Event>,
    pub overflow: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source: bool) -> i32;
}

extern "C" fn callback(
    _: FSEventStreamRef,
    context: *mut std::ffi::c_void,
    count: usize,
    paths: *mut std::ffi::c_void,
    flags: *const u32,
    ids: *const u64,
) {
    // FSEvents owns the arrays for the callback duration; context is a sender
    // kept alive until the stream has been stopped and invalidated on this thread.
    unsafe {
        let context = &*(context as *const Callback);
        let paths = paths as *const *const std::ffi::c_char;
        for i in 0..count {
            let path = std::ffi::CStr::from_ptr(*paths.add(i))
                .to_string_lossy()
                .into_owned();
            let flag = *flags.add(i);
            if context
                .tx
                .try_send(Event {
                    path: path.into(),
                    cursor: *ids.add(i),
                    rescan: flag
                        & (kFSEventStreamEventFlagMustScanSubDirs
                            | kFSEventStreamEventFlagEventIdsWrapped
                            | kFSEventStreamEventFlagRootChanged
                            | kFSEventStreamEventFlagMount
                            | kFSEventStreamEventFlagUnmount)
                        != 0,
                })
                .is_err()
            {
                context.overflow.store(true, Ordering::Relaxed);
            }
        }
    }
}

impl History {
    pub fn start(path: &Path, since: u64) -> anyhow::Result<Self> {
        let path =
            std::ffi::CString::new(std::fs::canonicalize(path)?.as_os_str().as_encoded_bytes())?;
        let (tx, events) = mpsc::sync_channel(4096);
        let overflow = Arc::new(AtomicBool::new(false));
        let callback_state = Callback {
            tx,
            overflow: overflow.clone(),
        };
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || unsafe {
            let string = CFStringCreateWithCString(
                kCFAllocatorDefault,
                path.as_ptr(),
                kCFStringEncodingUTF8,
            );
            let paths = CFArrayCreateMutable(kCFAllocatorDefault, 1, &kCFTypeArrayCallBacks);
            CFArrayAppendValue(paths, string);
            let context = FSEventStreamContext {
                version: 0,
                info: &callback_state as *const _ as *mut _,
                retain: None,
                release: None,
                copy_description: None,
            };
            let current = FSEventsGetCurrentEventId();
            let cursor = if since > 0 && since <= current {
                since
            } else {
                current
            };
            let stream = FSEventStreamCreate(
                kCFAllocatorDefault,
                callback,
                &context,
                paths,
                cursor,
                0.1,
                kFSEventStreamCreateFlagFileEvents
                    | kFSEventStreamCreateFlagWatchRoot
                    | kFSEventStreamCreateFlagNoDefer,
            );
            CFRelease(paths);
            CFRelease(string);
            if stream.is_null() {
                let _ = ready_tx.send(false);
                return;
            }
            let run_loop = CFRunLoopGetCurrent();
            FSEventStreamScheduleWithRunLoop(stream, run_loop, kCFRunLoopDefaultMode);
            if FSEventStreamStart(stream) == 0 {
                FSEventStreamInvalidate(stream);
                FSEventStreamRelease(stream);
                let _ = ready_tx.send(false);
                return;
            }
            if since == 0 || since > current {
                let _ = callback_state.tx.try_send(Event {
                    path: PathBuf::new(),
                    cursor: current,
                    rescan: true,
                });
            }
            let _ = ready_tx.send(true);
            while !stopped.load(Ordering::Relaxed) {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, false);
            }
            FSEventStreamStop(stream);
            FSEventStreamInvalidate(stream);
            FSEventStreamRelease(stream);
        });
        anyhow::ensure!(ready_rx.recv().unwrap_or(false), "FSEvents unavailable");
        Ok(Self {
            events,
            overflow,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for History {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(feature = "app")]
mod app;
mod error;
#[cfg(all(target_os = "macos", feature = "language"))]
mod language;
#[cfg(all(target_os = "windows", feature = "language"))]
#[path = "language_windows.rs"]
mod language;
#[cfg(feature = "list")]
mod list;
#[cfg(feature = "mic")]
mod mic;
#[cfg(all(target_os = "macos", feature = "sleep"))]
mod sleep;
#[cfg(all(target_os = "windows", feature = "sleep"))]
#[path = "sleep_windows.rs"]
mod sleep;

mod utils;

pub use error::Error;

pub use utils::BackgroundTask;

#[cfg(feature = "app")]
pub use app::*;
#[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "language"))]
pub use language::*;
#[cfg(feature = "list")]
pub use list::*;
#[cfg(feature = "mic")]
pub use mic::*;

#[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "sleep"))]
pub use sleep::*;

#[cfg(feature = "mic")]
#[derive(Debug, Clone)]
pub enum DetectEvent {
    MicStarted(Vec<InstalledApp>),
    MicStopped(Vec<InstalledApp>),
    #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "sleep"))]
    SleepStateChanged {
        value: bool,
    },
}

#[cfg(feature = "mic")]
pub type DetectCallback = std::sync::Arc<dyn Fn(DetectEvent) + Send + Sync + 'static>;

#[cfg(feature = "mic")]
pub fn new_callback<F>(f: F) -> DetectCallback
where
    F: Fn(DetectEvent) + Send + Sync + 'static,
{
    std::sync::Arc::new(f)
}

#[cfg(feature = "mic")]
pub(crate) trait Observer: Send + Sync {
    fn start(&mut self, f: DetectCallback);
    fn stop(&mut self);
}

#[cfg(feature = "mic")]
#[derive(Default)]
pub struct Detector {
    mic_detector: MicDetector,
    #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "sleep"))]
    sleep_detector: SleepDetector,
}

#[cfg(feature = "mic")]
impl Detector {
    pub fn start(&mut self, f: DetectCallback) {
        self.mic_detector.start(f.clone());

        #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "sleep"))]
        self.sleep_detector.start(f);
    }

    pub fn stop(&mut self) {
        self.mic_detector.stop();

        #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "sleep"))]
        self.sleep_detector.stop();
    }
}

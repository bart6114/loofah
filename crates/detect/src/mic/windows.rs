use std::{sync::mpsc, thread::JoinHandle, time::Duration};

use super::tracker::Applications;
use crate::{DetectCallback, DetectEvent, Observer};

#[derive(Default)]
pub struct Detector {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Observer for Detector {
    fn start(&mut self, callback: DetectCallback) {
        self.stop();
        let (tx, rx) = mpsc::channel();
        self.stop = Some(tx);
        self.thread = Some(std::thread::spawn(move || {
            let mut apps = Applications::default();
            loop {
                match crate::list_mic_using_apps() {
                    Ok(current) => {
                        let (started, stopped) = apps.update(current);
                        if !stopped.is_empty() {
                            callback(DetectEvent::MicStopped(stopped));
                        }
                        if !started.is_empty() {
                            callback(DetectEvent::MicStarted(started));
                        }
                    }
                    Err(error) => tracing::debug!(%error, "windows_mic_detection_failed"),
                }
                match rx.recv_timeout(Duration::from_millis(750)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        }));
    }

    fn stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Detector {
    fn drop(&mut self) {
        self.stop();
    }
}

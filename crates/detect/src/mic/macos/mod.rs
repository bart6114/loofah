mod app;
mod device;
mod state;

use cidre::core_audio as ca;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use state::SharedContext;

const DEVICE_IS_RUNNING_SOMEWHERE: ca::PropAddr = ca::PropAddr {
    selector: ca::PropSelector::DEVICE_IS_RUNNING_SOMEWHERE,
    scope: ca::PropScope::GLOBAL,
    element: ca::PropElement::MAIN,
};
const POLL_INTERVAL: Duration = Duration::from_secs(1);

struct Control {
    running: Arc<AtomicBool>,
    admission: Mutex<()>,
    wake: mpsc::SyncSender<()>,
}

impl Control {
    fn notify(&self) {
        if self.running.load(Ordering::SeqCst) {
            let _ = self.wake.try_send(());
        }
    }
    fn stop(&self) {
        let _admission = self.admission.lock().unwrap();
        self.running.store(false, Ordering::SeqCst);
        let _ = self.wake.try_send(());
    }
}

trait AudioBackend {
    type Registration;
    fn default_input(&mut self) -> Option<ca::Obj>;
    fn register(
        &mut self,
        object: ca::Obj,
        address: ca::PropAddr,
        control: Weak<Control>,
    ) -> Result<Self::Registration, ()>;
    fn is_running(&self, object: ca::Obj) -> Option<bool>;
    fn drain(&self);
}

#[derive(Default)]
pub struct Detector {
    worker: Option<(Arc<Control>, JoinHandle<()>)>,
}

impl crate::Observer for Detector {
    fn start(&mut self, f: crate::DetectCallback) {
        self.start_with(f, device::CoreAudio::new);
    }
    fn stop(&mut self) {
        if let Some((control, worker)) = self.worker.take() {
            control.stop();
            if let Err(error) = worker.join() {
                tracing::error!(?error, "mic_detector_worker_failed");
            }
        }
    }
}

impl Detector {
    fn start_with<B: AudioBackend + 'static>(
        &mut self,
        f: crate::DetectCallback,
        backend: impl FnOnce() -> B + Send + 'static,
    ) {
        if self.worker.is_some() {
            return;
        }
        let (wake, rx) = mpsc::sync_channel(1);
        let control = Arc::new(Control {
            running: Arc::new(AtomicBool::new(true)),
            admission: Mutex::new(()),
            wake,
        });
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || run_worker(worker_control, rx, f, backend()));
        self.worker = Some((control, worker));
    }
}

impl Drop for Detector {
    fn drop(&mut self) {
        crate::Observer::stop(self);
    }
}

fn run_worker<B: AudioBackend>(
    control: Arc<Control>,
    rx: mpsc::Receiver<()>,
    f: crate::DetectCallback,
    mut backend: B,
) {
    let mut ctx = SharedContext::new(f);
    ctx.running = control.running.clone();
    let mut registrations = None;
    let mut current_device = None;
    let mut seeded = false;
    while control.running.load(Ordering::SeqCst) {
        let device = backend.default_input();
        if registrations.is_none() || current_device != device {
            let admission = control.admission.lock().unwrap();
            if !control.running.load(Ordering::SeqCst) {
                break;
            }
            registrations.take();
            let attempt = (|| {
                let system = backend.register(
                    *ca::System::OBJ,
                    ca::PropSelector::HW_DEFAULT_INPUT_DEVICE.global_addr(),
                    Arc::downgrade(&control),
                )?;
                let input = device
                    .map(|object| {
                        backend.register(
                            object,
                            DEVICE_IS_RUNNING_SOMEWHERE,
                            Arc::downgrade(&control),
                        )
                    })
                    .transpose()?;
                Ok::<_, ()>((system, input))
            })();
            if let Ok(listeners) = attempt {
                registrations = Some(listeners);
                current_device = device;
            }
            drop(admission);
        }
        if !control.running.load(Ordering::SeqCst) {
            break;
        }
        if let Some(running) = device.and_then(|object| backend.is_running(object)) {
            if !seeded {
                ctx.seed_running_state(running);
                seeded = true;
            } else {
                ctx.handle_mic_change(running);
            }
        }
        app::poll_apps(&ctx);
        if rx.recv_timeout(POLL_INTERVAL) == Err(mpsc::RecvTimeoutError::Disconnected) {
            break;
        }
    }
    control.running.store(false, Ordering::SeqCst);
    drop(registrations);
    backend.drain();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Observer;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct NativeState {
        active: AtomicUsize,
        attempts: AtomicUsize,
        fail_at: AtomicUsize,
        device: AtomicUsize,
        drained: AtomicBool,
        wakeups: Mutex<Vec<Weak<Control>>>,
    }
    struct FakeBackend(Arc<NativeState>);
    struct FakeRegistration(Arc<NativeState>);
    impl Drop for FakeRegistration {
        fn drop(&mut self) {
            self.0.active.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl AudioBackend for FakeBackend {
        type Registration = FakeRegistration;
        fn default_input(&mut self) -> Option<ca::Obj> {
            Some(ca::Obj(self.0.device.load(Ordering::SeqCst) as u32 + 10))
        }
        fn register(
            &mut self,
            _: ca::Obj,
            _: ca::PropAddr,
            control: Weak<Control>,
        ) -> Result<FakeRegistration, ()> {
            let attempt = self.0.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            self.0.wakeups.lock().unwrap().push(control);
            if attempt == self.0.fail_at.load(Ordering::SeqCst) {
                return Err(());
            }
            self.0.active.fetch_add(1, Ordering::SeqCst);
            Ok(FakeRegistration(self.0.clone()))
        }
        fn is_running(&self, _: ca::Obj) -> Option<bool> {
            Some(false)
        }
        fn drain(&self) {
            self.0.drained.store(true, Ordering::SeqCst);
        }
    }
    fn wait_for(check: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !check() {
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not settle"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn registration_failure_rolls_back_and_allows_retry() {
        for failure in [1, 2] {
            let native = Arc::new(NativeState::default());
            native.fail_at.store(failure, Ordering::SeqCst);
            let backend = native.clone();
            let mut detector = Detector::default();
            detector.start_with(crate::new_callback(|_| {}), move || FakeBackend(backend));
            wait_for(|| {
                native.attempts.load(Ordering::SeqCst) >= failure
                    && native.active.load(Ordering::SeqCst) == 0
            });
            detector.worker.as_ref().unwrap().0.notify();
            wait_for(|| native.active.load(Ordering::SeqCst) == 2);
            detector.stop();
            assert_eq!(native.active.load(Ordering::SeqCst), 0);
            assert!(native.drained.load(Ordering::SeqCst));
        }
    }
    #[test]
    fn repeated_cycles_replacement_and_queued_notifications_release_every_owner() {
        let callback = crate::new_callback(|_| panic!("stopped detector emitted an event"));
        for _ in 0..10 {
            let native = Arc::new(NativeState::default());
            let backend = native.clone();
            let mut detector = Detector::default();
            detector.start_with(callback.clone(), move || FakeBackend(backend));
            wait_for(|| native.active.load(Ordering::SeqCst) == 2);
            let control = detector.worker.as_ref().unwrap().0.clone();
            native.device.store(1, Ordering::SeqCst);
            control.notify();
            wait_for(|| native.attempts.load(Ordering::SeqCst) == 4);
            control.stop();
            native.device.store(2, Ordering::SeqCst);
            for wakeup in native.wakeups.lock().unwrap().iter() {
                if let Some(control) = wakeup.upgrade() {
                    control.notify();
                }
            }
            detector.stop();
            assert_eq!(native.attempts.load(Ordering::SeqCst), 4);
            assert_eq!(native.active.load(Ordering::SeqCst), 0);
            assert_eq!(Arc::strong_count(&callback), 1);
            assert!(native.drained.load(Ordering::SeqCst));
            drop(control);
            assert!(
                native
                    .wakeups
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|weak| weak.upgrade().is_none())
            );
        }
    }
    #[test]
    fn immediate_stop_is_restartable() {
        let callback = crate::new_callback(|_| {});
        let mut detector = Detector::default();
        for _ in 0..10 {
            let native = Arc::new(NativeState::default());
            let backend = native.clone();
            detector.start_with(callback.clone(), move || FakeBackend(backend));
            detector.stop();
            assert_eq!(native.active.load(Ordering::SeqCst), 0);
            assert_eq!(Arc::strong_count(&callback), 1);
        }
    }
}

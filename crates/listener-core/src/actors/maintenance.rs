use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

#[derive(Clone, Default)]
pub struct CaptureMaintenanceGate(Arc<Inner>);
#[derive(Default)]
struct Inner {
    lock: Arc<RwLock<()>>,
    unknown: AtomicBool,
    background: Mutex<Weak<AtomicBool>>,
}

pub struct CaptureMaintenanceGuard {
    _held: OwnedRwLockWriteGuard<()>,
    owner: Arc<Inner>,
}

impl CaptureMaintenanceGate {
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn start(&self) -> Option<OwnedRwLockReadGuard<()>> {
        let background = self.0.background.lock().ok()?;
        if let Some(cancel) = background.upgrade() {
            cancel.store(true, Ordering::Release);
        }
        let held = self.0.lock.clone().try_read_owned().ok()?;
        (!self.0.unknown.load(Ordering::Acquire)).then_some(held)
    }

    pub(crate) fn maintain(&self) -> Option<CaptureMaintenanceGuard> {
        let background = self.0.background.lock().ok()?;
        if background.strong_count() != 0 {
            return None;
        }
        let held = self.0.lock.clone().try_write_owned().ok()?;
        (!self.0.unknown.load(Ordering::Acquire)).then_some(CaptureMaintenanceGuard {
            _held: held,
            owner: self.0.clone(),
        })
    }

    pub(crate) fn mark_unknown(&self) {
        self.0.unknown.store(true, Ordering::Release);
    }
}

/// Occupied until the actual background native work releases its owner. Starting
/// capture cancels background work without waiting for model inference to unwind.
pub struct CaptureBackgroundGuard {
    cancelled: Arc<AtomicBool>,
}
impl CaptureBackgroundGuard {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}
impl CaptureMaintenanceGuard {
    pub fn into_background(self) -> Option<CaptureBackgroundGuard> {
        let mut background = self.owner.background.lock().ok()?;
        if self.owner.unknown.load(Ordering::Acquire) || background.strong_count() != 0 {
            return None;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        *background = Arc::downgrade(&cancelled);
        Some(CaptureBackgroundGuard { cancelled })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_preempts_background_and_retiring_work_keeps_capacity() {
        let gate = CaptureMaintenanceGate::default();
        let background = gate.maintain().unwrap().into_background().unwrap();
        assert!(!background.is_cancelled());
        assert!(gate.maintain().is_none());
        let live = gate.start().unwrap();
        assert!(background.is_cancelled());
        drop(live);
        assert!(gate.maintain().is_none());
        drop(background);
        assert!(gate.maintain().is_some());
    }
    #[test]
    fn active_capture_and_unknown_state_reject_background() {
        let gate = CaptureMaintenanceGate::default();
        let live = gate.start().unwrap();
        assert!(gate.maintain().is_none());
        drop(live);
        let proof = gate.maintain().unwrap();
        gate.mark_unknown();
        assert!(proof.into_background().is_none());
        assert!(gate.start().is_none());
    }
    #[tokio::test]
    async fn cancellation_and_observer_drop_do_not_release_actual_native_reservation() {
        let gate = CaptureMaintenanceGate::default();
        let background = Arc::new(gate.maintain().unwrap().into_background().unwrap());
        let owned = background.clone();
        let (release, wait) = std::sync::mpsc::channel();
        let (entered, started) = tokio::sync::oneshot::channel();
        let worker = tokio::task::spawn_blocking(move || {
            let _owner = owned;
            entered.send(()).unwrap();
            wait.recv().unwrap();
        });
        started.await.unwrap();
        let live = gate.start().unwrap();
        assert!(background.is_cancelled());
        drop(background);
        drop(live);
        assert!(gate.maintain().is_none());
        for _ in 0..16 {
            assert!(gate.start().is_some());
            assert!(gate.maintain().is_none());
        }
        release.send(()).unwrap();
        worker.await.unwrap();
        assert!(gate.maintain().is_some());
    }
}

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

#[derive(Clone, Default)]
pub struct CaptureMaintenanceGate(Arc<Inner>);
#[derive(Default)]
struct Inner {
    lock: Arc<RwLock<()>>,
    unknown: AtomicBool,
}

pub struct CaptureMaintenanceGuard {
    _held: OwnedRwLockWriteGuard<()>,
}

impl CaptureMaintenanceGate {
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn start(&self) -> Option<OwnedRwLockReadGuard<()>> {
        let held = self.0.lock.clone().try_read_owned().ok()?;
        (!self.0.unknown.load(Ordering::Acquire)).then_some(held)
    }

    pub(crate) fn maintain(&self) -> Option<CaptureMaintenanceGuard> {
        let held = self.0.lock.clone().try_write_owned().ok()?;
        (!self.0.unknown.load(Ordering::Acquire)).then_some(CaptureMaintenanceGuard { _held: held })
    }

    pub(crate) fn mark_unknown(&self) {
        self.0.unknown.store(true, Ordering::Release);
    }
}

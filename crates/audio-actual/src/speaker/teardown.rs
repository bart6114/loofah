use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Stop,
    Callback,
    Aggregate,
    Tap,
    Register,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Failure {
    pub stage: Stage,
    pub status: Option<i32>,
}

pub(super) trait Operations<P: Copy> {
    fn stop(&self, device: u32, callback: P) -> i32;
    fn destroy_callback(&self, device: u32, callback: P) -> i32;
    fn destroy_aggregate(&self, device: u32) -> i32;
    fn destroy_tap(&self, tap: u32) -> i32;
}

pub(super) struct Owned<P: Copy + 'static, C: 'static, O: Operations<P>> {
    pub tap: Option<u32>,
    pub device: Option<u32>,
    pub callback: Option<P>,
    pub started: bool,
    pub context: Option<Box<C>>,
    pub owner: Option<Arc<dyn Send + Sync>>,
    pub cleanup_failed: Arc<AtomicBool>,
    failure: Option<Failure>,
    operations: O,
}
impl<P: Copy + 'static, C: 'static, O: Operations<P>> Owned<P, C, O> {
    pub fn new(operations: O, owner: Option<Arc<dyn Send + Sync>>) -> Self {
        Self {
            tap: None,
            device: None,
            callback: None,
            started: false,
            context: None,
            owner,
            cleanup_failed: Arc::new(AtomicBool::new(false)),
            failure: None,
            operations,
        }
    }
    pub fn uncertain(&mut self, failure: Failure) {
        self.failure.get_or_insert(failure);
        self.cleanup_failed.store(true, Ordering::Release);
    }
    fn check(&mut self, stage: Stage, call: impl FnOnce(&O) -> i32) -> Result<(), Failure> {
        let status =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call(&self.operations))).ok();
        if status == Some(0) {
            return Ok(());
        }
        let failure = Failure { stage, status };
        self.uncertain(failure);
        Err(failure)
    }
    pub fn close(&mut self) -> Result<(), Failure> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if let Some(device) = self.device {
            if let Some(callback) = self.callback {
                if self.started {
                    self.check(Stage::Stop, |ops| ops.stop(device, callback))?;
                    self.started = false;
                }
                self.check(Stage::Callback, |ops| {
                    ops.destroy_callback(device, callback)
                })?;
                self.callback = None;
            }
            self.check(Stage::Aggregate, |ops| ops.destroy_aggregate(device))?;
            self.device = None;
        }
        if let Some(tap) = self.tap {
            self.check(Stage::Tap, |ops| ops.destroy_tap(tap))?;
            self.tap = None;
        }
        Ok(())
    }
    pub fn startup_error<E>(&mut self, original: E) -> Result<E, (E, Failure)> {
        match self.close() {
            Ok(()) => Ok(original),
            Err(failure) => Err((original, failure)),
        }
    }
}
impl<P: Copy + 'static, C: 'static, O: Operations<P>> Drop for Owned<P, C, O> {
    fn drop(&mut self) {
        if let Err(failure) = self.close() {
            // The callback may still run. A process-lifetime quarantine is safer
            // than freeing its context or releasing the original vault owner.
            let _ = Box::leak(Box::new((
                self.tap.take(),
                self.device.take(),
                self.callback.take(),
                self.started,
                self.context.take(),
                self.owner.take(),
                failure,
            )));
            tracing::error!(?failure, "native_audio_resources_quarantined");
        }
    }
}

#[cfg(test)]
mod tests;

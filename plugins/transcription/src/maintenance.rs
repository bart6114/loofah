use crate::BatchSessionRegistry;
use hypr_transcription_core::listener2 as core;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard};

impl BatchSessionRegistry {
    pub(crate) fn start(&self) -> Result<OwnedRwLockReadGuard<()>, core::Error> {
        self.maintenance
            .clone()
            .try_read_owned()
            .map_err(|_| core::Error::BatchError("Session maintenance is in progress".into()))
    }

    pub fn admit_session_maintenance(&self) -> Result<OwnedRwLockWriteGuard<()>, crate::Error> {
        let guard = self
            .maintenance
            .clone()
            .try_write_owned()
            .map_err(|_| crate::Error::SessionAlreadyRunning)?;
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| crate::Error::StartSessionFailed)?;
        if !sessions.is_empty() {
            return Err(crate::Error::SessionAlreadyRunning);
        }
        Ok(guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn maintenance_waits_for_owned_runtime_after_the_visible_batch_entry_is_removed() {
        let registry = BatchSessionRegistry::default();
        let runtime = registry.start().unwrap();
        assert!(registry.sessions.lock().unwrap().is_empty());
        assert!(registry.admit_session_maintenance().is_err());
        drop(runtime);
        let proof = registry.admit_session_maintenance().unwrap();
        assert!(registry.start().is_err());
        drop(proof);
        assert!(registry.start().is_ok());
    }

    #[test]
    fn unknown_poisoned_batch_inventory_denies_maintenance() {
        let registry = Arc::new(BatchSessionRegistry::default());
        let other = registry.clone();
        let _ = std::thread::spawn(move || {
            let _inventory = other.sessions.lock().unwrap();
            panic!("unknown batch inventory");
        })
        .join();
        assert!(registry.admit_session_maintenance().is_err());
    }
}

use std::sync::Arc;

use hypr_storage::vault::VaultLease;

use crate::{SessionStore, StoreError};

impl SessionStore {
    pub fn pin_vault_lease(&self, lease: &VaultLease) -> Result<(), StoreError> {
        let canonical = self
            .vault_base
            .canonicalize()
            .map_err(|cause| StoreError::Io(cause.to_string()))?;
        if canonical != lease.canonical_vault() {
            return Err(StoreError::Conflict(
                "Vault lease belongs to a different vault".into(),
            ));
        }
        let pin = lease.pin();
        match self.vault_lease.set(pin) {
            Ok(()) => Ok(()),
            Err(pin)
                if self
                    .vault_lease
                    .get()
                    .is_some_and(|current| Arc::ptr_eq(current, &pin)) =>
            {
                Ok(())
            }
            Err(_) => Err(StoreError::Conflict(
                "Store already has a different vault owner".into(),
            )),
        }
    }

    pub(crate) fn spawn_blocking<F, R>(&self, operation: F) -> tokio::task::JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let lease = self.vault_lease.clone();
        tokio::task::spawn_blocking(move || {
            // The waiter and Store may disappear while native filesystem I/O continues.
            let _lease = lease;
            operation()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        io::Write,
        sync::{Condvar, Mutex},
    };

    use hypr_storage::vault::LeaseAttempt;

    use super::*;

    fn acquire(root: &std::path::Path) -> VaultLease {
        match VaultLease::try_acquire_existing(root).unwrap() {
            LeaseAttempt::Acquired(lease) => lease,
            LeaseAttempt::Busy => panic!("unexpected owner"),
        }
    }

    #[test]
    fn clones_created_before_pin_share_the_same_original_descriptor() {
        let root = tempfile::tempdir().unwrap();
        let lease = acquire(root.path());
        let store = SessionStore::new(root.path().into());
        let other = store.clone();
        let original = Arc::downgrade(&lease.pin());
        store.pin_vault_lease(&lease).unwrap();
        other.pin_vault_lease(&lease).unwrap();
        assert!(Arc::ptr_eq(
            store.vault_lease.get().unwrap(),
            other.vault_lease.get().unwrap()
        ));
        drop(lease);
        drop(store);
        assert!(original.upgrade().is_some());
        assert!(matches!(
            VaultLease::try_acquire_existing(root.path()).unwrap(),
            LeaseAttempt::Busy
        ));
        drop(other);
        assert!(original.upgrade().is_none());
        drop(acquire(root.path()));
    }

    #[test]
    fn a_lease_from_another_vault_never_pins_or_replaces_the_store_owner() {
        let root = tempfile::tempdir().unwrap();
        let other_root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path().into());
        let other = acquire(other_root.path());
        assert!(store.pin_vault_lease(&other).is_err());
        assert!(store.vault_lease.get().is_none());
        let lease = acquire(root.path());
        store.pin_vault_lease(&lease).unwrap();
        assert!(store.pin_vault_lease(&other).is_err());
        assert!(Arc::ptr_eq(store.vault_lease.get().unwrap(), &lease.pin()));
    }

    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }

    #[tokio::test]
    async fn cancelled_waiter_keeps_original_descriptor_through_actual_blocked_file_io() {
        let root = tempfile::tempdir().unwrap();
        let lease = acquire(root.path());
        let original = Arc::downgrade(&lease.pin());
        let store = SessionStore::new(root.path().into());
        store.pin_vault_lease(&lease).unwrap();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Release(gate.clone());
        let path = root.path().join("working");
        let worker_path = path.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (finished, done) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(async move {
            store
                .spawn_blocking(move || {
                    let mut file = File::create(worker_path).unwrap();
                    file.write_all(b"before").unwrap();
                    let _ = started.send(());
                    let mut open = gate.0.lock().unwrap();
                    while !*open {
                        open = gate.1.wait(open).unwrap();
                    }
                    drop(open);
                    file.write_all(b" after").unwrap();
                    file.sync_all().unwrap();
                    let _ = finished.send(());
                })
                .await
                .unwrap();
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        caller.abort();
        let _ = caller.await;
        drop(lease);
        assert!(original.upgrade().is_some());
        assert!(matches!(
            VaultLease::try_acquire_existing(root.path()).unwrap(),
            LeaseAttempt::Busy
        ));
        drop(release);
        tokio::time::timeout(std::time::Duration::from_secs(5), done)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while original.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(acquire(root.path()));
        assert_eq!(std::fs::read(path).unwrap(), b"before after");
    }
}

use std::{fs::File, sync::Arc};

use super::{LifecycleError, rejected};
use crate::{SessionStore, StoreError};

/// A single owned Local operation; cannot be serialized or reused for another session/store.
pub struct LocalSessionLifecycleProof(pub(super) Arc<Inner>);
pub(super) struct Inner {
    session_id: String,
    store: Arc<tokio::sync::Mutex<()>>,
    _lease: Arc<File>,
    _host_and_admission: Box<dyn Send + Sync>,
}

impl SessionStore {
    /// Trusted hosts supply a complete producer/persistence admission guard, not a UI state.
    pub fn checked_local_lifecycle_proof(
        &self,
        session_id: &str,
        lease: Arc<File>,
        host_and_admission: Box<dyn Send + Sync>,
    ) -> Result<LocalSessionLifecycleProof, LifecycleError> {
        crate::validate_session_id(session_id).map_err(rejected)?;
        if !self
            .vault_lease
            .get()
            .is_some_and(|owner| Arc::ptr_eq(owner, &lease))
        {
            return Err(rejected(StoreError::Conflict(
                "Local lifecycle requires the original vault lease".into(),
            )));
        }
        Ok(LocalSessionLifecycleProof(Arc::new(Inner {
            session_id: session_id.into(),
            store: self.write_lock.clone(),
            _lease: lease,
            _host_and_admission: host_and_admission,
        })))
    }

    pub(super) fn check_local_lifecycle_proof(
        &self,
        session_id: &str,
        proof: LocalSessionLifecycleProof,
    ) -> Result<Arc<Inner>, LifecycleError> {
        if proof.0.session_id != session_id || !Arc::ptr_eq(&proof.0.store, &self.write_lock) {
            return Err(rejected(StoreError::Conflict(
                "Local lifecycle proof belongs to another operation".into(),
            )));
        }
        Ok(proof.0)
    }
}

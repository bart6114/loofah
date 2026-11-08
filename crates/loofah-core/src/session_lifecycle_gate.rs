use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

use hypr_loofah_protocol::{ApplicationError, ErrorCode, MutationOutcome};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::{VaultContext, vault::error};

#[derive(Default)]
pub(crate) struct SessionLifecycleGates(Mutex<HashMap<String, Weak<RwLock<()>>>>);

impl SessionLifecycleGates {
    fn session(&self, id: &str) -> Result<Arc<RwLock<()>>, ApplicationError> {
        let mut gates = self.0.lock().map_err(|_| busy())?;
        gates.retain(|_, gate| gate.strong_count() != 0);
        if let Some(gate) = gates.get(id).and_then(Weak::upgrade) {
            return Ok(gate);
        }
        let gate = Arc::new(RwLock::new(()));
        gates.insert(id.into(), Arc::downgrade(&gate));
        Ok(gate)
    }
}

impl VaultContext {
    /// Retain through durable work admission/binding, then let the canonical work
    /// inventory protect the session until its actual publication or termination.
    pub(crate) async fn admit_session_work(
        &self,
        id: &str,
    ) -> Result<OwnedRwLockReadGuard<()>, ApplicationError> {
        self.validate(id)?;
        Ok(self.lifecycle_admission.session(id)?.read_owned().await)
    }

    /// Never queue an exclusive waiter: create/publication can nest shared
    /// admission, which would deadlock behind a queued fair RwLock writer.
    pub(crate) fn try_session_lifecycle_change(
        &self,
        id: &str,
    ) -> Result<OwnedRwLockWriteGuard<()>, ApplicationError> {
        self.validate(id)?;
        self.lifecycle_admission
            .session(id)?
            .try_write_owned()
            .map_err(|_| busy())
    }
}

fn busy() -> ApplicationError {
    let mut cause = error(
        ErrorCode::Conflict,
        "Session is busy; finish recording, processing or transfers before changing it",
    );
    cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
    cause
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exclusive_admission_never_queues_behind_shared_work_or_blocks_nested_admission() {
        let gates = SessionLifecycleGates::default();
        let first = gates.session("note").unwrap().read_owned().await;
        assert!(gates.session("note").unwrap().try_write_owned().is_err());
        let nested = gates.session("note").unwrap().try_read_owned().unwrap();
        assert!(gates.session("other").unwrap().try_write_owned().is_ok());
        drop(first);
        assert!(gates.session("note").unwrap().try_write_owned().is_err());
        drop(nested);
        assert!(gates.session("note").unwrap().try_write_owned().is_ok());
    }

    #[tokio::test]
    async fn shared_admission_waits_only_for_the_owned_short_lifecycle_commit() {
        let gates = SessionLifecycleGates::default();
        let change = gates.session("note").unwrap().try_write_owned().unwrap();
        let gate = gates.session("note").unwrap();
        assert!(gate.clone().try_read_owned().is_err());
        let pending = tokio::spawn(async move { gate.read_owned().await });
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        drop(change);
        let work = pending.await.unwrap();
        assert!(gates.session("note").unwrap().try_write_owned().is_err());
        drop(work);
        assert!(gates.session("note").unwrap().try_write_owned().is_ok());
    }

    #[test]
    fn inactive_session_entries_do_not_accumulate_or_detach_a_live_owner() {
        let gates = SessionLifecycleGates::default();
        let first = gates.session("note").unwrap();
        for id in 0..1000 {
            drop(gates.session(&format!("note-{id}")).unwrap());
        }
        assert!(Arc::ptr_eq(&first, &gates.session("note").unwrap()));
        assert_eq!(gates.0.lock().unwrap().len(), 1);
        drop(first);
        drop(gates.session("next").unwrap());
        assert_eq!(gates.0.lock().unwrap().len(), 1);
    }
}

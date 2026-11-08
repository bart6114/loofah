use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

use hypr_loofah_protocol::{ApplicationError, ErrorCode, MutationOutcome};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::{VaultContext, vault::error};

type RequestIdentity = (&'static str, String, uuid::Uuid);

#[derive(Default)]
pub(crate) struct SessionLifecycleGates {
    sessions: Mutex<HashMap<String, Weak<RwLock<()>>>>,
    requests: Mutex<HashMap<RequestIdentity, Weak<()>>>,
}

impl SessionLifecycleGates {
    fn request(
        &self,
        operation: &'static str,
        namespace: &str,
        request: uuid::Uuid,
    ) -> Result<Arc<()>, ApplicationError> {
        let mut requests = self.requests.lock().map_err(|_| request_busy())?;
        requests.retain(|_, owner| owner.strong_count() != 0);
        let key: RequestIdentity = (operation, namespace.to_owned(), request);
        if requests.get(&key).and_then(Weak::upgrade).is_some() {
            return Err(request_busy());
        }
        let owner = Arc::new(());
        requests.insert(key, Arc::downgrade(&owner));
        Ok(owner)
    }
    fn session(&self, id: &str) -> Result<Arc<RwLock<()>>, ApplicationError> {
        let mut gates = self.sessions.lock().map_err(|_| session_lifecycle_busy())?;
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
    pub(crate) fn admit_session_lifecycle_request(
        &self,
        operation: &'static str,
        namespace: &str,
        request: uuid::Uuid,
    ) -> Result<Arc<()>, ApplicationError> {
        self.lifecycle_admission
            .request(operation, namespace, request)
    }

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
            .map_err(|_| session_lifecycle_busy())
    }
}

fn request_busy() -> ApplicationError {
    let mut cause = error(
        ErrorCode::Conflict,
        "The original lifecycle request is still running; its outcome is unknown",
    );
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    cause
}

pub(crate) fn session_lifecycle_busy() -> ApplicationError {
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
        assert_eq!(gates.sessions.lock().unwrap().len(), 1);
        drop(first);
        drop(gates.session("next").unwrap());
        assert_eq!(gates.sessions.lock().unwrap().len(), 1);
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;
    #[test]
    fn active_request_admission_is_namespaced_and_released_only_with_the_original_owner() {
        let gates = SessionLifecycleGates::default();
        let id = uuid::Uuid::new_v4();
        let owner = gates.request("delete", "alice", id).unwrap();
        assert_eq!(
            gates
                .request("delete", "alice", id)
                .unwrap_err()
                .mutation_outcome,
            Some(MutationOutcome::Unknown)
        );
        assert!(gates.request("delete", "bob", id).is_ok());
        assert!(gates.request("restore", "alice", id).is_ok());
        assert!(
            gates
                .request("delete", "alice", uuid::Uuid::new_v4())
                .is_ok()
        );
        drop(owner);
        assert!(gates.request("delete", "alice", id).is_ok());
        assert!(gates.requests.lock().unwrap().len() <= 1);
    }
}

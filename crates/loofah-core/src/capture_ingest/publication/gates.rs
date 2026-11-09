use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use crate::events::EventHub;
use hypr_loofah_protocol::{ApplicationError, ErrorCode, MutationOutcome};

#[derive(Default, Clone)]
pub(crate) struct PublicationGates(Arc<Mutex<State>>, Arc<tokio::sync::Mutex<()>>);

#[derive(Default)]
struct State {
    initialized: bool,
    global: bool,
    review: BTreeSet<String>,
    operations: BTreeMap<(String, String), Operation>,
}

#[derive(Default)]
struct Operation {
    active: usize,
    unresolved: bool,
}

pub(crate) struct Ticket {
    gates: PublicationGates,
    session: String,
    key: String,
    committing: bool,
    events: Option<EventHub>,
}

impl PublicationGates {
    pub fn check(&self, session: &str) -> Result<(), ApplicationError> {
        let state = self.0.lock().map_err(|_| blocked())?;
        if state.global
            || state.review.contains(session)
            || state
                .operations
                .keys()
                .any(|(candidate, _)| candidate == session)
        {
            return Err(blocked());
        }
        Ok(())
    }

    pub(super) async fn bootstrap_guard(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.1.clone().lock_owned().await
    }

    pub(super) fn initialized(&self) -> bool {
        self.0.lock().is_ok_and(|state| state.initialized)
    }

    pub(super) fn finish_bootstrap(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.initialized = true;
        }
    }

    pub(super) fn block_all(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.global = true;
        }
    }

    pub(super) fn review(&self, session: &str) {
        if let Ok(mut state) = self.0.lock() {
            state.review.insert(session.into());
        }
    }

    pub(super) fn retain(&self, session: &str, key: &str) {
        if let Ok(mut state) = self.0.lock() {
            state
                .operations
                .entry((session.into(), key.into()))
                .or_default()
                .unresolved = true;
        }
    }

    pub(super) fn resolve(&self, session: &str, key: &str) {
        if let Ok(mut state) = self.0.lock() {
            let identity = (session.into(), key.into());
            if let Some(operation) = state.operations.get_mut(&identity) {
                operation.unresolved = false;
                if operation.active == 0 {
                    state.operations.remove(&identity);
                }
            }
        }
    }

    pub(crate) fn begin(&self, session: &str, key: &str) -> Result<Ticket, ApplicationError> {
        let mut state = self.0.lock().map_err(|_| blocked())?;
        if !state.initialized || state.global || state.review.contains(session) {
            return Err(blocked());
        }
        let operation = state
            .operations
            .entry((session.into(), key.into()))
            .or_default();
        operation.active = operation.active.checked_add(1).ok_or_else(blocked)?;
        Ok(Ticket {
            gates: self.clone(),
            session: session.into(),
            key: key.into(),
            committing: false,
            events: None,
        })
    }
}

impl Ticket {
    pub(crate) fn with_events(mut self, events: EventHub) -> Self {
        self.events = Some(events);
        self
    }

    pub(crate) fn committing(&mut self) {
        self.committing = true;
    }

    pub(crate) fn resolved(mut self) {
        self.gates.resolve(&self.session, &self.key);
        self.committing = false;
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let ready = if let Ok(mut state) = self.gates.0.lock() {
            let identity = (self.session.clone(), self.key.clone());
            if let Some(operation) = state.operations.get_mut(&identity) {
                operation.unresolved |= self.committing;
                operation.active = operation.active.saturating_sub(1);
                if operation.active == 0 && !operation.unresolved {
                    state.operations.remove(&identity);
                }
            }
            !state.global
                && !state.review.contains(&self.session)
                && !state
                    .operations
                    .keys()
                    .any(|(session, _)| session == &self.session)
        } else {
            false
        };
        if ready && let Some(events) = &self.events {
            super::invalidate_recording(events, &self.session);
        }
    }
}

fn blocked() -> ApplicationError {
    ApplicationError {
        code: ErrorCode::Unavailable,
        message: "Recording publication requires recovery or review".into(),
        request_id: None,
        retryable: true,
        current_revision: None,
        mutation_outcome: Some(MutationOutcome::NotCommitted),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;

    #[tokio::test]
    async fn only_the_last_resolved_ticket_invalidates_recording_availability() {
        use hypr_loofah_protocol::{ArtifactTarget, Change, IndexEntity};
        let gates = PublicationGates::default();
        gates.finish_bootstrap();
        let events = EventHub::new("vault".into(), 16);
        let cursor = events.publish(Change::SearchReady).cursor;
        let mut changes = events.subscribe(Some(&cursor));
        let first = gates
            .begin("note", "one")
            .unwrap()
            .with_events(events.clone());
        let second = gates
            .begin("note", "two")
            .unwrap()
            .with_events(events.clone());
        first.resolved();
        assert!(changes.recv().now_or_never().is_none());
        second.resolved();
        assert!(gates.check("note").is_ok());
        let Change::IndexChanged {
            entity,
            ids,
            artifacts,
        } = changes.recv().now_or_never().unwrap().unwrap().change
        else {
            panic!("missing readiness invalidation")
        };
        assert_eq!(entity, IndexEntity::Artifacts);
        assert_eq!(ids, ["note"]);
        let artifacts = artifacts.unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].session_id, "note");
        assert_eq!(artifacts[0].target, ArtifactTarget::Recording);
        assert!(changes.recv().now_or_never().is_none());
    }

    #[test]
    fn independent_publication_tickets_cannot_clear_each_others_read_gates() {
        let gates = PublicationGates::default();
        gates.finish_bootstrap();
        let first = gates.begin("note", "one").unwrap();
        let second = gates.begin("note", "two").unwrap();
        assert!(gates.check("note").is_err());
        assert!(gates.check("other").is_ok());
        first.resolved();
        assert!(gates.check("note").is_err());
        second.resolved();
        assert!(gates.check("note").is_ok());
        let first = gates.begin("note", "same").unwrap();
        let second = gates.begin("note", "same").unwrap();
        first.resolved();
        assert!(gates.check("note").is_err());
        second.resolved();
        assert!(gates.check("note").is_ok());
    }

    #[test]
    fn ambiguous_commits_remain_gated_after_caller_exit_and_precommit_retry_failure() {
        let gates = PublicationGates::default();
        gates.finish_bootstrap();
        let mut first = gates.begin("note", "one").unwrap();
        first.committing();
        drop(first);
        assert!(gates.check("note").is_err());
        drop(gates.begin("note", "one").unwrap());
        assert!(gates.check("note").is_err());
        gates.begin("note", "one").unwrap().resolved();
        assert!(gates.check("note").is_ok());
    }

    #[test]
    fn review_and_global_uncertainty_fail_closed_without_blocking_unrelated_known_sessions() {
        let gates = PublicationGates::default();
        assert!(gates.begin("note", "one").is_err());
        gates.finish_bootstrap();
        gates.review("note");
        assert!(gates.begin("note", "one").is_err());
        assert!(gates.check("other").is_ok());
        gates.resolve("note", "one");
        assert!(gates.check("note").is_err());
        gates.block_all();
        assert!(gates.check("other").is_err());
    }
}

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

use hypr_loofah_protocol::{ApplicationError, ErrorCode, MutationOutcome};
use tokio::sync::OwnedRwLockReadGuard;

use crate::{VaultContext, vault::error};

#[derive(Clone)]
pub struct LocalSessionWorkRegistry(Arc<Inner>);
#[derive(Clone)]
pub struct LocalSessionWorkObserver(Weak<Inner>);

struct Inner {
    context: Arc<VaultContext>,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    entries: HashMap<uuid::Uuid, Entry>,
    finished: HashMap<uuid::Uuid, String>,
}
struct Entry {
    session: String,
    _admission: OwnedRwLockReadGuard<()>,
    producer_finished: bool,
    persistence_finished: bool,
}

#[derive(Clone)]
pub struct LocalSessionWork {
    registry: LocalSessionWorkRegistry,
    request: uuid::Uuid,
}

impl LocalSessionWorkRegistry {
    pub fn new(context: Arc<VaultContext>) -> Self {
        Self(Arc::new(Inner {
            context,
            state: Mutex::new(State::default()),
        }))
    }

    pub fn downgrade(&self) -> LocalSessionWorkObserver {
        LocalSessionWorkObserver(Arc::downgrade(&self.0))
    }

    pub async fn begin(
        &self,
        session: String,
        request_id: String,
    ) -> Result<LocalSessionWork, ApplicationError> {
        let request = request(&request_id)?;
        let registry = self.clone();
        tokio::spawn(async move {
            let admission = registry.0.context.admit_session_work(&session).await?;
            registry.0.context.read_note(&session).await?;
            let mut state = registry.0.state.lock().map_err(|_| unavailable())?;
            if state.entries.contains_key(&request) || state.finished.contains_key(&request) {
                let mut cause = error(
                    ErrorCode::Conflict,
                    "The original Local work request already exists",
                );
                cause.mutation_outcome = Some(MutationOutcome::Unknown);
                return Err(cause);
            }
            if state.entries.len() >= 128 {
                return Err(error(
                    ErrorCode::Unavailable,
                    "Unfinished Local session work requires recovery",
                ));
            }
            state.entries.insert(
                request,
                Entry {
                    session,
                    _admission: admission,
                    producer_finished: false,
                    persistence_finished: false,
                },
            );
            drop(state);
            Ok(LocalSessionWork { registry, request })
        })
        .await
        .map_err(|_| unavailable())?
    }

    pub fn persistence_finished(
        &self,
        session: &str,
        request_id: &str,
    ) -> Result<(), ApplicationError> {
        let request = request(request_id)?;
        let mut state = self.0.state.lock().map_err(|_| unavailable())?;
        let Some(entry) = state.entries.get_mut(&request) else {
            return match state.finished.get(&request) {
                Some(original) if original == session => Ok(()),
                Some(_) => Err(error(
                    ErrorCode::Validation,
                    "Local work belongs to another session",
                )),
                None => Err(unavailable()),
            };
        };
        if entry.session != session {
            return Err(error(
                ErrorCode::Validation,
                "Local work belongs to another session",
            ));
        }
        entry.persistence_finished = true;
        if entry.producer_finished {
            let completed = state
                .entries
                .remove(&request)
                .expect("confirmed original entry");
            state.finished.insert(request, completed.session);
        }
        Ok(())
    }
}

impl LocalSessionWorkObserver {
    /// The caller must already own Core's exclusive lifecycle gate for this session.
    pub fn ensure_idle_for(
        &self,
        context: &Arc<VaultContext>,
        session: &str,
    ) -> Result<(), ApplicationError> {
        let registry = self.0.upgrade().ok_or_else(unavailable)?;
        if !Arc::ptr_eq(&registry.context, context) {
            return Err(unavailable());
        }
        let state = registry.state.lock().map_err(|_| unavailable())?;
        if state.entries.values().any(|entry| entry.session == session) {
            return Err(unavailable());
        }
        Ok(())
    }
}

impl LocalSessionWork {
    /// Called by the trusted host only after the original producer and native resources drain.
    pub fn producer_finished(&self) -> Result<(), ApplicationError> {
        let mut state = self.registry.0.state.lock().map_err(|_| unavailable())?;
        if let Some(entry) = state.entries.get_mut(&self.request) {
            entry.producer_finished = true;
            if entry.persistence_finished {
                let completed = state
                    .entries
                    .remove(&self.request)
                    .expect("confirmed original entry");
                state.finished.insert(self.request, completed.session);
            }
        }
        Ok(())
    }

    /// A rejected admission must prove that no producer was dispatched.
    pub fn not_started(&self) -> Result<(), ApplicationError> {
        let mut state = self.registry.0.state.lock().map_err(|_| unavailable())?;
        if let Some(completed) = state.entries.remove(&self.request) {
            state.finished.insert(self.request, completed.session);
        }
        Ok(())
    }
}

fn request(value: &str) -> Result<uuid::Uuid, ApplicationError> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil() && id.hyphenated().to_string() == value)
        .ok_or_else(|| {
            error(
                ErrorCode::Validation,
                "Local work requires a canonical request UUID",
            )
        })
}
fn unavailable() -> ApplicationError {
    error(
        ErrorCode::Unavailable,
        "Local work completion could not be confirmed",
    )
}

#[cfg(test)]
mod tests;

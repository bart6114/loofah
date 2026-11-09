use super::error;
use hypr_loofah_protocol::{ApplicationError, ErrorCode};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct SessionExportState(Arc<Inner>);
#[derive(Default)]
struct Inner {
    state: Mutex<Registry>,
    changed: tokio::sync::Notify,
}
#[derive(Default)]
struct Registry {
    stopping: bool,
    owners: HashMap<String, String>,
    active: HashMap<(String, String, String), CancellationToken>,
    cancelled: HashSet<(String, String, String)>,
    cancellation_full: HashSet<(String, String)>,
}
pub(super) struct ExportWork {
    state: SessionExportState,
    key: (String, String, String),
    pub cancel: CancellationToken,
}
impl SessionExportState {
    pub(super) fn owner(&self, window: &str) -> Result<String, ApplicationError> {
        let mut state = self.0.state.lock().unwrap();
        if state.stopping {
            return Err(error(
                ErrorCode::Unavailable,
                "Export service is shutting down",
            ));
        }
        if let Some(owner) = state.owners.get(window) {
            return Ok(owner.clone());
        }
        if state.owners.len() >= 32 {
            return Err(error(ErrorCode::ResourceLimit, "Too many export windows"));
        }
        let owner = uuid::Uuid::new_v4().to_string();
        state.owners.insert(window.into(), owner.clone());
        Ok(owner)
    }
    pub(super) fn admit(
        &self,
        owner: &str,
        epoch: &str,
        request: &str,
        parent: CancellationToken,
    ) -> Result<ExportWork, ApplicationError> {
        if !uuid::Uuid::parse_str(request).is_ok_and(|id| !id.is_nil() && id.to_string() == request)
        {
            return Err(error(
                ErrorCode::Validation,
                "A canonical export request UUID is required",
            ));
        }
        let mut state = self.0.state.lock().unwrap();
        let key = (owner.into(), epoch.into(), request.into());
        if state.owners.get(owner).map(String::as_str) != Some(epoch) {
            return Err(error(
                ErrorCode::Cancelled,
                "The original export window is closed",
            ));
        }
        if state.stopping {
            return Err(error(
                ErrorCode::Unavailable,
                "Export service is shutting down",
            ));
        }
        if state.cancelled.contains(&key)
            || state
                .cancellation_full
                .contains(&(owner.into(), epoch.into()))
        {
            return Err(error(
                ErrorCode::Cancelled,
                "This export request was cancelled",
            ));
        }
        if state.active.len() >= 2 || state.active.contains_key(&key) {
            return Err(error(
                ErrorCode::ResourceLimit,
                "An export is already active; wait for it to finish",
            ));
        }
        let cancel = parent.child_token();
        state.active.insert(key.clone(), cancel.clone());
        Ok(ExportWork {
            state: self.clone(),
            key,
            cancel,
        })
    }
    pub fn request_shutdown(&self) {
        let mut state = self.0.state.lock().unwrap();
        state.stopping = true;
        for token in state.active.values() {
            token.cancel();
        }
    }
    pub fn pending(&self) -> bool {
        !self.0.state.lock().unwrap().active.is_empty()
    }
    pub fn release_window(&self, owner: &str) {
        let mut state = self.0.state.lock().unwrap();
        state.owners.remove(owner);
        state.cancelled.retain(|(window, _, _)| window != owner);
        state
            .cancellation_full
            .retain(|(window, _)| window != owner);
        for ((window, _, _), token) in &state.active {
            if window == owner {
                token.cancel();
            }
        }
    }
    pub(super) fn cancel(&self, owner: &str, epoch: &str, request: &str) {
        if !uuid::Uuid::parse_str(request).is_ok_and(|id| !id.is_nil() && id.to_string() == request)
        {
            return;
        }
        let mut state = self.0.state.lock().unwrap();
        if state.owners.get(owner).map(String::as_str) != Some(epoch) {
            return;
        }
        let key = (owner.to_owned(), epoch.to_owned(), request.to_owned());
        if let Some(token) = state.active.get(&key) {
            token.cancel();
        }
        // The renderer may send cancellation before the original IPC is admitted.
        // Bound its journal without evicting an unknown still-in-flight request.
        if state
            .cancelled
            .iter()
            .filter(|(window, generation, _)| window == owner && generation == epoch)
            .count()
            >= 128
        {
            state.cancellation_full.insert((owner.into(), epoch.into()));
        } else {
            state.cancelled.insert(key);
        }
    }
    pub async fn drain(&self, deadline: Duration) -> bool {
        tokio::time::timeout(deadline, async {
            loop {
                let changed = self.0.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if !self.pending() {
                    return;
                }
                changed.await;
            }
        })
        .await
        .is_ok()
    }
}
impl ExportWork {
    pub fn publish<T>(
        &self,
        commit: impl FnOnce() -> Result<T, ApplicationError>,
    ) -> Result<T, ApplicationError> {
        let registry = self.state.0.state.lock().unwrap();
        self.check()?;
        if registry.stopping
            || registry.owners.get(&self.key.0) != Some(&self.key.1)
            || !registry.active.contains_key(&self.key)
        {
            return Err(error(
                ErrorCode::Cancelled,
                "The original export window is closed",
            ));
        }
        // Hold only for the already-staged, no-clobber filesystem commit so
        // navigation cannot retire the window between admission and publication.
        let result = commit();
        drop(registry);
        result
    }
    pub fn check(&self) -> Result<(), ApplicationError> {
        if self.cancel.is_cancelled() {
            Err(error(ErrorCode::Cancelled, "Export cancelled"))
        } else {
            Ok(())
        }
    }
}
impl Drop for ExportWork {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.state.0.state.lock().unwrap().active.remove(&self.key);
        self.state.0.changed.notify_waiters();
    }
}

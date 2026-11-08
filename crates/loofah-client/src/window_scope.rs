use std::{
    future::Future,
    sync::{Arc, Mutex},
    task::Poll,
};

use hypr_loofah_protocol::{CaptureScope, ErrorCode, MutationOutcome};
use tokio_util::sync::CancellationToken;

use crate::{
    AuthenticatedScope, AuthenticatedScopeCommitGuard, ProfileLease, RemoteClient, Result,
    cancelled, failure,
};

pub(crate) struct WindowScope {
    authority: Arc<AuthenticatedScope>,
    profile_retired: CancellationToken,
    login_retired: CancellationToken,
    stop: CancellationToken,
    watcher: tokio::task::JoinHandle<()>,
    work: Arc<WindowWorkState>,
}
impl Drop for WindowScope {
    fn drop(&mut self) {
        self.stop.cancel();
        self.watcher.abort();
    }
}

#[derive(Default)]
struct WindowWorkState {
    count: Mutex<usize>,
    changed: tokio::sync::Notify,
}
/// Owns one actual operation in this window, independent of shared capacity.
pub struct WindowWork(Option<Arc<WindowWorkState>>);
impl Drop for WindowWork {
    fn drop(&mut self) {
        if let Some(state) = &self.0 {
            *state.count.lock().unwrap() -= 1;
            state.changed.notify_waiters();
        }
    }
}

impl RemoteClient {
    /// A separately owned window view shares capacity and credentials, never a new login.
    /// Closing it leaves its parent and sibling views usable.
    pub async fn window_view(
        self: &Arc<Self>,
        profile: Arc<ProfileLease>,
        scope: CaptureScope,
        lifetime: CancellationToken,
    ) -> Result<Arc<Self>> {
        self.reject_window_authentication()?;
        let authority = self
            .bind_authenticated_scope(profile, scope, lifetime.clone())
            .await?;
        let guard = authority.guard_commit().await?;
        let login_retired = guard.auth_retirement();
        let profile_retired = authority.profile_retirement();
        let stop = self.stop.child_token();
        let watched_stop = stop.clone();
        let watched_login = login_retired.clone();
        let watched_profile = profile_retired.clone();
        let watcher = tokio::spawn(async move {
            tokio::select! {
                _ = watched_stop.cancelled() => {},
                _ = watched_login.cancelled() => {},
                _ = watched_profile.cancelled() => {},
                _ = lifetime.cancelled() => {},
            }
            watched_stop.cancel();
        });
        let view = Arc::new(Self {
            address: self.address.clone(),
            http: self.http.clone(),
            auth: self.auth.clone(),
            capabilities: self.capabilities.clone(),
            account: self.account.clone(),
            credentials: self.credentials.clone(),
            tokens: self.tokens.clone(),
            stop: stop.clone(),
            upload_writers: self.upload_writers.clone(),
            media_source_readers: self.media_source_readers.clone(),
            window_scope: Some(WindowScope {
                authority,
                profile_retired,
                login_retired,
                stop,
                watcher,
                work: Arc::default(),
            }),
        });
        guard.check_current()?;
        view.check_window_scope()?;
        Ok(view)
    }

    pub fn begin_window_work(&self) -> Result<WindowWork> {
        let state = self.window_scope.as_ref().map(|scope| scope.work.clone());
        if let Some(state) = &state {
            let mut count = state.count.lock().unwrap();
            self.check_window_scope()?;
            *count += 1;
        }
        Ok(WindowWork(state))
    }

    /// Call after disconnect. Existing physical readers keep their work owner
    /// until they really finish; another window's activity cannot delay this drain.
    pub async fn drain_window(&self) {
        if let Some(scope) = &self.window_scope {
            loop {
                let changed = scope.work.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if *scope.work.count.lock().unwrap() == 0 {
                    return;
                }
                changed.await;
            }
        }
    }

    pub fn window_view_lifetime(&self) -> Option<CancellationToken> {
        self.window_scope.as_ref().map(|scope| scope.stop.clone())
    }

    pub(crate) fn check_window_scope(&self) -> Result<()> {
        if let Some(scope) = &self.window_scope
            && (scope.stop.is_cancelled()
                || scope.profile_retired.is_cancelled()
                || scope.login_retired.is_cancelled())
        {
            scope.stop.cancel();
            return Err(cancelled());
        }
        Ok(())
    }

    pub(crate) fn reject_window_authentication(&self) -> Result<()> {
        if self.window_scope.is_some() {
            return Err(failure(
                ErrorCode::Forbidden,
                "This note window uses its original workspace account",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_window_endpoint(&self, segments: &[&str]) -> Result<()> {
        self.check_window_scope()?;
        if let Some(scope) = &self.window_scope
            && segments != ["me"]
            && !matches!(segments, ["vaults", vault, ..] if *vault == scope.authority.scope().vault_id)
        {
            return Err(failure(
                ErrorCode::Forbidden,
                "This note window is bound to its original vault",
            ));
        }
        Ok(())
    }

    pub(crate) async fn window_dispatch_guard(
        &self,
    ) -> Result<Option<AuthenticatedScopeCommitGuard>> {
        self.check_window_scope()?;
        match &self.window_scope {
            Some(scope) => scope.authority.guard_commit().await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn send_window_request(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        let guard = self.window_dispatch_guard().await.map_err(|mut error| {
            error.mutation_outcome = Some(MutationOutcome::NotCommitted);
            error
        })?;
        self.check_window_scope()?;
        let response = request.send();
        tokio::pin!(response);
        // Only logical dispatch holds the original account/profile fence. Body IO,
        // source readers and model work retain their own existing resource owners.
        let first = std::future::poll_fn(|cx| Poll::Ready(response.as_mut().poll(cx))).await;
        drop(guard);
        let result = match first {
            Poll::Ready(result) => result,
            Poll::Pending => response.await,
        };
        let response = result.map_err(|_| crate::unavailable())?;
        if self.window_scope.is_some() && response.status() == reqwest::StatusCode::FORBIDDEN {
            self.stop.cancel();
        }
        Ok(response)
    }
}

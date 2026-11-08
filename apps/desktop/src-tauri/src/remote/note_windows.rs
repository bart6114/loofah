use super::*;
use hypr_loofah_protocol::{CaptureScope, VaultInfo};

#[derive(Clone, serde::Serialize, specta::Type)]
pub struct RemoteNoteWindowInfo {
    pub connection: RemoteConnectionInfo,
    pub member_id: String,
    pub vault: VaultInfo,
    pub session_id: String,
}
pub(super) struct NoteGrant {
    info: Option<RemoteNoteWindowInfo>,
    client: Arc<RemoteClient>,
}

#[derive(Default)]
pub(super) struct NoteDrains {
    state: Mutex<NoteDrainState>,
    stop: CancellationToken,
    changed: tokio::sync::Notify,
}
#[derive(Default)]
struct NoteDrainState {
    active: usize,
    stopping: bool,
}
struct NoteDrain(Arc<NoteDrains>);
impl Drop for NoteDrain {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().active -= 1;
        self.0.changed.notify_waiters();
    }
}
impl NoteDrains {
    fn admit(self: &Arc<Self>) -> Result<NoteDrain, ApplicationError> {
        let mut state = self.state.lock().unwrap();
        if state.stopping {
            return Err(missing());
        }
        if state.active >= 32 {
            return Err(capacity());
        }
        state.active += 1;
        Ok(NoteDrain(self.clone()))
    }
}

struct UnpublishedNote<'a> {
    state: &'a RemoteState,
    label: &'a str,
    published: bool,
}
impl Drop for UnpublishedNote<'_> {
    fn drop(&mut self) {
        if !self.published {
            self.state.release_window(self.label);
        }
    }
}

pub(super) fn require_primary_connection_window(owner: &str) -> Result<(), ApplicationError> {
    if owner.starts_with("remote-note-") {
        return Err(failure(
            ErrorCode::Forbidden,
            "Use this note's original workspace",
        ));
    }
    Ok(())
}

impl RemoteState {
    pub fn note_windows_initialized(&self) -> bool {
        self.note_drains.state.lock().unwrap().active != 0
    }
    pub fn stop_note_windows(&self) {
        self.note_drains.state.lock().unwrap().stopping = true;
        self.note_drains.stop.cancel();
        for connection in self.registry.lock().unwrap().values() {
            for view in connection.views.values() {
                view.disconnect();
            }
        }
    }
    pub async fn drain_note_windows(&self) {
        loop {
            let changed = self.note_drains.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if !self.note_windows_initialized() {
                return;
            }
            changed.await;
        }
    }

    pub(super) fn reserve_note(
        &self,
        connection_id: &str,
        parent: &str,
        label: &str,
        original: &Arc<RemoteClient>,
        view: Arc<RemoteClient>,
        info: RemoteNoteWindowInfo,
    ) -> Result<(), ApplicationError> {
        let mut notes = self.notes.lock().unwrap();
        if self.note_drains.state.lock().unwrap().stopping {
            return Err(missing());
        }
        if notes.len() >= 16 || notes.contains_key(label) {
            return Err(capacity());
        }
        let mut registry = self.registry.lock().unwrap();
        let connection = registry.get_mut(connection_id).ok_or_else(missing)?;
        if !connection.authenticated
            || connection.login.is_some()
            || !Arc::ptr_eq(&connection.client_for(parent)?, original)
            || view.is_closed()
            || connection.leases.values().sum::<usize>() >= 64
        {
            return Err(missing());
        }
        connection.leases.insert(label.into(), 1);
        connection.views.insert(label.into(), view.clone());
        notes.insert(
            label.into(),
            NoteGrant {
                info: Some(info),
                client: view,
            },
        );
        Ok(())
    }

    pub(super) fn claim_note(&self, label: &str) -> Result<RemoteNoteWindowInfo, ApplicationError> {
        let mut notes = self.notes.lock().unwrap();
        let grant = notes.get_mut(label).ok_or_else(missing)?;
        let info = grant.info.as_ref().ok_or_else(missing)?;
        let client = self.client(&info.connection.connection_id, label)?;
        if !Arc::ptr_eq(&client, &grant.client) || client.is_closed() {
            return Err(missing());
        }
        // One renderer bootstrap only. Reload cannot silently adopt a replacement account.
        grant.info.take().ok_or_else(missing)
    }
}

#[tauri::command]
#[specta::specta]
pub async fn remote_open_note_window(
    app: tauri::AppHandle,
    window: tauri::Window,
    connection_id: String,
    request_id: String,
    vault_id: String,
    session_id: String,
) -> Result<(), ApplicationError> {
    require_primary_connection_window(window.label())?;
    uuid::Uuid::parse_str(&request_id)
        .map_err(|_| failure(ErrorCode::Validation, "A UUID request ID is required"))?;
    let drain = app.state::<RemoteState>().note_drains.admit()?;
    let owner = window.label().to_owned();
    let abandoned = CancellationToken::new();
    let _abandon_on_drop = abandoned.clone().drop_guard();
    // The admitted creator owns preparation through actual completion even if
    // its IPC waiter disappears. Shutdown cancels it and waits for this owner.
    tokio::spawn(async move {
        open_note_owned(
            app,
            owner,
            NoteWindowRequest {
                connection_id,
                request_id,
                vault_id,
                session_id,
            },
            abandoned,
            drain,
        )
        .await
    })
    .await
    .map_err(|_| failure(ErrorCode::Internal, "Note window preparation stopped"))?
}

struct NoteWindowRequest {
    connection_id: String,
    request_id: String,
    vault_id: String,
    session_id: String,
}
async fn open_note_owned(
    app: tauri::AppHandle,
    owner: String,
    request: NoteWindowRequest,
    abandoned: CancellationToken,
    drain: NoteDrain,
) -> Result<(), ApplicationError> {
    let NoteWindowRequest {
        connection_id,
        request_id,
        vault_id,
        session_id,
    } = request;
    let state = app.state::<RemoteState>();
    let (client, profile, cancel, _pending) =
        state.admit_export(&connection_id, &owner, &request_id)?;
    let linked_cancel = cancel.clone();
    let stopped = state.note_drains.stop.clone();
    let finished = CancellationToken::new();
    let _finish_on_drop = finished.clone().drop_guard();
    tokio::spawn(async move {
        tokio::select! {
            _ = finished.cancelled() => {},
            _ = abandoned.cancelled() => linked_cancel.cancel(),
            _ = stopped.cancelled() => linked_cancel.cancel(),
        }
    });
    let member = match client
        .execute(
            RemoteOperation::Me,
            &uuid::Uuid::new_v4().to_string(),
            cancel.clone(),
        )
        .await?
    {
        RemoteReply::Me(me) => me.member.id,
        _ => return Err(missing()),
    };
    let vault = match client
        .execute(
            RemoteOperation::Vault {
                vault_id: vault_id.clone(),
            },
            &uuid::Uuid::new_v4().to_string(),
            cancel.clone(),
        )
        .await?
    {
        RemoteReply::Vault(vault) if vault.id == vault_id => vault,
        _ => return Err(missing()),
    };
    let nonce = uuid::Uuid::new_v4().to_string();
    let note = tauri_plugin_windows::AppWindow::RemoteNote(nonce);
    let label = note.to_string();
    let lifetime = state.window_lifetime(&label);
    let mut pending_note = UnpublishedNote {
        state: &state,
        label: &label,
        published: false,
    };
    let scope = CaptureScope {
        server: client.address().into(),
        member_id: member.clone(),
        vault_id,
    };
    let authority = client
        .bind_authenticated_scope(profile.clone(), scope.clone(), lifetime.clone())
        .await?;
    authority.authorize_session(&session_id).await?;
    let view = client.window_view(profile, scope, lifetime).await?;
    let guard = authority.guard_commit().await?;
    if cancel.is_cancelled() {
        return Err(missing());
    }
    let info = RemoteNoteWindowInfo {
        connection: RemoteConnectionInfo {
            connection_id: connection_id.clone(),
            server: client.address().into(),
            capabilities: client.capabilities().clone(),
        },
        member_id: member,
        vault,
        session_id,
    };
    state.reserve_note(&connection_id, &owner, &label, &client, view.clone(), info)?;
    drop(guard);
    let stopped = view.window_view_lifetime().ok_or_else(missing)?;
    let owner = app.clone();
    let watched_view = view.clone();
    let watched_label = label.clone();
    tokio::spawn(async move {
        let _drain = drain;
        stopped.cancelled().await;
        owner.state::<RemoteState>().release_window(&watched_label);
        if let Some(window) = owner.get_webview_window(&watched_label) {
            let _ = window.close();
        }
        watched_view.drain_window().await;
    });
    // The physical window creator keeps its owner even if the invoking future
    // disappears. A late build must close instead of publishing a retired view.
    let creating = view.begin_window_work()?;
    let built = {
        let app = app.clone();
        let view = view.clone();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            let _creating = creating;
            if cancel.is_cancelled() || view.is_closed() {
                return Err(missing());
            }
            let window = note
                .show(&app)
                .map_err(|_| failure(ErrorCode::Unavailable, "Cannot open the note window"))?;
            if cancel.is_cancelled() || view.is_closed() {
                let _ = window.close();
                return Err(missing());
            }
            Ok(())
        })
        .await
    };
    if !matches!(built, Ok(Ok(_))) || cancel.is_cancelled() || view.is_closed() {
        state.release_window(&label);
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.close();
        }
        return Err(failure(
            ErrorCode::Unavailable,
            "Cannot open the note window",
        ));
    }
    pending_note.published = true;
    drop(pending_note);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn remote_claim_note_window<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
) -> Result<RemoteNoteWindowInfo, ApplicationError> {
    app.state::<RemoteState>().claim_note(window.label())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn note_creator_drain_covers_prebootstrap_owned_work_and_closes_admission() {
        let state = RemoteState::default();
        let creator = state.note_drains.admit().unwrap();
        assert!(state.note_windows_initialized());
        let release = Arc::new(tokio::sync::Notify::new());
        let hold = release.clone();
        let joined = tokio::spawn(async move {
            let _creator = creator;
            hold.notified().await;
        });
        // Dropping the IPC waiter cannot release the actual preparation owner.
        drop(joined);
        state.stop_note_windows();
        assert!(state.note_drains.stop.is_cancelled());
        assert!(state.note_drains.admit().is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), state.drain_note_windows())
                .await
                .is_err()
        );
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(1), state.drain_note_windows())
            .await
            .unwrap();
        assert!(!state.note_windows_initialized());
        assert!(state.note_drains.admit().is_err());
    }
}

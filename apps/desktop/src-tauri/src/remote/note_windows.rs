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
    active: std::sync::atomic::AtomicUsize,
    stopping: std::sync::atomic::AtomicBool,
    changed: tokio::sync::Notify,
}
struct NoteDrain(Arc<NoteDrains>);
impl Drop for NoteDrain {
    fn drop(&mut self) {
        self.0
            .active
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        self.0.changed.notify_waiters();
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
        self.note_drains
            .active
            .load(std::sync::atomic::Ordering::SeqCst)
            != 0
    }
    pub fn stop_note_windows(&self) {
        self.note_drains
            .stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
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
        if self
            .note_drains
            .stopping
            .load(std::sync::atomic::Ordering::SeqCst)
        {
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
    let state = app.state::<RemoteState>();
    let (client, profile, cancel, _pending) =
        state.admit_export(&connection_id, window.label(), &request_id)?;
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
    state
        .note_drains
        .active
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let drain = NoteDrain(state.note_drains.clone());
    state.reserve_note(
        &connection_id,
        window.label(),
        &label,
        &client,
        view.clone(),
        info,
    )?;
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

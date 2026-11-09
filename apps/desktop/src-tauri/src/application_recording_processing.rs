use crate::{application::context, local_capture::LocalCaptureState};
use hypr_loofah_core::local_capture_intents::{
    LocalCaptureControlSnapshot, LocalCaptureIntents, LocalCaptureResolution, LocalCaptureStatus,
};
use hypr_loofah_protocol::{ApplicationError, ErrorCode};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

fn service<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<Arc<LocalCaptureIntents>, ApplicationError> {
    let original = context(app)?;
    app.try_state::<Arc<LocalCaptureState>>()
        .ok_or_else(|| {
            hypr_loofah_client::failure(
                ErrorCode::Unavailable,
                "Local recording processing is not initialized",
            )
        })?
        .service_for(&original)
}

#[tauri::command]
#[specta::specta]
pub fn application_recording_processing_available<R: tauri::Runtime>(app: AppHandle<R>) -> bool {
    service(&app).is_ok()
}

#[tauri::command]
#[specta::specta]
pub async fn application_recording_processing_list<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: Option<String>,
) -> Result<Vec<LocalCaptureStatus>, ApplicationError> {
    let service = service(&app)?;
    if let Some(session) = &session_id {
        hypr_loofah_protocol::validate_session_id(session).map_err(|_| {
            hypr_loofah_client::failure(ErrorCode::Validation, "Invalid recording session")
        })?;
    }
    Ok(service
        .statuses()
        .await
        .into_iter()
        .filter(|status| {
            session_id
                .as_ref()
                .is_none_or(|session| session == &status.declaration.session_id)
        })
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn application_recording_processing_status<R: tauri::Runtime>(
    app: AppHandle<R>,
    request_id: String,
) -> Result<LocalCaptureStatus, ApplicationError> {
    let service = service(&app)?;
    validate_request(&request_id)?;
    service.status(&request_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_recover_recording_processing<R: tauri::Runtime>(
    app: AppHandle<R>,
    request_id: String,
) -> Result<LocalCaptureStatus, ApplicationError> {
    let service = service(&app)?;
    validate_request(&request_id)?;
    service.recover(request_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_recording_processing_controls<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: Option<String>,
) -> Result<Vec<LocalCaptureControlSnapshot>, ApplicationError> {
    let service = service(&app)?;
    if let Some(session) = &session_id {
        hypr_loofah_protocol::validate_session_id(session).map_err(|_| {
            hypr_loofah_client::failure(ErrorCode::Validation, "Invalid recording session")
        })?;
    }
    service.controls(session_id.as_deref()).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_cancel_recording_processing<R: tauri::Runtime>(
    app: AppHandle<R>,
    request_id: String,
) -> Result<LocalCaptureControlSnapshot, ApplicationError> {
    let service = service(&app)?;
    validate_request(&request_id)?;
    service.cancel(request_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_resolve_recording_processing<R: tauri::Runtime>(
    app: AppHandle<R>,
    request_id: String,
    resolution: LocalCaptureResolution,
) -> Result<LocalCaptureControlSnapshot, ApplicationError> {
    let service = service(&app)?;
    validate_request(&request_id)?;
    service.resolve(request_id, resolution).await
}

fn validate_request(value: &str) -> Result<(), ApplicationError> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil() && id.to_string() == value)
        .map(|_| ())
        .ok_or_else(|| {
            hypr_loofah_client::failure(
                ErrorCode::Validation,
                "Recording processing requires the original request UUID",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypr_loofah_core::{
        VaultContext,
        local_capture_intents::{LocalCaptureDeclaration, LocalCaptureState as State},
        search::SearchService,
        settings::ConfigState,
        startup::StartupState,
    };
    use hypr_storage::vault::{LeaseAttempt, VaultLease};

    async fn fixture() -> (tempfile::TempDir, Arc<VaultContext>) {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let vault = root.path().join("vault");
        std::fs::create_dir(&vault).unwrap();
        let LeaseAttempt::Acquired(lease) = VaultLease::try_acquire_existing(&vault).unwrap()
        else {
            panic!("owned fixture")
        };
        let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
        let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Original","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
        store.create_note_session(&meta, "Body").await.unwrap();
        let context = Arc::new(
            VaultContext::new(
                "local:original".into(),
                store,
                StartupState::new(Some(&vault)),
                Arc::new(ConfigState::load_or_default(&vault)),
                Arc::new(SearchService::default()),
            )
            .with_operation_storage(root.path().join("operations"))
            .with_vault_lease(lease)
            .unwrap(),
        );
        (root, context)
    }
    #[tokio::test]
    async fn remote_or_uninitialized_host_does_not_open_local_capture_services() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let request = uuid::Uuid::new_v4().to_string();
        assert!(!application_recording_processing_available(
            app.handle().clone()
        ));
        assert!(
            application_recording_processing_controls(app.handle().clone(), None)
                .await
                .is_err()
        );
        assert!(
            application_cancel_recording_processing(app.handle().clone(), request.clone())
                .await
                .is_err()
        );

        assert_eq!(
            application_recording_processing_list(app.handle().clone(), None)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Unavailable
        );
        assert_eq!(
            application_recording_processing_status(app.handle().clone(), request.clone())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Unavailable
        );
        assert_eq!(
            application_recover_recording_processing(app.handle().clone(), request)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Unavailable
        );
        assert!(app.try_state::<Arc<VaultContext>>().is_none());
        assert!(app.try_state::<Arc<LocalCaptureState>>().is_none());
        let (_root, context) = fixture().await;
        app.manage(context);
        app.manage(Arc::new(LocalCaptureState::default()));
        assert!(!application_recording_processing_available(
            app.handle().clone()
        ));
        assert!(
            application_recording_processing_list(app.handle().clone(), None)
                .await
                .is_err()
        );
        assert!(!app.state::<Arc<LocalCaptureState>>().initialized());
    }
    #[tokio::test]
    async fn read_and_explicit_recovery_preserve_original_request_and_owner() {
        let (_root, context) = fixture().await;
        let state = Arc::new(LocalCaptureState::default());
        state.initialize(context.clone()).await.unwrap();
        let request = uuid::Uuid::new_v4().to_string();
        let original = state.service_for(&context).unwrap();
        let intent = original
            .declare(LocalCaptureDeclaration {
                request_id: request.clone(),
                session_id: "note".into(),
                append: false,
                batch_requested: false,
                transcription: None,
                summary: None,
            })
            .await
            .unwrap();
        intent.not_started().await.unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(context.clone());
        app.manage(state.clone());
        assert!(application_recording_processing_available(
            app.handle().clone()
        ));
        let listed =
            application_recording_processing_list(app.handle().clone(), Some("note".into()))
                .await
                .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].declaration.request_id, request);
        assert_eq!(listed[0].state, State::NotStarted);
        let controls =
            application_recording_processing_controls(app.handle().clone(), Some("note".into()))
                .await
                .unwrap();
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].status, listed[0]);
        assert!(controls[0].actions.is_empty());
        assert_eq!(controls[0].revision.len(), 64);
        assert!(
            application_cancel_recording_processing(app.handle().clone(), request.clone())
                .await
                .is_err()
        );

        assert!(
            application_recording_processing_list(app.handle().clone(), Some("other".into()))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            application_recording_processing_status(app.handle().clone(), request.clone())
                .await
                .unwrap(),
            listed[0]
        );
        assert_eq!(
            application_recover_recording_processing(app.handle().clone(), request.clone())
                .await
                .unwrap(),
            listed[0]
        );
        assert_eq!(original.statuses().await.len(), 1);
        assert!(
            application_recover_recording_processing(
                app.handle().clone(),
                uuid::Uuid::new_v4().to_string()
            )
            .await
            .is_err()
        );
        assert!(
            application_recording_processing_status(app.handle().clone(), "bad-request".into())
                .await
                .is_err()
        );
        let (_other_root, replacement) = fixture().await;
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(replacement);
        app.manage(state.clone());
        assert!(!application_recording_processing_available(
            app.handle().clone()
        ));
        assert!(
            application_recover_recording_processing(app.handle().clone(), request)
                .await
                .is_err()
        );
        assert_eq!(original.statuses().await.len(), 1);
        state.drain().await.unwrap();
    }
}

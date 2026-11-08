use std::sync::Arc;

use hypr_loofah_client::{JobSubmissionReceipt, JobSubmissions};
use hypr_loofah_protocol::{ApplicationError, ErrorCode, ProcessingJobSnapshot, SummaryJobRequest};
use tauri::Manager;

#[derive(Default)]
pub struct JobSubmissionState(tokio::sync::OnceCell<Arc<JobSubmissions>>);

async fn store<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Arc<JobSubmissions>, ApplicationError> {
    app.state::<JobSubmissionState>()
        .0
        .get_or_try_init(|| async {
            let path = app
                .path()
                .app_local_data_dir()
                .map_err(|_| {
                    hypr_loofah_client::failure(
                        ErrorCode::Internal,
                        "Cannot locate device summary request storage",
                    )
                })?
                .join("job-submissions");
            JobSubmissions::new(path)
        })
        .await
        .cloned()
}

#[tauri::command]
#[specta::specta]
pub async fn device_job_submission_prepare<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scope: String,
    input: SummaryJobRequest,
    create_only: Option<bool>,
) -> Result<JobSubmissionReceipt, ApplicationError> {
    let store = store(&app).await?;
    if create_only.unwrap_or(false) {
        store.prepare_new(scope, input).await
    } else {
        store.prepare(scope, input).await
    }
}

#[tauri::command]
#[specta::specta]
pub async fn device_job_submissions<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scope: String,
    session_id: Option<String>,
) -> Result<Vec<JobSubmissionReceipt>, ApplicationError> {
    store(&app).await?.list(scope, session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn device_job_submission_get<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scope: String,
    request_id: String,
) -> Result<Option<JobSubmissionReceipt>, ApplicationError> {
    store(&app).await?.load(scope, request_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn device_job_submission_observe<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scope: String,
    request_id: String,
    snapshot: ProcessingJobSnapshot,
) -> Result<JobSubmissionReceipt, ApplicationError> {
    store(&app)
        .await?
        .observe(scope, request_id, snapshot)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn device_job_submission_forget<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scope: String,
    request_id: String,
) -> Result<bool, ApplicationError> {
    store(&app).await?.forget(scope, request_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_commands_round_trip_device_receipts_without_login_or_a_vault_session() {
        for target in [
            hypr_loofah_protocol::DocumentTarget::Summary,
            hypr_loofah_protocol::DocumentTarget::TemplateOutput {
                document_id: "existing-output".into(),
            },
        ] {
            for policy in [
                hypr_loofah_protocol::TitlePolicy::Preserve,
                hypr_loofah_protocol::TitlePolicy::GenerateIfEmpty,
            ] {
                round_trip(target.clone(), policy).await;
            }
        }
    }

    async fn round_trip(
        target: hypr_loofah_protocol::DocumentTarget,
        title_policy: hypr_loofah_protocol::TitlePolicy,
    ) {
        let root = tempfile::tempdir().unwrap();
        let store = JobSubmissions::new(root.path().join("requests")).unwrap();
        let state = JobSubmissionState::default();
        assert!(state.0.set(store).is_ok());
        let app = tauri::test::mock_builder()
            .manage(state)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let scope = "local:fixture".to_owned();
        let input = SummaryJobRequest {
            title_policy,
            session_id: "note".into(),
            target,
            expected_revision: "baseline".into(),
        };
        let receipt = device_job_submission_prepare(
            app.handle().clone(),
            scope.clone(),
            input.clone(),
            Some(true),
        )
        .await
        .unwrap();
        assert_eq!(
            device_job_submission_prepare(
                app.handle().clone(),
                scope.clone(),
                input.clone(),
                Some(true)
            )
            .await
            .unwrap_err()
            .code,
            ErrorCode::Conflict
        );
        assert_eq!(
            device_job_submission_prepare(app.handle().clone(), scope.clone(), input, None)
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(
            device_job_submission_get(
                app.handle().clone(),
                scope.clone(),
                receipt.request_id.clone()
            )
            .await
            .unwrap(),
            Some(receipt.clone())
        );
        assert_eq!(
            device_job_submissions(app.handle().clone(), scope.clone(), Some("note".into()))
                .await
                .unwrap(),
            [receipt.clone()]
        );
        assert!(
            device_job_submissions(app.handle().clone(), "different-account".into(), None)
                .await
                .unwrap()
                .is_empty()
        );
        let snapshot = ProcessingJobSnapshot {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: "note".into(),
            target: receipt.input.target.clone(),
            state: hypr_loofah_protocol::ProcessingJobState::Succeeded,
            phase: None,
            failure: None,
            created_at: "time".into(),
            updated_at: "time".into(),
        };
        let observed = device_job_submission_observe(
            app.handle().clone(),
            scope.clone(),
            receipt.request_id.clone(),
            snapshot.clone(),
        )
        .await
        .unwrap();
        assert_eq!(observed.job, Some(snapshot));
        assert!(
            !device_job_submission_forget(
                app.handle().clone(),
                "different-account".into(),
                receipt.request_id.clone()
            )
            .await
            .unwrap()
        );
        assert!(
            device_job_submission_forget(app.handle().clone(), scope.clone(), receipt.request_id)
                .await
                .unwrap()
        );
        assert!(
            device_job_submissions(app.handle().clone(), scope, None)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

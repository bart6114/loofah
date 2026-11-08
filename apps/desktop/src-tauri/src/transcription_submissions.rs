use std::sync::Arc;

use hypr_loofah_client::{TranscriptionSubmissionReceipt, TranscriptionSubmissions};
use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, TranscriptionJobRequest, TranscriptionJobSnapshot,
};
use tauri::Manager;

#[derive(Default)]
pub struct TranscriptionSubmissionState(tokio::sync::OnceCell<Arc<TranscriptionSubmissions>>);

async fn store<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Arc<TranscriptionSubmissions>, ApplicationError> {
    app.state::<TranscriptionSubmissionState>()
        .0
        .get_or_try_init(|| async {
            let path = app
                .path()
                .app_local_data_dir()
                .map_err(|_| {
                    hypr_loofah_client::failure(
                        ErrorCode::Internal,
                        "Cannot locate device transcription request storage",
                    )
                })?
                .join("transcription-submissions");
            TranscriptionSubmissions::new(path)
        })
        .await
        .cloned()
}

#[tauri::command]
#[specta::specta]
pub async fn device_transcription_submission_prepare<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scope: String,
    input: TranscriptionJobRequest,
) -> Result<TranscriptionSubmissionReceipt, ApplicationError> {
    store(&app).await?.prepare(scope, input).await
}

#[tauri::command]
#[specta::specta]
pub async fn device_transcription_submissions<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scope: String,
    session_id: Option<String>,
) -> Result<Vec<TranscriptionSubmissionReceipt>, ApplicationError> {
    store(&app).await?.list(scope, session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn device_transcription_submission_get<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scope: String,
    request_id: String,
) -> Result<Option<TranscriptionSubmissionReceipt>, ApplicationError> {
    store(&app).await?.load(scope, request_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn device_transcription_submission_observe<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scope: String,
    request_id: String,
    snapshot: TranscriptionJobSnapshot,
) -> Result<TranscriptionSubmissionReceipt, ApplicationError> {
    store(&app)
        .await?
        .observe(scope, request_id, snapshot)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn device_transcription_submission_forget<R: tauri::Runtime>(
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
    async fn native_transcription_receipts_round_trip_without_network_or_transcript_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let state = TranscriptionSubmissionState::default();
        assert!(
            state
                .0
                .set(TranscriptionSubmissions::new(temp.path().join("receipts")).unwrap())
                .is_ok()
        );
        let app = tauri::test::mock_builder()
            .manage(state)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let input = TranscriptionJobRequest {
            session_id: "note".into(),
            expected_recording_sha256: "a".repeat(64),
            expected_transcript: hypr_loofah_protocol::AbsentTranscriptExpectation::Absent,
            model: "whisper-small".into(),
            languages: vec!["nl".into()],
            keywords: vec!["Loofah".into()],
        };
        let receipt = device_transcription_submission_prepare(
            app.handle().clone(),
            "account/vault".into(),
            input.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            device_transcription_submission_prepare(
                app.handle().clone(),
                receipt.scope.clone(),
                input
            )
            .await
            .unwrap(),
            receipt
        );
        assert_eq!(
            device_transcription_submission_get(
                app.handle().clone(),
                receipt.scope.clone(),
                receipt.request_id.clone()
            )
            .await
            .unwrap(),
            Some(receipt.clone())
        );
        assert_eq!(
            device_transcription_submissions(
                app.handle().clone(),
                receipt.scope.clone(),
                Some("note".into())
            )
            .await
            .unwrap(),
            [receipt.clone()]
        );
        let snapshot = TranscriptionJobSnapshot {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: "note".into(),
            state: hypr_loofah_protocol::TranscriptionJobState::Queued,
            phase: None,
            created_at: "time".into(),
            updated_at: "time".into(),
            result_available: false,
            failure: None,
            publication: None,
        };
        let observed = device_transcription_submission_observe(
            app.handle().clone(),
            receipt.scope.clone(),
            receipt.request_id.clone(),
            snapshot.clone(),
        )
        .await
        .unwrap();
        assert_eq!(observed.job, Some(snapshot));
        assert!(
            !device_transcription_submission_forget(
                app.handle().clone(),
                "different-account".into(),
                receipt.request_id.clone()
            )
            .await
            .unwrap()
        );
        assert!(
            device_transcription_submission_forget(
                app.handle().clone(),
                receipt.scope,
                receipt.request_id
            )
            .await
            .unwrap()
        );
    }
}

use crate::{RemoteClient, Result, failure, invalid, json_body};
use hypr_loofah_protocol::*;
use reqwest::Method;
use serde::{Serialize, de::DeserializeOwned};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
mod authority;
mod download;
mod submissions;
mod upload;
pub use authority::{MediaAuthority, MediaScope, MediaSourceExportGuard};
pub use submissions::{MediaSubmission, MediaSubmissions};
const SNAPSHOT_LIMIT: usize = 16 * 1024 * 1024;
impl RemoteClient {
    pub fn media_source_export_capability(&self, cancel: &CancellationToken) -> Result<()> {
        self.media_jobs_capability(cancel)?;
        if !self
            .capabilities
            .features
            .iter()
            .any(|feature| feature == "media_source_export")
        {
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support original media source export. Upgrade the server and reconnect.",
            ));
        }
        Ok(())
    }

    pub fn media_initial_metadata_capability(&self, cancel: &CancellationToken) -> Result<()> {
        self.media_jobs_capability(cancel)?;
        if !self
            .capabilities
            .features
            .iter()
            .any(|f| f == "media_initial_metadata")
        {
            let mut cause = failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support initial media metadata. Upgrade the server and reconnect.",
            );
            cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
            return Err(cause);
        }
        Ok(())
    }
    pub fn media_jobs_capability(&self, cancel: &CancellationToken) -> Result<()> {
        if self.is_closed() || cancel.is_cancelled() {
            return Err(crate::cancelled());
        }
        if !self.capabilities.features.iter().any(|f| f == "media_jobs") {
            let mut cause = failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support media jobs. Upgrade the server and reconnect.",
            );
            cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
            return Err(cause);
        }
        Ok(())
    }
}
impl MediaAuthority {
    pub async fn source(&self, session: &str) -> Result<MediaSessionSnapshot> {
        validate_session_id(session).map_err(|cause| invalid(&cause.to_string()))?;
        let value: MediaSessionSnapshot = self
            .json(
                Method::GET,
                &["source", session],
                None::<&()>,
                &uuid::Uuid::new_v4().to_string(),
                SNAPSHOT_LIMIT,
            )
            .await?;
        validate_media_session_snapshot(&value).map_err(bad_reply)?;
        if value.metadata.meta.id != session {
            return Err(bad_reply("Media source session changed"));
        }
        Ok(value)
    }
    pub async fn list(&self, session: Option<&str>) -> Result<Vec<MediaRequestStatus>> {
        if let Some(session) = session {
            validate_session_id(session).map_err(|cause| invalid(&cause.to_string()))?;
        }
        let values: Vec<MediaRequestStatus> = self
            .json(
                Method::GET,
                &["requests"],
                None::<&()>,
                &uuid::Uuid::new_v4().to_string(),
                16 * 1024 * 1024,
            )
            .await?;
        if values.len() > 4096 {
            return Err(bad_reply("Media inventory exceeds its entry budget"));
        }
        for value in &values {
            validate_status(value)?;
        }
        Ok(values
            .into_iter()
            .filter(|value| {
                session.is_none_or(|id| {
                    value.declaration.operation.session_id() == Some(id)
                        || value
                            .media
                            .as_ref()
                            .is_some_and(|media| media.session_id == id)
                })
            })
            .collect())
    }
    pub async fn status(&self, request: &str) -> Result<MediaRequestStatus> {
        uuid(request)?;
        let value: MediaRequestStatus = self
            .json(
                Method::GET,
                &["requests", request],
                None::<&()>,
                request,
                SNAPSHOT_LIMIT,
            )
            .await?;
        validate_status(&value)?;
        if value.declaration.request_id != request {
            return Err(bad_reply("Media status request changed"));
        }
        Ok(value)
    }
    pub async fn summary_status(&self, request: &str) -> Result<MediaSummaryStatus> {
        let media = self.status(request).await?;
        let value: MediaSummaryStatus = self
            .json(
                Method::GET,
                &["requests", request, "summary"],
                None::<&()>,
                request,
                SNAPSHOT_LIMIT,
            )
            .await?;
        uuid(&value.child_request_id)
            .map_err(|_| bad_reply("Invalid original media summary child"))?;
        let session = media
            .declaration
            .operation
            .session_id()
            .or_else(|| media.media.as_ref().map(|m| m.session_id.as_str()));
        if value.request_id != request
            || session != Some(value.session_id.as_str())
            || value.job.as_ref().is_some_and(|job| {
                job.session_id != value.session_id
                    || media
                        .declaration
                        .summary
                        .as_ref()
                        .is_none_or(|summary| summary.target != job.target)
            })
            || (value.state == MediaSummaryState::Child && value.job.is_none())
        {
            return Err(bad_reply(
                "Media summary response changed its original scope",
            ));
        }
        Ok(value)
    }
    pub async fn result(&self, request: &str) -> Result<Option<TranscriptWithData>> {
        let status = self.status(request).await?;
        let value: Option<TranscriptWithData> = self
            .json(
                Method::GET,
                &["requests", request, "result"],
                None::<&()>,
                request,
                TRANSCRIPTION_RESULT_RESPONSE_LIMIT,
            )
            .await?;
        if let Some(value) = &value {
            let expected_session = status
                .declaration
                .operation
                .session_id()
                .or_else(|| status.media.as_ref().map(|m| m.session_id.as_str()));
            if expected_session != Some(value.session_id.as_str())
                || value.words.len() > 250_000
                || value.speaker_hints.len() > 500_000
                || !value.started_at.is_finite()
            {
                return Err(bad_reply(
                    "Retained media transcript does not match the original request",
                ));
            }
            if let Some(id) = status.media.as_ref().and_then(|m| m.transcript_id.as_ref()) {
                if id != &value.id {
                    return Err(bad_reply("Retained media transcript identity changed"));
                }
            }
        }
        Ok(value)
    }
    pub async fn declare(&self, submission: &MediaSubmission) -> Result<MediaAdmission> {
        self.validate_submission(submission)?;
        let declaration = &submission.declaration;
        let admission: MediaAdmission = self
            .json(
                Method::POST,
                &["requests"],
                Some(declaration),
                &declaration.request_id,
                SNAPSHOT_LIMIT,
            )
            .await?;
        validate_status(&admission.status)?;
        if submission
            .admission_revision
            .as_ref()
            .is_some_and(|revision| revision != &admission.revision)
            || admission.declaration != *declaration
            || admission.status.declaration != *declaration
            || admission.revision != admission.status.revision
        {
            return Err(bad_reply(
                "Media admission changed its original declaration",
            ));
        }
        Ok(admission)
    }
    pub async fn cancel(&self, request: &str) -> Result<MediaRequestStatus> {
        self.control(request, "cancel").await
    }
    /// Explicitly revisits the original journal, redeclaring only the same saved input after NotFound.
    pub async fn recover(&self, submission: &MediaSubmission) -> Result<MediaRequestStatus> {
        self.validate_submission(submission)?;
        let value = match self
            .control(&submission.declaration.request_id, "recover")
            .await
        {
            Ok(value) => value,
            Err(cause) if cause.code == ErrorCode::NotFound => {
                self.declare(submission).await?.status
            }
            Err(cause) => return Err(cause),
        };
        if value.declaration != submission.declaration
            || submission
                .admission_revision
                .as_ref()
                .is_some_and(|revision| revision != &value.revision)
        {
            return Err(bad_reply("Recovered media declaration changed"));
        }
        Ok(value)
    }
    async fn control(&self, request: &str, action: &str) -> Result<MediaRequestStatus> {
        uuid(request)?;
        let value: MediaRequestStatus = self
            .json(
                Method::POST,
                &["requests", request, action],
                Some(&serde_json::json!({})),
                request,
                SNAPSHOT_LIMIT,
            )
            .await?;
        validate_status(&value)?;
        if value.declaration.request_id != request {
            return Err(bad_reply("Media control request changed"));
        }
        Ok(value)
    }
    fn validate_submission(&self, submission: &MediaSubmission) -> Result<()> {
        if submission.schema_version != 1 || submission.scope != *self.scope() {
            return Err(failure(
                ErrorCode::Conflict,
                "Original media submission scope changed",
            ));
        }
        if matches!(
            submission.declaration.operation,
            MediaOperation::ImportAudio {
                initial: Some(_),
                ..
            }
        ) {
            self.client
                .media_initial_metadata_capability(&self.cancel)?;
        }
        validate_media_declaration(&submission.declaration).map_err(invalid)
    }
    async fn json<T: DeserializeOwned + Send, B: Serialize + Sync>(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&B>,
        request: &str,
        limit: usize,
    ) -> Result<T> {
        let mutation = method != Method::GET;
        let prepared = async {
            self.check_current().await?;
            let guard = self.guard().await?;
            let mut path = vec!["vaults", self.scope().vault_id.as_str(), "media"];
            path.extend_from_slice(segments);
            let mut req = self
                .client
                .http
                .request(method, self.client.endpoint(&path)?)
                .bearer_auth(&guard.access)
                .header("x-loofah-api-major", API_MAJOR.to_string())
                .header("x-request-id", request)
                .timeout(Duration::from_secs(35));
            if let Some(body) = body {
                req = req.json(body);
            }
            Ok::<_, ApplicationError>((req, guard))
        }
        .await
        .map_err(|e| outcome(e, request, mutation, false))?;
        let (req, _guard) = prepared;
        let work = async {
            let response = req.send().await.map_err(|_| crate::unavailable())?;
            if !response.status().is_success() {
                return Err(json_body::<ApplicationError>(response, 131_072)
                    .await
                    .unwrap_or_else(|_| crate::unavailable()));
            }
            json_body(response, limit).await
        };
        tokio::select! {
            _ = self.cancel.cancelled() => Err(crate::cancelled()),
            _ = self.lifetime.cancelled() => Err(crate::cancelled()),
            _ = self.client.stop.cancelled() => Err(crate::cancelled()),
            value = work => value,
        }
        .map_err(|e| outcome(e, request, mutation, true))
    }
}
fn validate_status(value: &MediaRequestStatus) -> Result<()> {
    validate_media_declaration(&value.declaration).map_err(bad_reply)?;
    if !valid_revision(&value.revision) {
        return Err(bad_reply("Invalid media admission revision"));
    }
    if let Some(media) = &value.media {
        if media.request_id != value.declaration.request_id
            || value
                .declaration
                .operation
                .session_id()
                .is_some_and(|id| id != media.session_id)
        {
            return Err(bad_reply("Media status scope changed"));
        }
        validate_session_id(&media.session_id)
            .map_err(|_| bad_reply("Invalid media result session"))?;
        let kind = match value.declaration.operation {
            MediaOperation::ImportAudio { .. } => LocalMediaKind::ImportAudio,
            MediaOperation::Transcribe { .. } => LocalMediaKind::Transcribe,
            MediaOperation::ImportSubtitles { .. } => LocalMediaKind::ImportSubtitles,
        };
        if media.kind != kind {
            return Err(bad_reply("Media result operation changed"));
        }
        if media
            .imported
            .as_ref()
            .is_some_and(|meta| meta.id != media.session_id || kind != LocalMediaKind::ImportAudio)
        {
            return Err(bad_reply("Original imported session receipt changed"));
        }
    }
    Ok(())
}
fn outcome(
    mut cause: ApplicationError,
    request: &str,
    mutation: bool,
    dispatched: bool,
) -> ApplicationError {
    cause.request_id.get_or_insert_with(|| request.into());
    if mutation && cause.mutation_outcome.is_none() {
        cause.mutation_outcome = Some(
            if dispatched
                && matches!(
                    cause.code,
                    ErrorCode::Cancelled
                        | ErrorCode::Unavailable
                        | ErrorCode::Internal
                        | ErrorCode::ResourceLimit
                        | ErrorCode::IncompatibleVersion
                )
            {
                MutationOutcome::Unknown
            } else {
                MutationOutcome::NotCommitted
            },
        );
    }
    cause
}
fn bad_reply(message: &str) -> ApplicationError {
    failure(ErrorCode::IncompatibleVersion, message)
}
fn uuid(value: &str) -> Result<()> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil() && id.to_string() == value)
        .map(|_| ())
        .ok_or_else(|| invalid("Canonical media request UUID required"))
}

fn valid_revision(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

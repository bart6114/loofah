use crate::{
    VaultContext,
    local_cli::LocalCli,
    transcription_jobs::ledger,
    vault::{error, storage_error},
};
use hypr_loofah_protocol::*;
use hypr_vault_write::transcript_publication::{
    ProcessingSourceBinding, TranscriptOnlyBinding, TranscriptPublicationIdentity,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

mod completion;
mod journal;
#[cfg(test)]
mod tests;
const OWNER: &str = "desktop";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ManualMediaKind {
    AudioImport,
    Transcription,
    Subtitles,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct ManualMediaSummaryDeclaration {
    pub request_id: String,
    pub session_id: String,
    pub fresh_session: bool,
    pub kind: ManualMediaKind,
    pub summary: Option<CaptureProcessingSummary>,
    pub only_if_empty: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ManualMediaSummaryState {
    PendingMedia,
    SourceBound,
    Skipped,
    Child,
}
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct ManualMediaSummaryStatus {
    pub request_id: String,
    pub session_id: String,
    pub state: ManualMediaSummaryState,
    pub child_request_id: String,
    pub job: Option<ProcessingJobSnapshot>,
    pub reason: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    vault_id: String,
    declaration: ManualMediaSummaryDeclaration,
    session_created_at: Option<String>,
    request: Option<SummaryJobRequest>,
    source: Option<ManualMediaSource>,
    status: ManualMediaSummaryStatus,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManualMediaSource {
    transcript: TranscriptOnlyBinding,
    recording_sha256: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManualMediaChildOrigin {
    parent_request_id: String,
    child_request_id: String,
    session_id: String,
    source: ManualMediaSource,
}
impl ManualMediaChildOrigin {
    pub(crate) fn validate_request(
        &self,
        request: &SummaryJobRequest,
    ) -> Result<(), ApplicationError> {
        uuid(&self.parent_request_id)?;
        uuid(&self.child_request_id)?;
        if request.session_id != self.session_id
            || !is_transcription_sha256(&self.source.transcript.transcript_revision)
            || !self.source.transcript.transcript_present
            || self
                .source
                .recording_sha256
                .as_ref()
                .is_some_and(|sha| !is_transcription_sha256(sha))
            || chrono::DateTime::parse_from_rfc3339(&self.source.transcript.session_created_at)
                .is_err()
        {
            return Err(review("Manual summary child source changed"));
        }
        Ok(())
    }
}
pub(crate) enum ManualMediaGuard {
    Audio {
        _guard: hypr_vault_write::transcript_publication::ProcessingSourceGuard,
    },
    Transcript {
        _guard: hypr_vault_write::transcript_publication::TranscriptOnlyGuard,
    },
}
impl VaultContext {
    pub async fn declare_manual_media_summary(
        self: &Arc<Self>,
        declaration: ManualMediaSummaryDeclaration,
    ) -> Result<ManualMediaSummaryStatus, ApplicationError> {
        let owner = self.clone();
        tokio::spawn(async move { owner.declare_manual_summary_owned(declaration).await })
            .await
            .map_err(review)?
    }
    async fn declare_manual_summary_owned(
        self: &Arc<Self>,
        declaration: ManualMediaSummaryDeclaration,
    ) -> Result<ManualMediaSummaryStatus, ApplicationError> {
        validate_declaration(&declaration)?;
        let _request = self.admit_session_lifecycle_request(
            "manual-media-summary",
            OWNER,
            uuid(&declaration.request_id)?,
        )?;
        if let Some(record) = journal::load(self, &declaration.request_id).await? {
            if record.declaration != declaration {
                return Err(error(
                    ErrorCode::Conflict,
                    "Original manual summary declaration changed",
                ));
            }
            return self.manual_summary_status_owned(record).await;
        }
        let _admission = self.admit_session_work(&declaration.session_id).await?;
        let created = if declaration.fresh_session {
            if declaration.kind != ManualMediaKind::AudioImport
                || self.created_session_id("local-cli-import", &declaration.request_id)?
                    != declaration.session_id
            {
                return Err(error(
                    ErrorCode::Validation,
                    "Fresh summary target is not the original import session",
                ));
            }
            match self.read_metadata(&declaration.session_id).await {
                Err(cause) if cause.code == ErrorCode::NotFound => None,
                Ok(_) => {
                    return Err(error(
                        ErrorCode::Conflict,
                        "Fresh import session already exists without its summary declaration",
                    ));
                }
                Err(cause) => return Err(cause),
            }
        } else {
            Some(
                self.read_metadata(&declaration.session_id)
                    .await?
                    .meta
                    .created_at,
            )
        };
        let request = if let Some(options) = &declaration.summary {
            let revision = if declaration.fresh_session {
                hypr_vault_write::SessionStore::absent_document_revision(
                    &declaration.session_id,
                    &options.target,
                )
                .map_err(|cause| storage_error(cause, false))?
            } else {
                self.read_document(&declaration.session_id, &options.target)
                    .await?
                    .revision
            };
            Some(SummaryJobRequest {
                session_id: declaration.session_id.clone(),
                target: options.target.clone(),
                expected_revision: revision,
                title_policy: options.title_policy,
                only_if_empty: declaration.only_if_empty,
            })
        } else {
            None
        };
        if let Some(request) = &request {
            crate::jobs::validate_request(request)?;
        }
        let record = Record {
            version: 1,
            vault_id: self.events.vault_id(),
            session_created_at: created,
            request,
            source: None,
            status: ManualMediaSummaryStatus {
                request_id: declaration.request_id.clone(),
                session_id: declaration.session_id.clone(),
                state: ManualMediaSummaryState::PendingMedia,
                child_request_id: uuid::Uuid::new_v4().to_string(),
                job: None,
                reason: None,
            },
            declaration,
        };
        journal::persist(self, &record, true).await?;
        Ok(record.status)
    }
    pub async fn manual_media_summary_status(
        self: &Arc<Self>,
        request: &str,
    ) -> Result<ManualMediaSummaryStatus, ApplicationError> {
        self.manual_summary_status_owned(
            journal::load(self, request).await?.ok_or_else(|| {
                error(ErrorCode::NotFound, "Manual media summary intent not found")
            })?,
        )
        .await
    }
    async fn manual_summary_status_owned(
        &self,
        mut record: Record,
    ) -> Result<ManualMediaSummaryStatus, ApplicationError> {
        if let Some(job) = &record.status.job {
            record.status.job = Some(
                self.processing_jobs()?
                    .owned_job(OWNER.into(), job.id.clone())
                    .await?,
            );
        }
        Ok(record.status)
    }
    pub(crate) async fn guard_manual_media_summary(
        self: &Arc<Self>,
        owner: &str,
        child: &str,
        origin: Option<&ManualMediaChildOrigin>,
    ) -> Result<Option<ManualMediaGuard>, ApplicationError> {
        let Some(origin) = origin else {
            return Ok(None);
        };
        if owner != OWNER || child != origin.child_request_id {
            return Err(review("Manual summary child identity changed"));
        }
        let record = journal::load(self, &origin.parent_request_id)
            .await?
            .ok_or_else(|| review("Manual summary parent missing"))?;
        if !matches!(
            record.status.state,
            ManualMediaSummaryState::SourceBound | ManualMediaSummaryState::Child
        ) || record.status.child_request_id != child
            || record.declaration.session_id != origin.session_id
            || record.source.as_ref() != Some(&origin.source)
        {
            return Err(review("Manual summary parent has no published source"));
        }
        if self
            .read_metadata(&origin.session_id)
            .await?
            .meta
            .created_at
            != origin.source.transcript.session_created_at
        {
            return Err(review("Manual summary session generation changed"));
        }
        let guard = if let Some(sha) = &origin.source.recording_sha256 {
            ManualMediaGuard::Audio {
                _guard: self
                    .store
                    .guard_processing_source(
                        &origin.session_id,
                        Some(ProcessingSourceBinding {
                            recording_sha256: sha.clone(),
                            transcript_revision: origin
                                .source
                                .transcript
                                .transcript_revision
                                .clone(),
                            transcript_present: true,
                        }),
                    )
                    .await
                    .map_err(|cause| storage_error(cause, false))?
                    .ok_or_else(|| review("Manual summary recording disappeared"))?,
            }
        } else {
            ManualMediaGuard::Transcript {
                _guard: self
                    .store
                    .guard_transcript_only_source(
                        &origin.session_id,
                        origin.source.transcript.clone(),
                    )
                    .await
                    .map_err(|cause| storage_error(cause, false))?
                    .ok_or_else(|| review("Manual summary transcript disappeared"))?,
            }
        };
        Ok(Some(guard))
    }
}
fn validate_declaration(
    declaration: &ManualMediaSummaryDeclaration,
) -> Result<(), ApplicationError> {
    uuid(&declaration.request_id)?;
    hypr_vault_read::paths::validate_session_id(&declaration.session_id).map_err(review)?;
    if declaration.fresh_session && declaration.kind != ManualMediaKind::AudioImport {
        return Err(error(
            ErrorCode::Validation,
            "Only audio import creates a fresh manual session",
        ));
    }
    Ok(())
}
fn root(context: &VaultContext) -> Result<PathBuf, ApplicationError> {
    if context.lease_pin().is_none() {
        return Err(review("Original Local vault ownership required"));
    }
    Ok(context
        .operations
        .as_ref()
        .ok_or_else(|| review("Local operation storage unavailable"))?
        .root
        .join("manual-media-summary"))
}
fn uuid(value: &str) -> Result<uuid::Uuid, ApplicationError> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil() && id.to_string() == value)
        .ok_or_else(|| error(ErrorCode::Validation, "Canonical request UUID required"))
}
fn review(cause: impl std::fmt::Display) -> ApplicationError {
    let mut cause = error(ErrorCode::Unavailable, &cause.to_string());
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    cause
}

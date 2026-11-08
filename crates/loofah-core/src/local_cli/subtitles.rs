use super::*;
use crate::transcription_jobs::ledger;
use hypr_vault_write::transcript_publication::{
    SubtitleSourcePreparation, SubtitleSourceSnapshot, TranscriptPublicationIdentity,
    TranscriptPublicationOutcome,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[path = "subtitles/journal.rs"]
mod journal;
#[cfg(test)]
#[path = "subtitles/tests.rs"]
mod tests;

const SOURCE_LIMIT: u64 = 32 * 1024 * 1024;
const RESULT_LIMIT: usize = 64 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    vault_id: String,
    request_id: String,
    session_id: String,
    filename: String,
    source_sha256: String,
    source_bytes: u64,
    transcript_revision: String,
    transcript_present: bool,
    session_created_at: String,
    transcript_id: String,
    created_at: String,
    started_at: f64,
    memo: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Parsed {
    intent_sha256: String,
    transcript: TranscriptWithData,
}

impl LocalCli {
    pub(super) async fn execute_subtitles(
        &self,
        request_id: String,
        operation: LocalCliOperation,
        artifact: Option<LocalCliArtifact>,
    ) -> Result<LocalCliOutput, ApplicationError> {
        let (request, import) = match operation {
            LocalCliOperation::ImportSubtitles {
                session_id,
                filename,
            } => {
                validate_filename(&filename)?;
                (request_id, Some((session_id, filename)))
            }
            LocalCliOperation::RecoverSubtitles { request_id } => (request_id, None),
            _ => unreachable!("subtitle dispatch is restricted"),
        };
        let request_uuid = canonical_uuid(&request)?;
        let _request = self.context.admit_session_lifecycle_request(
            "local-subtitles",
            "local-cli",
            request_uuid,
        )?;
        let root = self
            .context
            .operations
            .as_ref()
            .ok_or_else(|| {
                error(
                    ErrorCode::Unavailable,
                    "Local operation storage is unavailable",
                )
            })?
            .root
            .join("local-subtitles");
        let directory = root.join(&request);
        let publication_root = root.join("publications");
        let identity = TranscriptPublicationIdentity {
            vault_id: self.context.events.vault_id(),
            namespace: "local-subtitles".into(),
            request_id: request_uuid,
        };
        let path = directory.clone();
        let owner = self.context.clone();
        let mut intent = tokio::task::spawn_blocking(move || {
            let _owner = owner;
            journal::load(&path)
        })
        .await
        .map_err(unknown)??;
        if let Some(existing) = &intent {
            if existing.request_id != request || existing.vault_id != identity.vault_id {
                return Err(unknown("Subtitle request identity changed"));
            }
            if let Some((session, filename)) = &import {
                let source = artifact
                    .as_ref()
                    .ok_or_else(|| error(ErrorCode::Validation, "Subtitle source required"))?;
                if &existing.session_id != session
                    || &existing.filename != filename
                    || existing.source_sha256 != source.sha256
                    || existing.source_bytes != source.size_bytes
                {
                    return Err(error(
                        ErrorCode::Conflict,
                        "Original subtitle declaration changed",
                    ));
                }
            }
            if let Some(outcome) = self
                .context
                .store
                .completed_subtitle_publication(&publication_root, identity.clone())
                .await
                .map_err(|cause| storage_error(cause, true))?
            {
                let parsed = read_parsed(&self.context, &directory, existing).await?;
                return outcome_reply(existing, parsed.transcript.words.len(), outcome);
            }
        }
        let session = intent
            .as_ref()
            .map(|value| value.session_id.clone())
            .or_else(|| import.as_ref().map(|value| value.0.clone()))
            .ok_or_else(|| error(ErrorCode::NotFound, "Original subtitle request not found"))?;
        let _admission = self.context.try_session_lifecycle_change(&session)?;
        self.context.ensure_session_jobs_idle(&session).await?;
        self.context
            .ensure_capture_session_lifecycle_idle(&session)
            .await?;
        let host = self.context.local_lifecycle_host.get().ok_or_else(|| {
            error(
                ErrorCode::Unavailable,
                "Local physical ownership proof is unavailable",
            )
        })?;
        let _physical = host.admit(&self.context, &session).await?;
        if let Some(existing) = &intent
            && let Some(plan) = self
                .context
                .store
                .prepare_transcript_publication_status(&publication_root, identity.clone())
                .await
                .map_err(|cause| storage_error(cause, true))?
        {
            let parsed = read_parsed(&self.context, &directory, existing).await?;
            return outcome_reply(
                existing,
                parsed.transcript.words.len(),
                plan.commit()
                    .await
                    .map_err(|cause| storage_error(cause, true))?,
            );
        }
        let source_sha = intent
            .as_ref()
            .map(|value| value.source_sha256.clone())
            .or_else(|| artifact.as_ref().map(|value| value.sha256.clone()))
            .ok_or_else(|| unknown("Original subtitle source is unavailable"))?;
        let source = subtitle_source(&self.context, &session, source_sha.clone()).await?;
        if let Some(existing) = &intent {
            check_source(existing, &source)?;
        } else {
            let artifact = artifact
                .as_ref()
                .ok_or_else(|| unknown("Subtitle source is unavailable"))?;
            if artifact.size_bytes > SOURCE_LIMIT || !is_transcription_sha256(&artifact.sha256) {
                return Err(error(
                    ErrorCode::Validation,
                    "Subtitle input exceeds 32 MiB or has an invalid digest",
                ));
            }
            let now = chrono::Utc::now();
            intent = Some(Intent {
                version: 1,
                vault_id: identity.vault_id.clone(),
                request_id: request.clone(),
                session_id: session.clone(),
                filename: import.as_ref().unwrap().1.clone(),
                source_sha256: source_sha,
                source_bytes: artifact.size_bytes,
                transcript_revision: source.transcript_revision().into(),
                transcript_present: source.transcript_present(),
                session_created_at: source.session_created_at().into(),
                transcript_id: uuid::Uuid::new_v4().to_string(),
                created_at: now.to_rfc3339(),
                started_at: now.timestamp_millis() as f64,
                memo: crate::transcription_jobs::local_memo(&self.context, &session).await?,
            });
        }
        let intent = intent.unwrap();
        let intent_copy = intent.clone();
        let path = directory.clone();
        let owner = self.context.clone();
        let parsed = tokio::task::spawn_blocking(move || {
            let _owner = owner;
            journal::prepare(&path, &intent_copy, artifact)
        })
        .await
        .map_err(unknown)??;
        let words = parsed.transcript.words.len();
        let outcome = self
            .context
            .store
            .prepare_subtitle_publication(&publication_root, identity, source, parsed.transcript)
            .await
            .map_err(|cause| storage_error(cause, true))?
            .commit()
            .await
            .map_err(|cause| storage_error(cause, true))?;
        outcome_reply(&intent, words, outcome).map_err(|mut cause| {
            cause.request_id = Some(request);
            cause
        })
    }
}

async fn subtitle_source(
    context: &Arc<VaultContext>,
    session: &str,
    sha: String,
) -> Result<SubtitleSourceSnapshot, ApplicationError> {
    match context
        .store
        .prepare_subtitle_source(session, sha)
        .await
        .map_err(|cause| storage_error(cause, true))?
    {
        Some(SubtitleSourcePreparation::Ready(source)) => Ok(source),
        Some(SubtitleSourcePreparation::Busy) => Err(error(
            ErrorCode::Conflict,
            "Session has active recording or transcript work",
        )),
        None => Err(error(ErrorCode::NotFound, "Session not found")),
    }
}
fn check_source(intent: &Intent, source: &SubtitleSourceSnapshot) -> Result<(), ApplicationError> {
    if source.transcript_revision() != intent.transcript_revision
        || source.transcript_present() != intent.transcript_present
        || source.session_created_at() != intent.session_created_at
    {
        return Err(error(
            ErrorCode::Conflict,
            "Original subtitle target changed; retained result was not applied",
        ));
    }
    Ok(())
}
async fn read_parsed(
    context: &Arc<VaultContext>,
    directory: &Path,
    intent: &Intent,
) -> Result<Parsed, ApplicationError> {
    let owner = context.clone();
    let path = directory.to_owned();
    let intent = intent.clone();
    tokio::task::spawn_blocking(move || {
        let _owner = owner;
        journal::read_parsed(&path, &intent)
    })
    .await
    .map_err(unknown)?
}
fn outcome_reply(
    intent: &Intent,
    words: usize,
    outcome: TranscriptPublicationOutcome,
) -> Result<LocalCliOutput, ApplicationError> {
    let (transcript_id, revision) = match outcome {
        TranscriptPublicationOutcome::Published {
            transcript_id,
            revision,
        }
        | TranscriptPublicationOutcome::AlreadyCommitted {
            transcript_id,
            revision,
        } => (Some(transcript_id), Some(revision)),
        TranscriptPublicationOutcome::Empty => (None, None),
        TranscriptPublicationOutcome::Conflict => {
            return Err(error(
                ErrorCode::Conflict,
                "Original subtitle target changed; retained result was not applied",
            ));
        }
        TranscriptPublicationOutcome::Busy => {
            return Err(error(
                ErrorCode::Conflict,
                "Session has active recording or transcript work",
            ));
        }
        TranscriptPublicationOutcome::RequiresReview => {
            return Err(unknown(
                "Subtitle publication requires review; recover the original request",
            ));
        }
    };
    Ok(LocalCliOutput {
        reply: LocalCliReply::SubtitlesImported {
            session_id: intent.session_id.clone(),
            transcript_id,
            word_count: words,
            revision,
        },
        artifact: None,
    })
}
fn canonical_uuid(value: &str) -> Result<uuid::Uuid, ApplicationError> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil() && id.to_string() == value)
        .ok_or_else(|| error(ErrorCode::Validation, "Canonical request UUID required"))
}
fn validate_filename(value: &str) -> Result<(), ApplicationError> {
    let path = Path::new(value);
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if path.file_name().and_then(|s| s.to_str()) != Some(value)
        || !["srt", "vtt"].contains(&extension.as_str())
    {
        return Err(error(
            ErrorCode::Validation,
            "Subtitle source must be an SRT or VTT filename",
        ));
    }
    Ok(())
}
fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn unknown(cause: impl std::fmt::Display) -> ApplicationError {
    let mut cause = error(ErrorCode::Unavailable, &cause.to_string());
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    cause
}

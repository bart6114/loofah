use super::*;
use crate::media_jobs::retirement::{verify_retirement_request, verify_retirement_status};

impl MediaSubmission {
    pub fn has_verified_source_export(&self) -> bool {
        self.source_export.as_ref().is_some_and(|export| {
            self.declaration
                .operation
                .upload()
                .is_some_and(|(_, source)| source == &export.source)
                && media_declaration_sha256(&self.declaration)
                    .is_ok_and(|hash| hash == export.declaration_sha256)
        })
    }
}

impl MediaSubmissions {
    /// Called by the trusted host only after a verified, no-clobber source publication.
    /// This records the completed export, not continued existence of the user's copy.
    pub async fn observe_source_export(
        self: &Arc<Self>,
        submission: &MediaSubmission,
    ) -> Result<MediaSubmission> {
        let source = submission
            .declaration
            .operation
            .upload()
            .ok_or_else(|| invalid("This request has no uploaded source"))?
            .1
            .clone();
        let export = MediaSourceExportObservation {
            declaration_sha256: media_declaration_sha256(&submission.declaration)
                .map_err(invalid)?,
            source,
        };
        self.update_observation(submission, move |original| {
            if original.retirement.is_some() {
                return Err(failure(
                    ErrorCode::Conflict,
                    "Media retirement was already requested",
                ));
            }
            original.source_export = Some(export);
            Ok(())
        })
        .await
    }

    pub async fn prepare_retirement(
        self: &Arc<Self>,
        submission: &MediaSubmission,
        request: MediaRetirementRequest,
    ) -> Result<MediaSubmission> {
        verify_retirement_request(submission, &request)?;
        self.update_observation(submission, move |original| {
            verify_retirement_request(original, &request)?;
            if let Some(existing) = &original.retirement {
                if existing.request != request {
                    return Err(failure(
                        ErrorCode::Conflict,
                        "Keep the original saved media retirement decision",
                    ));
                }
            } else {
                if request.disposition == MediaSourceDisposition::Exported
                    && !original.has_verified_source_export()
                {
                    return Err(invalid("No verified original source export was recorded"));
                }
                original.retirement = Some(MediaRetirementReceipt {
                    request,
                    retired: false,
                });
            }
            Ok(())
        })
        .await
    }

    pub async fn observe_retirement(
        self: &Arc<Self>,
        submission: &MediaSubmission,
        status: &MediaRetirementStatus,
    ) -> Result<MediaSubmission> {
        verify_retirement_status(submission, status)?;
        let status = status.clone();
        self.update_observation(submission, move |original| {
            verify_retirement_status(original, &status)?;
            let intent = original
                .retirement
                .as_mut()
                .ok_or_else(|| invalid("Original media retirement receipt is missing"))?;
            if status.retirement.as_ref() != Some(&intent.request) {
                return Err(bad_reply("Media retirement acknowledgement changed"));
            }
            if status.state == MediaRetirementState::Retired && status.quota_released {
                intent.retired = true;
            }
            Ok(())
        })
        .await
    }

    async fn update_observation(
        self: &Arc<Self>,
        submission: &MediaSubmission,
        action: impl FnOnce(&mut MediaSubmission) -> Result<()> + Send + 'static,
    ) -> Result<MediaSubmission> {
        let submission = submission.clone();
        self.run(submission.scope.clone(), move |directory| {
            let _lock = lock(&directory.join(".lock"))?;
            let path = directory.join(format!("{}.json", submission.declaration.request_id));
            let mut original = read(&path)?.ok_or_else(damaged)?;
            if original.scope != submission.scope
                || original.declaration != submission.declaration
                || original.admission_revision != submission.admission_revision
            {
                return Err(damaged());
            }
            action(&mut original)?;
            validate_observations(&original)?;
            let bytes = serde_json::to_string(&original).map_err(io)?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(damaged());
            }
            check_file(&path)?;
            hypr_storage::fs::atomic_write(&path, &bytes).map_err(io)?;
            sync(directory)?;
            Ok(original)
        })
        .await
    }
}

pub(super) fn validate_observations(submission: &MediaSubmission) -> Result<()> {
    if submission.source_export.is_some() && !submission.has_verified_source_export() {
        return Err(damaged());
    }
    if let Some(intent) = &submission.retirement {
        verify_retirement_request(submission, &intent.request).map_err(|_| damaged())?;
        if intent.request.disposition == MediaSourceDisposition::Exported
            && !submission.has_verified_source_export()
        {
            return Err(damaged());
        }
    }
    Ok(())
}

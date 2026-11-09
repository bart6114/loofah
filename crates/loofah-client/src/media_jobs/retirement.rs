use super::*;

impl RemoteClient {
    pub fn media_retirement_capability(&self, cancel: &CancellationToken) -> Result<()> {
        self.media_jobs_capability(cancel)?;
        if !self
            .capabilities
            .features
            .iter()
            .any(|value| value == "media_retirement")
        {
            let mut cause = failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support explicit media retirement. Upgrade and reconnect.",
            );
            cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
            return Err(cause);
        }
        Ok(())
    }
}

impl MediaAuthority {
    pub async fn retirement_status(
        &self,
        submission: &MediaSubmission,
    ) -> Result<MediaRetirementStatus> {
        self.client.media_retirement_capability(&self.cancel)?;
        self.validate_submission(submission)?;
        let status = self
            .json(
                Method::GET,
                &["requests", &submission.declaration.request_id, "retirement"],
                None::<&()>,
                &uuid::Uuid::new_v4().to_string(),
                131_072,
            )
            .await?;
        verify_retirement_status(submission, &status)?;
        Ok(status)
    }

    pub async fn retire(&self, submission: &MediaSubmission) -> Result<MediaRetirementStatus> {
        self.client.media_retirement_capability(&self.cancel)?;
        self.validate_submission(submission)?;
        let intent = submission
            .retirement
            .as_ref()
            .ok_or_else(|| invalid("Persist the original retirement decision before dispatch"))?;
        verify_retirement_request(submission, &intent.request)?;
        if intent.request.disposition == MediaSourceDisposition::Exported
            && !submission.has_verified_source_export()
        {
            return Err(invalid(
                "Save the verified original source before retiring it as exported",
            ));
        }
        let status = self.retirement_status(submission).await?;
        if let Some(original) = &status.retirement {
            if original != &intent.request {
                return Err(bad_reply("Original media retirement decision changed"));
            }
            if status.state == MediaRetirementState::Retired {
                return Ok(status);
            }
        } else if status.state != MediaRetirementState::Eligible
            || !status
                .allowed_dispositions
                .contains(&intent.request.disposition)
        {
            return Err(failure(
                ErrorCode::Conflict,
                "The original media request is not eligible for this retirement decision",
            ));
        }
        let status = self
            .json_with_budget(
                Method::POST,
                &["requests", &submission.declaration.request_id, "retirement"],
                Some(&intent.request),
                &intent.request.retirement_id,
                131_072,
                MEDIA_RETIREMENT_TIMEOUT,
            )
            .await?;
        verify_retirement_status(submission, &status)
            .map_err(|error| outcome(error, &intent.request.retirement_id, true, true))?;
        if status.retirement.as_ref() != Some(&intent.request) {
            return Err(outcome(
                bad_reply("The server did not acknowledge the original media retirement decision"),
                &intent.request.retirement_id,
                true,
                true,
            ));
        }
        Ok(status)
    }
}

pub(super) fn verify_retirement_request(
    submission: &MediaSubmission,
    input: &MediaRetirementRequest,
) -> Result<()> {
    validate_media_retirement_request(input).map_err(invalid)?;
    if submission.admission_revision.as_deref() != Some(input.expected_revision.as_str())
        || media_declaration_sha256(&submission.declaration).map_err(invalid)?
            != input.expected_declaration_sha256
        || submission
            .declaration
            .operation
            .upload()
            .map(|(_, source)| source)
            != input.expected_source.as_ref()
    {
        return Err(failure(
            ErrorCode::Conflict,
            "Media retirement does not match the original device declaration",
        ));
    }
    Ok(())
}

pub(super) fn verify_retirement_status(
    submission: &MediaSubmission,
    status: &MediaRetirementStatus,
) -> Result<()> {
    if status.request_id != submission.declaration.request_id
        || !valid_revision(&status.revision)
        || submission
            .admission_revision
            .as_ref()
            .is_some_and(|revision| revision != &status.revision)
        || status.declaration_sha256
            != media_declaration_sha256(&submission.declaration).map_err(invalid)?
        || status.source.as_ref()
            != submission
                .declaration
                .operation
                .upload()
                .map(|(_, source)| source)
        || status
            .reason
            .as_ref()
            .is_some_and(|reason| reason.len() > 4096)
        || (status.state == MediaRetirementState::Retired) != status.quota_released
        || status.allowed_dispositions.len() > 3
        || status
            .allowed_dispositions
            .iter()
            .enumerate()
            .any(|(index, choice)| status.allowed_dispositions[..index].contains(choice))
        || (status.state != MediaRetirementState::Eligible
            && !status.allowed_dispositions.is_empty())
        || (matches!(
            status.state,
            MediaRetirementState::Retiring | MediaRetirementState::Retired
        ) && status.retirement.is_none())
    {
        return Err(bad_reply("Original media retirement status changed"));
    }
    if let Some(input) = &status.retirement {
        let mut bound = submission.clone();
        bound.admission_revision = Some(status.revision.clone());
        verify_retirement_request(&bound, input)?;
        if submission
            .retirement
            .as_ref()
            .is_some_and(|original| original.request != *input)
        {
            return Err(bad_reply("Original media retirement decision changed"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_media_retirement_uses_five_minute_budget_without_extending_ordinary_requests() {
        let client = reqwest::Client::new();
        let ordinary = client
            .get("https://example.test/status")
            .timeout(MEDIA_REQUEST_TIMEOUT)
            .build()
            .unwrap();
        let retirement = client
            .post("https://example.test/retirement")
            .timeout(MEDIA_RETIREMENT_TIMEOUT)
            .build()
            .unwrap();
        assert_eq!(ordinary.timeout(), Some(&Duration::from_secs(35)));
        assert_eq!(retirement.timeout(), Some(&Duration::from_secs(300)));
    }
}

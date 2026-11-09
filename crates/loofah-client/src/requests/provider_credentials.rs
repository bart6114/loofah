use super::*;

const RESPONSE_LIMIT: usize = 16 * 1024;

pub(super) fn is_operation(operation: &RemoteOperation) -> bool {
    matches!(
        operation,
        RemoteOperation::ProviderCredentialStatus { .. }
            | RemoteOperation::SaveProviderCredential { .. }
    )
}
pub(super) fn is_write(operation: &RemoteOperation) -> bool {
    matches!(operation, RemoteOperation::SaveProviderCredential { .. })
}

fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}

pub(super) fn validate_operation(operation: &RemoteOperation, request_id: &str) -> Result<()> {
    let (vault, provider, input) = match operation {
        RemoteOperation::ProviderCredentialStatus {
            vault_id,
            provider_id,
        } => (vault_id, provider_id, None),
        RemoteOperation::SaveProviderCredential {
            vault_id,
            provider_id,
            input,
        } => (vault_id, provider_id, Some(input)),
        _ => return Ok(()),
    };
    if !uuid(vault) || !uuid(request_id) {
        return Err(invalid("Canonical vault and request UUIDs are required"));
    }
    if !matches!(
        provider.as_str(),
        "openai"
            | "anthropic"
            | "google_generative_ai"
            | "openrouter"
            | "mistral"
            | "ollama"
            | "lmstudio"
            | "azure_openai"
            | "azure_ai"
            | "cloudflare_workers_ai"
            | "custom"
    ) {
        return Err(invalid("Invalid provider identifier"));
    }
    if let Some(input) = input {
        if !uuid(&input.expected_revision) {
            return Err(invalid("A canonical credential revision is required"));
        }
        if input.api_key.as_ref().is_some_and(|key| {
            key.is_empty()
                || key.len() > 16 * 1024
                || key.trim() != key
                || !key.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        }) {
            return Err(invalid("Invalid provider credential"));
        }
    }
    Ok(())
}

fn malformed() -> ApplicationError {
    failure(
        ErrorCode::IncompatibleVersion,
        "Invalid provider credential response",
    )
}

fn validate_status(status: &ProviderCredentialStatus, provider: &str) -> Result<()> {
    if status.provider_id != provider
        || !uuid(&status.revision)
        || (status.source == ProviderCredentialSource::Vault && !status.endpoint_matches)
    {
        return Err(malformed());
    }
    Ok(())
}

pub(super) async fn decode(
    response: reqwest::Response,
    operation: &RemoteOperation,
) -> Result<RemoteReply> {
    match operation {
        RemoteOperation::ProviderCredentialStatus { provider_id, .. } => {
            let status: ProviderCredentialStatus = json_body(response, RESPONSE_LIMIT).await?;
            validate_status(&status, provider_id)?;
            Ok(RemoteReply::ProviderCredentialStatus(status))
        }
        RemoteOperation::SaveProviderCredential {
            provider_id, input, ..
        } => {
            let outcome: ProviderCredentialSaveOutcome =
                json_body(response, RESPONSE_LIMIT).await?;
            let (ProviderCredentialSaveOutcome::Saved(status)
            | ProviderCredentialSaveOutcome::Conflict(status)) = &outcome;
            validate_status(status, provider_id)?;
            if status.revision == input.expected_revision {
                return Err(malformed());
            }
            if matches!(outcome, ProviderCredentialSaveOutcome::Saved(_))
                && (!status.endpoint_matches
                    || (input.api_key.is_some()
                        != (status.source == ProviderCredentialSource::Vault)))
            {
                return Err(malformed());
            }
            Ok(RemoteReply::ProviderCredentialSave(outcome))
        }
        _ => Err(malformed()),
    }
}

pub(super) async fn decode_error(response: reqwest::Response) -> ApplicationError {
    match json_body::<ApplicationError>(response, RESPONSE_LIMIT).await {
        Ok(cause) => {
            let mut error = failure(cause.code, "Provider credential request failed");
            error.mutation_outcome = cause.mutation_outcome;
            error
        }
        Err(_) => failure(ErrorCode::Unavailable, "Provider credential request failed"),
    }
}

impl RemoteClient {
    pub async fn provider_credential_status(
        self: &Arc<Self>,
        vault_id: &str,
        provider_id: &str,
        request_id: &str,
        cancel: CancellationToken,
    ) -> Result<ProviderCredentialStatus> {
        match self
            .execute(
                RemoteOperation::ProviderCredentialStatus {
                    vault_id: vault_id.into(),
                    provider_id: provider_id.into(),
                },
                request_id,
                cancel,
            )
            .await?
        {
            RemoteReply::ProviderCredentialStatus(status) => Ok(status),
            _ => Err(malformed()),
        }
    }

    pub async fn save_provider_credential(
        self: &Arc<Self>,
        vault_id: &str,
        provider_id: &str,
        input: SaveProviderCredentialRequest,
        request_id: &str,
        cancel: CancellationToken,
    ) -> Result<ProviderCredentialSaveOutcome> {
        match self
            .execute(
                RemoteOperation::SaveProviderCredential {
                    vault_id: vault_id.into(),
                    provider_id: provider_id.into(),
                    input,
                },
                request_id,
                cancel,
            )
            .await?
        {
            RemoteReply::ProviderCredentialSave(outcome) => Ok(outcome),
            _ => {
                let mut error = malformed();
                error.request_id = Some(request_id.into());
                error.mutation_outcome = Some(MutationOutcome::Unknown);
                Err(error)
            }
        }
    }
}

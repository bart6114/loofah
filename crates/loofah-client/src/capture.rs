use std::{sync::Arc, time::Duration};

use hypr_loofah_protocol::{
    API_MAJOR, ApplicationError, CaptureChunkDescriptor, CaptureIngestRequest, CaptureIngestStatus,
    CapturePrefixReceipt, CaptureSeal, ErrorCode, MutationOutcome,
};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{RemoteClient, Result, cancelled, failure, invalid, json_body, unavailable};

impl RemoteClient {
    pub async fn prepare_capture_append(
        self: &Arc<Self>,
        prepared: &crate::CaptureAppendPreparation,
        cancel: CancellationToken,
    ) -> Result<hypr_loofah_protocol::CaptureAppendAdmission> {
        self.capture_capability(&cancel)?;
        if !self
            .capabilities
            .features
            .iter()
            .any(|feature| feature == "capture_append")
        {
            return Err(not_committed(failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support checked recording append",
            )));
        }
        let input = prepared.request();
        if input.processing.is_some()
            && !self
                .capabilities
                .features
                .iter()
                .any(|feature| feature == "capture_processing")
        {
            return Err(not_committed(failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support declared append processing",
            )));
        }
        if input.scope.server != self.address.as_str() {
            return Err(not_committed(invalid(
                "Append scope belongs to another server",
            )));
        }
        if let Some(value) = prepared.admission().await.map_err(not_committed)? {
            return Ok(value);
        }
        let request = self
            .http
            .post(self.endpoint(&["vaults", &input.scope.vault_id, "capture-append-admissions"])?)
            .json(input);
        let value = self
            .capture_request(request, &input.request_id, true, cancel)
            .await?;
        prepared.observe(value).await.map_err(unknown)
    }

    pub async fn begin_capture(
        self: &Arc<Self>,
        input: &CaptureIngestRequest,
        cancel: CancellationToken,
    ) -> Result<CaptureIngestStatus> {
        self.capture_capability(&cancel)?;
        if let hypr_loofah_protocol::CaptureTarget::AppendSession {
            session_id,
            admission,
        } = &input.identity.target
        {
            if !self
                .capabilities
                .features
                .iter()
                .any(|feature| feature == "capture_append")
            {
                return Err(not_committed(failure(
                    ErrorCode::IncompatibleVersion,
                    "This server does not support checked recording append",
                )));
            }
            hypr_loofah_protocol::validate_capture_append_admission(admission)
                .map_err(|message| not_committed(invalid(message)))?;
            if session_id != &admission.request.session_id
                || input.identity.scope != admission.request.scope
                || input.identity.capture_id != admission.request.capture_id
                || input.identity.request_id != admission.request.request_id
                || input.format != admission.baseline.format
            {
                return Err(not_committed(invalid(
                    "Append capture differs from its original admission",
                )));
            }
        }
        if input.identity.scope.server != self.address.as_str()
            || input.first_admitted_audio_observed_at_ms > 9_007_199_254_740_991
        {
            return Err(not_committed(invalid(
                "Capture scope or observation does not match this connection",
            )));
        }
        hypr_loofah_protocol::validate_capture_format(&input.format)
            .map_err(|message| not_committed(invalid(message)))?;
        let request = self
            .http
            .post(self.endpoint(&["vaults", &input.identity.scope.vault_id, "captures"])?)
            .json(input);
        let status = self
            .capture_request(request, &input.identity.request_id, true, cancel)
            .await?;
        let status = self
            .capture_status_checked(
                status,
                &input.identity.scope.vault_id,
                &input.identity.capture_id,
            )
            .map_err(unknown)?;
        if status.identity != input.identity
            || status.format != input.format
            || status.first_admitted_audio_observed_at_ms
                != input.first_admitted_audio_observed_at_ms
        {
            return Err(unknown(invalid_response()));
        }
        Ok(status)
    }

    pub async fn capture_status(
        self: &Arc<Self>,
        vault_id: &str,
        capture_id: &str,
        cancel: CancellationToken,
    ) -> Result<CaptureIngestStatus> {
        self.capture_capability(&cancel)?;
        let request = self
            .http
            .get(self.endpoint(&["vaults", vault_id, "captures", capture_id])?);
        let status = self
            .capture_request(request, &uuid::Uuid::new_v4().to_string(), false, cancel)
            .await?;
        self.capture_status_checked(status, vault_id, capture_id)
    }

    pub async fn append_capture_chunk(
        self: &Arc<Self>,
        vault_id: &str,
        capture_id: &str,
        descriptor: &CaptureChunkDescriptor,
        bytes: Vec<u8>,
        cancel: CancellationToken,
    ) -> Result<CapturePrefixReceipt> {
        self.capture_capability(&cancel)?;
        if bytes.len() > 1024 * 1024
            || bytes.is_empty()
            || descriptor.bytes as usize != bytes.len()
            || descriptor.sha256 != hash(&bytes)
            || descriptor.digest != hypr_loofah_protocol::capture_chunk_digest(descriptor)
        {
            return Err(not_committed(invalid("Invalid capture chunk declaration")));
        }
        let declaration = serde_json::to_string(descriptor)
            .map_err(|_| not_committed(invalid("Invalid capture chunk declaration")))?;
        if declaration.len() > 2048 {
            return Err(not_committed(invalid(
                "Capture declaration exceeds its size limit",
            )));
        }
        let sequence = descriptor.sequence.to_string();
        let request = self
            .http
            .put(self.endpoint(&[
                "vaults", vault_id, "captures", capture_id, "chunks", &sequence,
            ])?)
            .header("content-type", "application/octet-stream")
            .header("x-capture-chunk", declaration)
            .body(bytes);
        let receipt: CapturePrefixReceipt = self
            .capture_request(request, &uuid::Uuid::new_v4().to_string(), true, cancel)
            .await?;
        let expected = CapturePrefixReceipt {
            capture_fingerprint: descriptor.capture_fingerprint.clone(),
            chunk_count: descriptor
                .sequence
                .checked_add(1)
                .ok_or_else(|| unknown(invalid_response()))?,
            frames: descriptor
                .first_frame
                .checked_add(descriptor.frames as u64)
                .ok_or_else(|| unknown(invalid_response()))?,
            pcm_bytes: descriptor
                .pcm_offset
                .checked_add(descriptor.bytes as u64)
                .ok_or_else(|| unknown(invalid_response()))?,
            chain_digest: descriptor.digest.clone(),
        };
        if receipt != expected {
            return Err(unknown(invalid_response()));
        }
        Ok(receipt)
    }

    pub async fn seal_capture(
        self: &Arc<Self>,
        vault_id: &str,
        capture_id: &str,
        seal: &CaptureSeal,
        cancel: CancellationToken,
    ) -> Result<CaptureIngestStatus> {
        self.capture_capability(&cancel)?;
        let request = self
            .http
            .post(self.endpoint(&["vaults", vault_id, "captures", capture_id, "seal"])?)
            .json(seal);
        let status = self
            .capture_request(request, &uuid::Uuid::new_v4().to_string(), true, cancel)
            .await?;
        let status = self
            .capture_status_checked(status, vault_id, capture_id)
            .map_err(unknown)?;
        if status.seal.as_ref() != Some(seal) {
            return Err(unknown(invalid_response()));
        }
        Ok(status)
    }

    pub async fn publish_capture(
        self: &Arc<Self>,
        vault_id: &str,
        capture_id: &str,
        seal: &CaptureSeal,
        cancel: CancellationToken,
    ) -> Result<CaptureIngestStatus> {
        self.capture_capability(&cancel)?;
        if !self
            .capabilities
            .features
            .iter()
            .any(|feature| feature == "capture_publication")
        {
            return Err(not_committed(failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support capture publication. Upgrade the server, then reconnect.",
            )));
        }
        if seal.frames == 0
            || seal.chunk_count == 0
            || seal.pcm_bytes == 0
            || !valid_hash(&seal.capture_fingerprint)
            || !valid_hash(&seal.chain_digest)
            || !valid_hash(&seal.source_sha256)
        {
            return Err(not_committed(invalid(
                "A nonempty sealed capture is required",
            )));
        }
        let request = self
            .http
            .post(self.endpoint(&["vaults", vault_id, "captures", capture_id, "publish"])?)
            .json(seal);
        let status = self
            .capture_request(request, &uuid::Uuid::new_v4().to_string(), true, cancel)
            .await?;
        let status = self
            .capture_status_checked(status, vault_id, capture_id)
            .map_err(unknown)?;
        if status.seal.as_ref() != Some(seal)
            || status.publication.as_ref().is_none_or(|publication| {
                publication.source != *seal
                    || publication.canonical_audio_bytes > u64::from(u32::MAX)
            })
        {
            return Err(unknown(invalid_response()));
        }
        // The caller still verifies an independent canonical read before device cleanup.
        Ok(status)
    }

    pub(crate) fn capture_capability(&self, cancel: &CancellationToken) -> Result<()> {
        if cancel.is_cancelled() || self.stop.is_cancelled() {
            return Err(not_committed(cancelled()));
        }
        if !self
            .capabilities
            .features
            .iter()
            .any(|feature| feature == "capture_ingestion")
        {
            return Err(not_committed(failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support capture uploads. Upgrade the server, then reconnect.",
            )));
        }
        Ok(())
    }

    pub(crate) async fn capture_request<T: DeserializeOwned>(
        self: &Arc<Self>,
        request: reqwest::RequestBuilder,
        request_id: &str,
        mutation: bool,
        cancel: CancellationToken,
    ) -> Result<T> {
        let access = tokio::select! {
            _ = cancel.cancelled() => return Err(not_committed(cancelled())),
            _ = self.stop.cancelled() => return Err(not_committed(cancelled())),
            result = self.access_token() => result.map_err(not_committed)?,
        };
        let request = request
            .bearer_auth(&access)
            .header("x-loofah-api-major", API_MAJOR.to_string())
            .header("x-request-id", request_id)
            .timeout(Duration::from_secs(35));
        let result = async {
            let response = request.send().await.map_err(|_| unavailable())?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                self.expire_access(&access).await;
            }
            if !response.status().is_success() {
                return Err(json_body::<ApplicationError>(response, 131_072)
                    .await
                    .unwrap_or_else(|_| unavailable()));
            }
            json_body(response, 64 * 1024).await
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(cancelled()),
            _ = self.stop.cancelled() => Err(cancelled()),
            result = result => result,
        }
        .map_err(|mut cause| {
            cause.request_id.get_or_insert(request_id.to_owned());
            if mutation {
                cause
                    .mutation_outcome
                    .get_or_insert(MutationOutcome::Unknown);
            }
            cause
        })
    }

    pub(crate) fn capture_status_checked(
        &self,
        status: CaptureIngestStatus,
        vault_id: &str,
        capture_id: &str,
    ) -> Result<CaptureIngestStatus> {
        let prefix = &status.prefix;
        if status.identity.scope.server != self.address.as_str()
            || status.identity.scope.vault_id != vault_id
            || status.identity.capture_id != capture_id
            || status.first_admitted_audio_observed_at_ms > 9_007_199_254_740_991
            || hypr_loofah_protocol::validate_capture_format(&status.format).is_err()
            || prefix.capture_fingerprint
                != hypr_loofah_protocol::capture_fingerprint(&status.identity, &status.format)
            || prefix
                .frames
                .checked_mul(status.format.channels.len() as u64)
                .and_then(|samples| samples.checked_mul(4))
                != Some(prefix.pcm_bytes)
            || prefix.chunk_count > 131_072
            || prefix.chunk_count > prefix.frames
            || !valid_hash(&prefix.chain_digest)
            || (prefix.chunk_count == 0
                && (prefix.frames != 0
                    || prefix.pcm_bytes != 0
                    || prefix.chain_digest
                        != hypr_loofah_protocol::capture_chain_seed(&prefix.capture_fingerprint)))
            || status.seal.as_ref().is_some_and(|seal| {
                seal.capture_fingerprint != prefix.capture_fingerprint
                    || seal.chunk_count != prefix.chunk_count
                    || seal.frames != prefix.frames
                    || seal.pcm_bytes != prefix.pcm_bytes
                    || seal.chain_digest != prefix.chain_digest
                    || !valid_hash(&seal.source_sha256)
            })
            || status.publication.as_ref().is_some_and(|publication| {
                status.seal.as_ref() != Some(&publication.source)
                    || status.session_id.as_deref() != Some(&publication.session_id)
                    || !valid_hash(&publication.canonical_audio_sha256)
                    || publication.canonical_audio_bytes == 0
            })
            || status.retirement.as_ref().is_some_and(|retirement| {
                hypr_loofah_protocol::validate_capture_retirement_request(&retirement.request)
                    .is_err()
                    || retirement.request.identity != status.identity
                    || retirement.request.format != status.format
                    || status.seal.as_ref() != Some(&retirement.request.source)
                    || status.publication.as_ref() != Some(&retirement.request.publication)
                    || retirement.remaining_chunks > prefix.chunk_count
                    || (retirement.state == hypr_loofah_protocol::CaptureRetirementState::Retired
                        && retirement.remaining_chunks != 0)
            })
        {
            return Err(invalid_response());
        }
        Ok(status)
    }
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn invalid_response() -> ApplicationError {
    failure(
        ErrorCode::Unavailable,
        "The server returned an invalid capture receipt",
    )
}
fn not_committed(mut cause: ApplicationError) -> ApplicationError {
    cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
    cause
}
fn unknown(mut cause: ApplicationError) -> ApplicationError {
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    cause
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

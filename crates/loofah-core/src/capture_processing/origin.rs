use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureChildOrigin {
    capture_id: String,
    capture_fingerprint: String,
    intent_id: String,
    model_sha256: String,
    request_sha256: String,
}
impl CaptureChildOrigin {
    pub(crate) fn validate_request(
        &self,
        request: &impl Serialize,
    ) -> Result<(), ApplicationError> {
        if !valid_uuid(&self.capture_id)
            || !valid_uuid(&self.intent_id)
            || !is_transcription_sha256(&self.capture_fingerprint)
            || !is_transcription_sha256(&self.model_sha256)
            || self.request_sha256 != request_hash(request)?
        {
            return Err(storage("Invalid capture processing child origin"));
        }
        Ok(())
    }
    pub(super) fn capture_id(&self) -> &str {
        &self.capture_id
    }
    pub(super) fn matches(&self, record: &ledger::Record, request_id: &str) -> bool {
        if self.capture_id != record.identity.capture_id
            || self.capture_fingerprint != record.snapshot.capture_fingerprint
            || self.intent_id != record.snapshot.intent_id
            || self.model_sha256 != record.model_sha256
        {
            return false;
        }
        if request_id == child_request_id(record, "repair") {
            return record
                .repair_request
                .as_ref()
                .is_some_and(|request| self.validate_request(request).is_ok());
        }
        if request_id == child_request_id(record, "summary") {
            return record
                .summary_request
                .as_ref()
                .is_some_and(|request| self.validate_request(request).is_ok());
        }
        false
    }
}
pub(super) fn child_origin(
    record: &ledger::Record,
    stage: &str,
) -> Result<CaptureChildOrigin, ApplicationError> {
    let request_sha256 = match stage {
        "repair" => request_hash(
            record
                .repair_request
                .as_ref()
                .ok_or_else(|| conflict("Repair plan missing"))?,
        )?,
        "summary" => request_hash(
            record
                .summary_request
                .as_ref()
                .ok_or_else(|| conflict("Summary plan missing"))?,
        )?,
        _ => return Err(conflict("Unknown capture processing stage")),
    };
    Ok(CaptureChildOrigin {
        capture_id: record.identity.capture_id.clone(),
        capture_fingerprint: record.snapshot.capture_fingerprint.clone(),
        intent_id: record.snapshot.intent_id.clone(),
        model_sha256: record.model_sha256.clone(),
        request_sha256,
    })
}
fn request_hash(request: &impl Serialize) -> Result<String, ApplicationError> {
    Ok(crate::transcription_jobs::ledger::hash(
        &serde_json::to_vec(request).map_err(storage)?,
    ))
}

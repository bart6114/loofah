use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Admission {
    version: u32,
    vault_id: String,
    principal: String,
    pub declaration: MediaDeclaration,
    pub revision: String,
    pub phase: MediaPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<ApplicationError>,
}
impl Admission {
    pub fn new(
        vault_id: String,
        principal: String,
        declaration: MediaDeclaration,
        model_sha256: Option<String>,
    ) -> Self {
        let revision = uuid::Uuid::new_v4().to_string();
        let phase = if declaration.operation.upload().is_some() {
            MediaPhase::Receiving
        } else {
            MediaPhase::Ready
        };
        Self {
            version: if model_sha256.is_some() { 2 } else { 1 },
            vault_id,
            principal,
            declaration,
            revision,
            phase,
            refusal: None,
            model_sha256,
        }
    }
    fn validate(
        &self,
        dispatcher: &MediaDispatcher,
        request: &str,
    ) -> Result<(), ApplicationError> {
        validate_media_declaration(&self.declaration)
            .map_err(|_| conflict("Media admission requires review"))?;
        if self.model_sha256.as_ref().is_some_and(|hash| {
            !is_transcription_sha256(hash) || self.declaration.transcription.is_none()
        }) || !matches!(self.version, 1 | 2)
            || self.vault_id != dispatcher.context.events.vault_id()
            || self.principal != dispatcher.principal
            || self.declaration.request_id != request
            || !uuid::Uuid::parse_str(&self.revision)
                .is_ok_and(|id| !id.is_nil() && id.to_string() == self.revision)
        {
            return Err(conflict("Media admission identity requires review"));
        }
        Ok(())
    }
}
impl MediaDispatcher {
    pub(super) async fn load(&self, request: &str) -> Result<Option<Admission>, ApplicationError> {
        validate_request(request)?;
        let guard = self.journal_gate(request).await.read_owned().await;
        let path = self.root.join(format!("{request}.admission.json"));
        let root = self.root.clone();
        let pin = self.context.clone();
        let record = tokio::task::spawn_blocking(move || {
            let _pin = pin;
            let _guard = guard;
            if !ledger::existing_directory(&root)? || !ledger::check_file(&path)? {
                return Ok(None);
            }
            let record = serde_json::from_slice::<Admission>(&ledger::read(&path, 1024 * 1024)?)
                .map_err(|_| conflict("Media admission requires review"))?;
            Ok(Some(record))
        })
        .await
        .map_err(|_| unavailable("Media admission reader stopped"))??;
        if let Some(record) = &record {
            record.validate(self, request)?;
        }
        Ok(record)
    }
    pub(super) async fn save(
        &self,
        record: Admission,
        create: bool,
    ) -> Result<(), ApplicationError> {
        record.validate(self, &record.declaration.request_id)?;
        let guard = self
            .journal_gate(&record.declaration.request_id)
            .await
            .write_owned()
            .await;
        let path = self
            .root
            .join(format!("{}.admission.json", record.declaration.request_id));
        let root = self.root.clone();
        let pin = self.context.clone();
        tokio::task::spawn_blocking(move || {
            let _pin = pin;
            let _guard = guard;
            ledger::ensure_directory(&root)?;
            let bytes = ledger::encode_bounded_result(&record, 1024 * 1024)?;
            if create {
                ledger::write_new(&path, &bytes)
            } else {
                if !ledger::check_file(&path)? {
                    return Err(conflict("Original media admission disappeared"));
                }
                hypr_storage::fs::atomic_write(&path, std::str::from_utf8(&bytes).unwrap())
                    .map_err(|_| unavailable("Media admission checkpoint failed"))?;
                ledger::sync_parent(&path)
            }
        })
        .await
        .map_err(|_| unavailable("Media admission writer stopped"))?
    }
    pub(super) async fn inventory(&self) -> Result<Vec<Admission>, ApplicationError> {
        let root = self.root.clone();
        let limit = self.limits.requests;
        let pin = self.context.clone();
        let ids = tokio::task::spawn_blocking(move || {
            let _pin = pin;
            if !ledger::existing_directory(&root)? {
                return Ok(Vec::new());
            }
            let mut ids = Vec::new();
            for entry in
                std::fs::read_dir(&root).map_err(|_| unavailable("Media inventory unavailable"))?
            {
                let entry = entry.map_err(|_| unavailable("Media inventory unavailable"))?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                let Some(id) = name.strip_suffix(".admission.json") else {
                    continue;
                };
                validate_request(id)?;
                if ids.len() >= limit {
                    return Err(unavailable("Media inventory budget exhausted"));
                }
                ids.push(id.to_owned());
            }
            ids.sort();
            Ok(ids)
        })
        .await
        .map_err(|_| unavailable("Media inventory reader stopped"))??;
        let mut records = Vec::new();
        for id in ids {
            records.push(
                self.load(&id)
                    .await?
                    .ok_or_else(|| conflict("Media admission disappeared"))?,
            );
        }
        Ok(records)
    }
}
pub(super) fn validate_request(request: &str) -> Result<(), ApplicationError> {
    if !uuid::Uuid::parse_str(request).is_ok_and(|id| !id.is_nil() && id.to_string() == request) {
        return Err(error(
            ErrorCode::Validation,
            "Canonical media request UUID required",
        ));
    }
    Ok(())
}

pub(crate) fn sample_media_uploads(
    context: &VaultContext,
) -> crate::operational::Sample<crate::operational::MediaUploadCounts> {
    use crate::operational::{MediaUploadCounts, Sample};
    use sha2::{Digest, Sha256};
    let Some(operations) = &context.operations else {
        return Sample::Disabled;
    };
    let root = operations.root.join("member-media");
    let sample = || -> Result<Sample<MediaUploadCounts>, ApplicationError> {
        let mut counts = MediaUploadCounts::default();
        if !ledger::existing_directory(&root)? {
            return Ok(Sample::Available(counts));
        }
        let mut entries = 0_usize;
        let mut bytes = 0_usize;
        for member in std::fs::read_dir(&root)
            .map_err(|_| unavailable("Operational inventory unavailable"))?
        {
            entries += 1;
            if entries > 4096 {
                return Ok(Sample::Incomplete);
            }
            let member = member.map_err(|_| unavailable("Operational inventory unavailable"))?;
            let namespace = member.file_name();
            let namespace = namespace
                .to_str()
                .ok_or_else(|| unavailable("Operational inventory invalid"))?;
            if !is_transcription_sha256(namespace) || !ledger::existing_directory(&member.path())? {
                return Ok(Sample::Unavailable);
            }
            let directory = member.path().join("local-media");
            if !ledger::existing_directory(&directory)? {
                continue;
            }
            for entry in std::fs::read_dir(&directory)
                .map_err(|_| unavailable("Operational inventory unavailable"))?
            {
                entries += 1;
                if entries > 4096 {
                    return Ok(Sample::Incomplete);
                }
                let entry = entry.map_err(|_| unavailable("Operational inventory unavailable"))?;
                let name = entry.file_name();
                let Some(request) = name
                    .to_str()
                    .and_then(|name| name.strip_suffix(".admission.json"))
                else {
                    continue;
                };
                validate_request(request)?;
                let body = ledger::read(&entry.path(), 1024 * 1024)?;
                bytes = bytes.saturating_add(body.len());
                if bytes > 16 * 1024 * 1024 {
                    return Ok(Sample::Incomplete);
                }
                let record: Admission = serde_json::from_slice(&body)
                    .map_err(|_| unavailable("Operational inventory invalid"))?;
                validate_media_declaration(&record.declaration)
                    .map_err(|_| unavailable("Operational inventory invalid"))?;
                let expected_namespace = Sha256::digest(
                    serde_json::to_vec(&(&record.vault_id, &record.principal)).unwrap(),
                )
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
                if !matches!(record.version, 1 | 2)
                    || record.vault_id != context.events.vault_id()
                    || record.declaration.request_id != request
                    || namespace != expected_namespace
                {
                    return Ok(Sample::Unavailable);
                }
                match record.phase {
                    MediaPhase::Receiving => counts.receiving += 1,
                    MediaPhase::Ready => counts.ready += 1,
                    MediaPhase::Running => counts.dispatched += 1,
                    MediaPhase::Review => counts.review += 1,
                    _ => counts.terminal += 1,
                }
                counts.declared_bytes = counts.declared_bytes.saturating_add(
                    record
                        .declaration
                        .operation
                        .upload()
                        .map_or(0, |(_, source)| source.size_bytes),
                );
            }
        }
        Ok(Sample::Available(counts))
    };
    sample().unwrap_or(Sample::Unavailable)
}

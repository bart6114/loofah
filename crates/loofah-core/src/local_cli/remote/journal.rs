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
    pub refusal: Option<ApplicationError>,
}
impl Admission {
    pub fn new(vault_id: String, principal: String, declaration: MediaDeclaration) -> Self {
        let revision = uuid::Uuid::new_v4().to_string();
        let phase = if declaration.operation.upload().is_some() {
            MediaPhase::Receiving
        } else {
            MediaPhase::Ready
        };
        Self {
            version: 1,
            vault_id,
            principal,
            declaration,
            revision,
            phase,
            refusal: None,
        }
    }
    fn validate(
        &self,
        dispatcher: &MediaDispatcher,
        request: &str,
    ) -> Result<(), ApplicationError> {
        validate_media_declaration(&self.declaration)
            .map_err(|_| conflict("Media admission requires review"))?;
        if self.version != 1
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

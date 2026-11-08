use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, MutationOutcome, ProviderCredentialSaveOutcome,
    ProviderCredentialSource, ProviderCredentialStatus, SaveProviderCredentialRequest,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{VaultContext, vault::error};

const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct ProviderCredentialPolicy {
    pub provider_id: String,
    pub endpoint: String,
    pub operator_key: Option<String>,
}

pub struct ProviderCredentials {
    root: PathBuf,
    vault_id: String,
    policies: BTreeMap<String, ProviderCredentialPolicy>,
    lock: Mutex<()>,
    io: Arc<tokio::sync::Semaphore>,
    _lease: Arc<File>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    vault_id: String,
    records: BTreeMap<String, Record>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    revision: String,
    endpoint: String,
    policy_endpoint: String,
    api_key: Option<String>,
}

impl VaultContext {
    pub async fn configure_provider_credentials(
        self: &Arc<Self>,
        policies: Vec<ProviderCredentialPolicy>,
    ) -> Result<Arc<ProviderCredentials>, ApplicationError> {
        self.store.ensure_ready().map_err(|_| unavailable())?;
        let count = policies.len();
        let policies: BTreeMap<_, _> = policies
            .into_iter()
            .map(|policy| (policy.provider_id.clone(), policy))
            .collect();
        if policies.is_empty() || policies.len() > 16 || policies.len() != count {
            return Err(error(
                ErrorCode::Validation,
                "Invalid provider credential policy",
            ));
        }
        for (id, policy) in &policies {
            if crate::jobs::providers::connections::provider_definition(id).is_none() {
                return Err(error(
                    ErrorCode::Validation,
                    "Invalid provider credential policy",
                ));
            }
            fingerprint(&policy.endpoint)?;
            if let Some(key) = &policy.operator_key {
                validate_key(key)?;
            }
        }
        let context = self.clone();
        tokio::spawn(async move {
            let expected = policies.clone();
            let credentials = context
                .provider_credentials
                .get_or_try_init(|| {
                    let context = context.clone();
                    async move {
                        let root = context
                            .operations
                            .as_ref()
                            .ok_or_else(unavailable)?
                            .root
                            .join("provider-credentials");
                        let vault_id = context.events.vault_id();
                        let vault_base = context.store.vault_base().to_owned();
                        let lease = context.lease_pin().ok_or_else(unavailable)?;
                        tokio::task::spawn_blocking(move || {
                            let _context = context;
                            if root.starts_with(&vault_base) {
                                return Err(unavailable());
                            }
                            ensure_directory(&root)?;
                            let canonical =
                                std::fs::canonicalize(&root).map_err(|_| storage(false))?;
                            if canonical.starts_with(vault_base) {
                                return Err(unavailable());
                            }
                            let owner = Arc::new(ProviderCredentials {
                                root: canonical,
                                vault_id: vault_id.clone(),
                                policies,
                                lock: Mutex::new(()),
                                io: Arc::new(tokio::sync::Semaphore::new(4)),
                                _lease: lease,
                            });
                            let mut stored = if owner.path().exists() {
                                owner.read()?
                            } else {
                                Stored {
                                    version: 1,
                                    vault_id: vault_id.clone(),
                                    records: BTreeMap::new(),
                                }
                            };
                            if stored.vault_id != vault_id {
                                return Err(storage(false));
                            }
                            let mut changed = !owner.path().exists();
                            for (id, policy) in &owner.policies {
                                if !stored.records.contains_key(id) {
                                    stored.records.insert(
                                        id.clone(),
                                        Record {
                                            revision: uuid::Uuid::new_v4().to_string(),
                                            endpoint: fingerprint(&policy.endpoint)?,
                                            policy_endpoint: fingerprint(&policy.endpoint)?,
                                            api_key: None,
                                        },
                                    );
                                    changed = true;
                                } else {
                                    let endpoint = fingerprint(&policy.endpoint)?;
                                    let record =
                                        stored.records.get_mut(id).expect("existing provider");
                                    if record.policy_endpoint != endpoint {
                                        record.policy_endpoint = endpoint;
                                        record.revision = uuid::Uuid::new_v4().to_string();
                                        changed = true;
                                    }
                                }
                            }
                            if stored.records.len() > 64 {
                                return Err(storage(false));
                            }
                            if changed {
                                owner.persist(&stored)?;
                            }
                            Ok(owner)
                        })
                        .await
                        .map_err(|_| storage(false))?
                    }
                })
                .await?;
            if credentials.policies != expected {
                return Err(error(
                    ErrorCode::Conflict,
                    "Provider credential policy is already configured",
                ));
            }
            Ok(credentials.clone())
        })
        .await
        .map_err(|_| storage(false))?
    }

    pub fn provider_credentials(&self) -> Result<Arc<ProviderCredentials>, ApplicationError> {
        self.store.ensure_ready().map_err(|_| unavailable())?;
        self.provider_credentials
            .get()
            .cloned()
            .ok_or_else(unavailable)
    }
}

impl ProviderCredentials {
    pub async fn status(
        self: &Arc<Self>,
        provider_id: String,
    ) -> Result<ProviderCredentialStatus, ApplicationError> {
        self.run(move |owner| {
            let policy = owner.policy(&provider_id)?;
            let stored = owner.read()?;
            status(
                policy,
                stored
                    .records
                    .get(&provider_id)
                    .ok_or_else(|| storage(false))?,
            )
        })
        .await
    }

    pub async fn save(
        self: &Arc<Self>,
        provider_id: String,
        request: SaveProviderCredentialRequest,
    ) -> Result<ProviderCredentialSaveOutcome, ApplicationError> {
        if uuid::Uuid::parse_str(&request.expected_revision).is_err() {
            return Err(error(
                ErrorCode::Validation,
                "A credential revision is required",
            ));
        }
        if let Some(key) = &request.api_key {
            validate_key(key)?;
        }
        self.run(move |owner| {
            let policy = owner.policy(&provider_id)?;
            let mut stored = owner.read()?;
            let record = stored
                .records
                .get_mut(&provider_id)
                .ok_or_else(|| storage(false))?;
            if record.revision != request.expected_revision {
                return Ok(ProviderCredentialSaveOutcome::Conflict(status(
                    policy, record,
                )?));
            }
            record.revision = uuid::Uuid::new_v4().to_string();
            record.endpoint = fingerprint(&policy.endpoint)?;
            record.policy_endpoint = record.endpoint.clone();
            record.api_key = request.api_key;
            let result = status(policy, record)?;
            owner.persist(&stored)?;
            Ok(ProviderCredentialSaveOutcome::Saved(result))
        })
        .await
    }

    /// Only the processing host consumes this value; transport replies use status().
    pub async fn resolve(
        self: &Arc<Self>,
        provider_id: String,
    ) -> Result<Option<String>, ApplicationError> {
        self.run(move |owner| {
            let policy = owner.policy(&provider_id)?;
            let stored = owner.read()?;
            let record = stored
                .records
                .get(&provider_id)
                .ok_or_else(|| storage(false))?;
            if record.endpoint == fingerprint(&policy.endpoint)? {
                if let Some(key) = &record.api_key {
                    return Ok(Some(key.clone()));
                }
            }
            Ok(policy.operator_key.clone())
        })
        .await
    }

    async fn run<T: Send + 'static>(
        self: &Arc<Self>,
        operation: impl FnOnce(&Self) -> Result<T, ApplicationError> + Send + 'static,
    ) -> Result<T, ApplicationError> {
        let permit = self
            .io
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable())?;
        let owner = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _lock = owner.lock.lock().map_err(|_| storage(false))?;
            operation(&owner)
        })
        .await
        .map_err(|_| storage(false))?
    }

    fn policy(&self, id: &str) -> Result<&ProviderCredentialPolicy, ApplicationError> {
        self.policies.get(id).ok_or_else(|| {
            error(
                ErrorCode::Validation,
                "This provider is not permitted by this host",
            )
        })
    }

    fn path(&self) -> PathBuf {
        self.root.join("credentials.json")
    }

    fn read(&self) -> Result<Stored, ApplicationError> {
        ensure_directory(&self.root)?;
        let path = self.path();
        check_file(&path)?;
        let file = File::open(path).map_err(|_| storage(false))?;
        if file.metadata().map_err(|_| storage(false))?.len() > MAX_BYTES {
            return Err(storage(false));
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| storage(false))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(storage(false));
        }
        let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| storage(false))?;
        if stored.version != 1
            || stored.vault_id != self.vault_id
            || stored.records.len() > 64
            || stored.records.values().any(|record| {
                uuid::Uuid::parse_str(&record.revision).is_err()
                    || record.endpoint.len() != 64
                    || record.policy_endpoint.len() != 64
                    || !record
                        .policy_endpoint
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                    || !record.endpoint.bytes().all(|byte| byte.is_ascii_hexdigit())
                    || record
                        .api_key
                        .as_ref()
                        .is_some_and(|key| validate_key(key).is_err())
            })
        {
            return Err(storage(false));
        }
        Ok(stored)
    }

    fn persist(&self, stored: &Stored) -> Result<(), ApplicationError> {
        ensure_directory(&self.root)?;
        let path = self.path();
        if std::fs::symlink_metadata(&path).is_ok() {
            check_file(&path)?;
        }
        let content = serde_json::to_string(stored).map_err(|_| storage(true))?;
        if content.len() as u64 > MAX_BYTES {
            return Err(storage(true));
        }
        hypr_storage::fs::atomic_write(&path, &content).map_err(|_| storage(true))?;
        #[cfg(unix)]
        File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| storage(true))?;
        Ok(())
    }
}

fn status(
    policy: &ProviderCredentialPolicy,
    record: &Record,
) -> Result<ProviderCredentialStatus, ApplicationError> {
    let endpoint_matches =
        record.api_key.is_none() || record.endpoint == fingerprint(&policy.endpoint)?;
    let source = if endpoint_matches && record.api_key.is_some() {
        ProviderCredentialSource::Vault
    } else if policy.operator_key.is_some() {
        ProviderCredentialSource::Operator
    } else {
        ProviderCredentialSource::Missing
    };
    Ok(ProviderCredentialStatus {
        provider_id: policy.provider_id.clone(),
        revision: record.revision.clone(),
        source,
        endpoint_matches,
    })
}

fn fingerprint(endpoint: &str) -> Result<String, ApplicationError> {
    let endpoint = url::Url::parse(endpoint.trim_end_matches('/'))
        .map_err(|_| error(ErrorCode::Validation, "Invalid provider endpoint"))?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(error(ErrorCode::Validation, "Invalid provider endpoint"));
    }
    Ok(Sha256::digest(endpoint.as_str())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn validate_key(key: &str) -> Result<(), ApplicationError> {
    if key.is_empty()
        || key.len() > 16 * 1024
        || key.trim() != key
        || !key.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(error(ErrorCode::Validation, "Invalid provider credential"));
    }
    Ok(())
}

fn ensure_directory(path: &Path) -> Result<(), ApplicationError> {
    crate::transcription_jobs::ledger::ensure_directory(path).map_err(|_| storage(false))
}

fn check_file(path: &Path) -> Result<(), ApplicationError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| storage(false))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(storage(false));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
            return Err(storage(false));
        }
    }
    Ok(())
}

fn unavailable() -> ApplicationError {
    error(
        ErrorCode::Unavailable,
        "Provider credentials are not available",
    )
}
fn storage(writing: bool) -> ApplicationError {
    let mut cause = error(
        ErrorCode::Unavailable,
        "Provider credential storage is unavailable",
    );
    if writing {
        cause.mutation_outcome = Some(MutationOutcome::Unknown);
    }
    cause
}

#[cfg(test)]
mod tests;

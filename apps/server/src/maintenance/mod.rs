//! Offline, whole-storage snapshots. These operations never initialize authentication or models.

mod adoption;
mod copy;
pub use adoption::{AdoptionReport, adopt_vault};
#[cfg(test)]
mod tests;

use std::{fmt, path::Path};

use hypr_storage::vault::{LeaseAttempt, VaultLease};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalog::{Catalog, validate_backup_catalog};
use copy::{Entry, Inventory};

type Result<T> = std::result::Result<T, MaintenanceError>;

#[derive(Debug)]
pub struct MaintenanceError(&'static str);

impl fmt::Display for MaintenanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for MaintenanceError {}
impl From<std::io::Error> for MaintenanceError {
    fn from(_: std::io::Error) -> Self {
        Self("Storage I/O failed; no existing destination was replaced")
    }
}
impl From<serde_json::Error> for MaintenanceError {
    fn from(_: serde_json::Error) -> Self {
        Self("Backup metadata is invalid or unsupported")
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    snapshot_id: String,
    application_version: String,
    secrets_required: bool,
    entries: Inventory,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretsManifest {
    schema_version: u32,
    snapshot_id: String,
    main_manifest_sha256: String,
    entries: Inventory,
}

/// Only non-secret identifiers and counts are returned to the operator.
#[derive(Debug, Serialize)]
pub struct Report {
    pub snapshot_id: String,
    pub entries: usize,
    pub secrets_required: bool,
}

/// The service must have exited, including all detached storage owners.
pub async fn backup(storage: &Path, output: &Path, secrets_output: &Path) -> Result<Report> {
    let storage = copy::existing_directory(storage)?;
    let output = copy::destination(output)?;
    let secrets_output = copy::destination(secrets_output)?;
    copy::disjoint(&[&storage, &output, &secrets_output])?;
    validate_backup_catalog(&storage.join("catalog.json"))
        .map_err(|_| MaintenanceError("A supported service catalog is required"))?;
    let catalog = Catalog::open(&storage).await.map_err(|_| {
        MaintenanceError("Service storage is invalid or still owned; stop the service first")
    })?;
    // The blocking owner keeps the original descriptors even if its caller is cancelled.
    tokio::task::spawn_blocking(move || {
        let _catalog = catalog;
        let mut leases = Vec::new();
        let vaults = storage.join("vaults");
        if vaults.exists() {
            copy::existing_directory(&vaults)?;
            for entry in std::fs::read_dir(&vaults)? {
                let entry = entry?;
                if entry.file_type()?.is_dir()
                    && entry.file_name().to_str().is_some_and(|name| {
                        uuid::Uuid::parse_str(name).is_ok_and(|id| id.to_string() == name)
                    })
                {
                    match VaultLease::try_acquire_existing(&entry.path())? {
                        LeaseAttempt::Acquired(lease) => leases.push(lease),
                        LeaseAttempt::Busy => {
                            return Err(MaintenanceError(
                                "A vault is still owned; stop every vault owner first",
                            ));
                        }
                    }
                }
            }
        }
        let entries = copy::inventory(&storage)?;
        let mut main = Inventory::new();
        let mut secrets = Inventory::new();
        for (path, entry) in &entries {
            if path == ".loofah-server.lock" {
                continue;
            }
            if secret_path(path) {
                if !private_mode(entry) {
                    return Err(MaintenanceError(
                        "Provider credential storage must be owner-only",
                    ));
                }
                secrets.insert(path.clone(), entry.clone());
                for parent in Path::new(path)
                    .ancestors()
                    .skip(1)
                    .filter(|p| !p.as_os_str().is_empty())
                {
                    let parent = copy::relative_name(parent)?;
                    secrets.insert(parent.clone(), entries[&parent].clone());
                }
            } else {
                main.insert(path.clone(), entry.clone());
            }
        }
        let snapshot_id = uuid::Uuid::new_v4().to_string();
        let manifest = Manifest {
            schema_version: 1,
            snapshot_id: snapshot_id.clone(),
            application_version: env!("LOOFAH_VERSION").into(),
            secrets_required: !secrets.is_empty(),
            entries: main,
        };
        let main_bytes = serde_json::to_vec(&manifest)?;
        let secret_manifest = SecretsManifest {
            schema_version: 1,
            snapshot_id,
            main_manifest_sha256: digest(&main_bytes),
            entries: secrets,
        };
        let main_stage = copy::stage(&output)?;
        let secret_stage = copy::stage(&secrets_output)?;
        copy::copy_inventory(&storage, &main_stage.join("data"), &manifest.entries, false)?;
        copy::copy_inventory(
            &storage,
            &secret_stage.join("data"),
            &secret_manifest.entries,
            false,
        )?;
        copy::write_private(&main_stage.join("manifest.json"), &main_bytes)?;
        copy::write_private(
            &secret_stage.join("manifest.json"),
            &serde_json::to_vec(&secret_manifest)?,
        )?;
        // Detect uncoordinated edits as well as partial copies before publishing either bundle.
        if copy::inventory(&storage)? != entries {
            return Err(MaintenanceError(
                "Storage changed during backup; keep all writers stopped",
            ));
        }
        verify_sync(&main_stage, Some(&secret_stage))?;
        copy::publish(&secret_stage, &secrets_output)?;
        copy::publish(&main_stage, &output)?;
        Ok(report(&manifest))
    })
    .await
    .map_err(|_| MaintenanceError("Backup worker failed"))?
}

pub async fn verify(snapshot: &Path, secrets: Option<&Path>) -> Result<Report> {
    let snapshot = snapshot.to_path_buf();
    let secrets = secrets.map(Path::to_path_buf);
    tokio::task::spawn_blocking(move || {
        let (main, _) = verify_sync(&snapshot, secrets.as_deref())?;
        Ok(report(&main))
    })
    .await
    .map_err(|_| MaintenanceError("Verification worker failed"))?
}

/// Restore is deliberately limited to an absent target. The caller switches service config after verification.
pub async fn restore(snapshot: &Path, secrets: Option<&Path>, target: &Path) -> Result<Report> {
    let snapshot = copy::existing_directory(snapshot)?;
    let secrets = secrets.map(copy::existing_directory).transpose()?;
    let target = copy::destination(target)?;
    let mut paths = vec![snapshot.as_path(), target.as_path()];
    if let Some(secrets) = &secrets {
        paths.push(secrets);
    }
    copy::disjoint(&paths)?;
    tokio::task::spawn_blocking(move || {
        let (main, secret) = verify_sync(&snapshot, secrets.as_deref())?;
        let stage = copy::stage(&target)?;
        copy::copy_inventory(&snapshot.join("data"), &stage, &main.entries, true)?;
        if let (Some(secrets), Some(secret)) = (&secrets, &secret) {
            copy::copy_inventory(&secrets.join("data"), &stage, &secret.entries, true)?;
        }
        copy::restore_directory_modes(&stage, &main.entries)?;
        if let Some(secret) = &secret {
            copy::restore_directory_modes(&stage, &secret.entries)?;
        }
        validate_backup_catalog(&stage.join("catalog.json"))
            .map_err(|_| MaintenanceError("Service catalog is invalid or from a future schema"))?;
        copy::publish(&stage, &target)?;
        Ok(report(&main))
    })
    .await
    .map_err(|_| MaintenanceError("Restore worker failed"))?
}

fn verify_sync(
    snapshot: &Path,
    secrets: Option<&Path>,
) -> Result<(Manifest, Option<SecretsManifest>)> {
    copy::check_bundle(snapshot)?;
    let main_bytes = copy::read_manifest(&snapshot.join("manifest.json"))?;
    let main: Manifest = serde_json::from_slice(&main_bytes)?;
    if main.schema_version != 1 || uuid::Uuid::parse_str(&main.snapshot_id).is_err() {
        return Err(MaintenanceError("Backup schema is unsupported"));
    }
    if main
        .entries
        .keys()
        .any(|path| secret_path(path) || path == ".loofah-server.lock")
    {
        return Err(MaintenanceError(
            "Main backup contains a private credential path or live service lock",
        ));
    }
    copy::verify_inventory(&snapshot.join("data"), &main.entries)?;
    validate_backup_catalog(&snapshot.join("data/catalog.json"))
        .map_err(|_| MaintenanceError("Service catalog is invalid or from a future schema"))?;
    let secret = match secrets {
        None if main.secrets_required => {
            return Err(MaintenanceError(
                "The matching private credentials bundle is required",
            ));
        }
        None => None,
        Some(path) => {
            copy::check_bundle(path)?;
            let secret: SecretsManifest =
                serde_json::from_slice(&copy::read_manifest(&path.join("manifest.json"))?)?;
            if secret.schema_version != 1
                || secret.snapshot_id != main.snapshot_id
                || secret.main_manifest_sha256 != digest(&main_bytes)
            {
                return Err(MaintenanceError(
                    "Private credentials bundle does not match this backup",
                ));
            }
            if secret.entries.is_empty() == main.secrets_required
                || secret.entries.iter().any(|(path, entry)| {
                    (secret_path(path) && !private_mode(entry))
                        || (!secret_path(path)
                            && (!matches!(entry, Entry::Directory { .. })
                                || !secret.entries.keys().any(|child| {
                                    secret_path(child) && child.starts_with(&format!("{path}/"))
                                })))
                })
            {
                return Err(MaintenanceError("Private credentials inventory is invalid"));
            }
            copy::verify_inventory(&path.join("data"), &secret.entries)?;
            Some(secret)
        }
    };
    Ok((main, secret))
}

fn secret_path(path: &str) -> bool {
    let mut parts = path.split('/');
    parts.next() == Some("runtime")
        && parts.next().is_some_and(|id| {
            uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id)
        })
        && parts.next() == Some("creates")
        && parts.next() == Some("provider-credentials")
}

fn private_mode(entry: &Entry) -> bool {
    let mode = match entry {
        Entry::Directory { mode } | Entry::File { mode, .. } => mode,
    };
    mode & 0o077 == 0
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn report(main: &Manifest) -> Report {
    Report {
        snapshot_id: main.snapshot_id.clone(),
        entries: main.entries.len(),
        secrets_required: main.secrets_required,
    }
}

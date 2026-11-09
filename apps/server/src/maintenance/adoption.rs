use super::{
    Catalog, Entry, Inventory, MaintenanceError, Result, copy, digest, validate_backup_catalog,
};
use hypr_storage::vault::{LeaseAttempt, VaultLease};
use serde::Serialize;
use std::{
    fs::{self, File, Metadata},
    path::Path,
};

#[cfg(test)]
mod tests;

#[derive(Debug, Serialize)]
pub struct AdoptionReport {
    pub vault_id: String,
    pub source_entries: usize,
    pub storage_entries: usize,
}

/// Creates a new complete storage root; neither original root is replaced or migrated.
pub async fn adopt_vault(
    storage: &Path,
    vault_id: &str,
    source: &Path,
    output: &Path,
) -> Result<AdoptionReport> {
    adopt(
        storage,
        vault_id,
        source,
        output,
        #[cfg(test)]
        None,
    )
    .await
}

async fn adopt(
    storage: &Path,
    vault_id: &str,
    source: &Path,
    output: &Path,
    #[cfg(test)] checkpoint: Option<Box<dyn FnOnce() + Send>>,
) -> Result<AdoptionReport> {
    if !uuid::Uuid::parse_str(vault_id).is_ok_and(|id| !id.is_nil() && id.to_string() == vault_id) {
        return Err(MaintenanceError("An exact catalog vault UUID is required"));
    }
    let storage = copy::existing_directory(storage)?;
    let source = copy::existing_directory(source)?;
    let output = copy::destination(output)?;
    copy::disjoint(&[&storage, &source, &output])?;
    let storage_identity = fs::symlink_metadata(&storage)?;
    validate_backup_catalog(&storage.join("catalog.json"))
        .map_err(|_| MaintenanceError("A supported existing service catalog is required"))?;
    let catalog = Catalog::open(&storage)
        .await
        .map_err(|_| MaintenanceError("Service storage is still owned; stop the service first"))?;
    catalog
        .validate_adoption_target(vault_id)
        .await
        .map_err(|_| {
            MaintenanceError(
                "Provision the intended active target vault online before stopping the service",
            )
        })?;
    let vault_id = vault_id.to_owned();
    tokio::task::spawn_blocking(move || {
        let _catalog = catalog;
        check_root(&storage, &storage_identity)?;
        let source_identity = fs::symlink_metadata(&source)?;
        let _source_lease = lease(&source)?;
        let vaults = storage.join("vaults");
        let mut leases = Vec::new();
        if vaults.exists() {
            copy::existing_directory(&vaults)?;
            for item in fs::read_dir(&vaults)? {
                let item = item?;
                if item.file_type()?.is_dir()
                    && item.file_name().to_str().is_some_and(|name| {
                        uuid::Uuid::parse_str(name).is_ok_and(|id| id.to_string() == name)
                    })
                {
                    leases.push(lease(&item.path())?);
                }
            }
        }
        let original = copy::inventory(&storage)?;
        let local = copy::inventory(&source)?;
        let target_prefix = format!("vaults/{vault_id}");
        let runtime_prefix = format!("runtime/{vault_id}/");
        let _runtime_leases = fresh_target(&storage, &original, &target_prefix, &runtime_prefix)?;
        #[cfg(test)]
        if let Some(checkpoint) = checkpoint {
            checkpoint();
        }
        let mut server: Inventory = original
            .iter()
            .filter(|(name, _)| {
                name.as_str() != ".loofah-server.lock"
                    && name.as_str() != target_prefix
                    && !name.starts_with(&format!("{target_prefix}/"))
            })
            .map(|(name, entry)| (name.clone(), entry.clone()))
            .collect();
        if !server.contains_key("vaults") {
            server.insert("vaults".into(), Entry::Directory { mode: 0o700 });
        }
        let source_entries: Inventory = local
            .iter()
            .filter(|(name, _)| name.as_str() != ".loofah-vault.lock")
            .map(|(name, entry)| (name.clone(), entry.clone()))
            .collect();
        let mut expected = server.clone();
        expected.insert(target_prefix.clone(), Entry::Directory { mode: 0o700 });
        for (name, entry) in &source_entries {
            expected.insert(format!("{target_prefix}/{name}"), entry.clone());
        }
        let stage = copy::stage(&output)?;
        copy::copy_inventory(&storage, &stage, &server, true)?;
        copy::copy_inventory(&source, &stage.join(&target_prefix), &source_entries, true)?;
        copy::restore_directory_modes(&stage, &expected)?;
        if copy::inventory(&stage)? != expected {
            return Err(MaintenanceError(
                "Adoption copy failed exact inventory verification",
            ));
        }
        check_root(&storage, &storage_identity)?;
        check_root(&source, &source_identity)?;
        if copy::inventory(&storage)? != original || copy::inventory(&source)? != local {
            return Err(MaintenanceError(
                "Original storage changed during adoption; keep all writers stopped",
            ));
        }
        validate_backup_catalog(&stage.join("catalog.json"))
            .map_err(|_| MaintenanceError("Copied catalog is invalid or unsupported"))?;
        copy::publish(&stage, &output)?;
        Ok(AdoptionReport {
            vault_id,
            source_entries: source_entries.len(),
            storage_entries: expected.len(),
        })
    })
    .await
    .map_err(|_| MaintenanceError("Adoption worker failed; original roots were not replaced"))?
}

fn lease(root: &Path) -> Result<VaultLease> {
    match VaultLease::try_acquire_existing(root)? {
        LeaseAttempt::Acquired(lease) => Ok(lease),
        LeaseAttempt::Busy => Err(MaintenanceError(
            "A vault is still owned; stop every Local and server vault owner first",
        )),
    }
}

fn fresh_target(
    storage: &Path,
    original: &Inventory,
    target: &str,
    runtime: &str,
) -> Result<Vec<File>> {
    let generated = hypr_vault_write::agents_doc::agents_doc_content();
    let mut locks = Vec::new();
    for (name, entry) in original {
        if name == target && !matches!(entry, Entry::Directory { .. }) {
            return Err(MaintenanceError("Target vault is not an empty directory"));
        }
        if let Some(relative) = name.strip_prefix(&format!("{target}/")) {
            let allowed = match (relative, entry) {
                ("sessions", Entry::Directory { .. }) => true,
                (".loofah-vault.lock", Entry::File { bytes: 0, .. }) => true,
                ("AGENTS.md", Entry::File { bytes, sha256, .. }) => {
                    *bytes == generated.len() as u64 && *sha256 == digest(generated.as_bytes())
                }
                _ => false,
            };
            if !allowed {
                return Err(MaintenanceError(
                    "Target vault is not fresh; canonical, settings, Trash or unknown content must not be replaced",
                ));
            }
        }
        if name == runtime.trim_end_matches('/') && !matches!(entry, Entry::Directory { .. }) {
            return Err(MaintenanceError("Target runtime root is not a directory"));
        }
        if let Some(relative) = name.strip_prefix(runtime) {
            match entry {
                Entry::Directory { .. }
                    if matches!(
                        relative,
                        "creates"
                            | "creates/jobs"
                            | "creates/transcription-jobs"
                            | "creates/captures"
                            | "creates/capture-live"
                            | "creates/capture-processing"
                    ) => {}
                Entry::File { bytes: 0, .. }
                    if matches!(
                        relative,
                        "creates/jobs/.lock"
                            | "creates/transcription-jobs/.lock"
                            | "creates/captures/.lock"
                            | "creates/capture-live/.lock"
                            | "creates/capture-processing/.lock"
                    ) =>
                {
                    let path = storage.join(name);
                    let before = fs::symlink_metadata(&path)?;
                    let file = File::options().read(true).write(true).open(&path)?;
                    check_file_identity(&before, &file.metadata()?)?;
                    check_file_identity(&before, &fs::symlink_metadata(&path)?)?;
                    file.try_lock().map_err(|_| {
                        MaintenanceError("Target runtime is still owned; wait for actual shutdown")
                    })?;
                    locks.push(file);
                }
                _ => {
                    return Err(MaintenanceError(
                        "Target runtime contains work or unknown state; provision a new empty vault",
                    ));
                }
            }
        }
    }
    Ok(locks)
}

fn check_root(path: &Path, before: &Metadata) -> Result<()> {
    let after = fs::symlink_metadata(path)?;
    if !before.is_dir() || !after.is_dir() {
        return Err(MaintenanceError("Original storage root changed"));
    }
    same_identity(before, &after)
}
fn check_file_identity(before: &Metadata, after: &Metadata) -> Result<()> {
    if !before.is_file() || !after.is_file() || before.len() != 0 || after.len() != 0 {
        return Err(MaintenanceError("Target runtime lock changed"));
    }
    same_identity(before, after)
}
fn same_identity(before: &Metadata, after: &Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
            return Err(MaintenanceError("Original filesystem identity changed"));
        }
    }
    #[cfg(not(unix))]
    let _ = (before, after);
    Ok(())
}

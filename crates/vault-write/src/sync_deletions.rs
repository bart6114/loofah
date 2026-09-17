use std::fs::{self, File};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeletionIntent {
    pub version: u32,
    pub vault: PathBuf,
    pub session: String,
    pub trash: PathBuf,
}

pub fn directory(vault: &Path) -> std::io::Result<PathBuf> {
    let vault = vault.canonicalize()?;
    let hash = Sha256::digest(vault.as_os_str().as_encoded_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let root = dirs::data_dir()
        .ok_or_else(|| std::io::Error::other("application data unavailable"))?
        .join("io.loofah.sync.staging")
        .join(hash);
    if root.starts_with(&vault) || vault.starts_with(&root) {
        return Err(std::io::Error::other(
            "sync state must be outside the vault",
        ));
    }
    Ok(root)
}

pub fn record(vault: &Path, session: &str, trash: &Path) -> std::io::Result<()> {
    hypr_vault_read::paths::validate_session_id(session).map_err(std::io::Error::other)?;
    let intent = DeletionIntent {
        version: 1,
        vault: vault.canonicalize()?,
        session: session.into(),
        trash: trash.canonicalize()?,
    };
    let root = directory(vault)?;
    if !root.join("connection.json").is_file() {
        return Ok(());
    }
    let directory = root.join("deletions");
    fs::create_dir_all(&directory)?;
    let mut temp = tempfile::NamedTempFile::new_in(&directory)?;
    serde_json::to_writer(&mut temp, &intent)?;
    temp.as_file().sync_all()?;
    temp.persist(directory.join(format!("{session}.json")))
        .map_err(|error| error.error)?;
    File::open(directory)?.sync_all()
}

pub fn read(vault: &Path, session: &str) -> std::io::Result<Option<DeletionIntent>> {
    hypr_vault_read::paths::validate_session_id(session).map_err(std::io::Error::other)?;
    let bytes = match fs::read(
        directory(vault)?
            .join("deletions")
            .join(format!("{session}.json")),
    ) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let intent: DeletionIntent = serde_json::from_slice(&bytes)?;
    if intent.version != 1
        || intent.vault != vault.canonicalize()?
        || intent.session != session
        || !intent.trash.starts_with(intent.vault.join(".trash"))
    {
        return Err(std::io::Error::other("invalid deletion intent"));
    }
    if !intent.trash.is_dir() {
        return Ok(None);
    }
    Ok(Some(intent))
}

pub fn clear(vault: &Path, session: &str) -> std::io::Result<()> {
    hypr_vault_read::paths::validate_session_id(session).map_err(std::io::Error::other)?;
    let directory = directory(vault)?.join("deletions");
    match fs::remove_file(directory.join(format!("{session}.json"))) {
        Ok(()) => File::open(directory)?.sync_all(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

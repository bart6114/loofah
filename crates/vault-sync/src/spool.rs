use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crypto::{EncryptedManifest, PreparedVersion, VaultKey};
use crate::snapshot::{FileDigest, checked_path, copy_digest};
use crate::{Error, Result};

const RECEIPT: &str = "upload.json";
const MAX_RECEIPT: u64 = 40 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    revision: Uuid,
    encrypted_manifest: Vec<u8>,
    objects: BTreeMap<Uuid, FileDigest>,
}

pub struct UploadSpool {
    directory: PathBuf,
    receipt: Receipt,
    manifest: EncryptedManifest,
}

impl UploadSpool {
    /// The receipt is published only after every immutable ciphertext file is durable.
    /// No plaintext paths or keys are written to this upload spool.
    pub fn persist(prepared: PreparedVersion, directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let directory = directory.canonicalize()?;
        let staging = tempfile::Builder::new()
            .prefix(".upload-")
            .tempdir_in(&directory)?;
        let mut objects = BTreeMap::new();
        let membership = prepared.manifest.membership();
        for (id, source) in prepared.new_objects {
            let expected = membership.get(&id).ok_or(Error::Authentication)?;
            let destination = staging.path().join(id.to_string());
            source.persist(&destination).map_err(|error| error.error)?;
            let mut input = File::open(&destination)?;
            if &copy_digest(&mut input, &mut std::io::sink())? != expected {
                return Err(Error::Authentication);
            }
            input.sync_all()?;
            objects.insert(id, expected.clone());
        }
        let receipt = Receipt {
            version: 1,
            revision: prepared.revision,
            encrypted_manifest: prepared.encrypted_manifest,
            objects,
        };
        let bytes = serde_json::to_vec(&receipt).map_err(|_| Error::RecoveryRequired)?;
        if bytes.len() as u64 > MAX_RECEIPT {
            return Err(Error::Invalid("upload receipt too large".into()));
        }
        let mut output = File::create_new(staging.path().join(RECEIPT))?;
        output.write_all(&bytes)?;
        output.sync_all()?;
        File::open(staging.path())?.sync_all()?;
        let destination = directory.join(receipt.revision.to_string());
        hypr_storage::fs::rename_no_replace(staging.path(), &destination)?;
        File::open(&directory)?.sync_all()?;
        Ok(Self {
            directory: destination,
            receipt,
            manifest: prepared.manifest,
        })
    }

    /// A lost or corrupt spool fails closed. The caller must prepare a fresh version,
    /// with fresh object identities, rather than regenerate any old stream bytes.
    pub fn reopen(directory: &Path, revision: Uuid, key: &VaultKey) -> Result<Self> {
        let directory = checked_path(directory, Path::new(&revision.to_string()))?;
        let receipt_path = checked_path(&directory, Path::new(RECEIPT))?;
        let metadata = fs::metadata(&receipt_path)?;
        if !metadata.is_file() || metadata.len() > MAX_RECEIPT {
            return Err(Error::RecoveryRequired);
        }
        let receipt: Receipt = serde_json::from_slice(&fs::read(receipt_path)?)
            .map_err(|_| Error::RecoveryRequired)?;
        if receipt.version != 1 || receipt.revision != revision {
            return Err(Error::RecoveryRequired);
        }
        let manifest = key.open_manifest(revision, &receipt.encrypted_manifest)?;
        let membership = manifest.membership();
        for (id, expected) in &receipt.objects {
            if membership.get(id) != Some(expected) {
                return Err(Error::Authentication);
            }
            let path = checked_path(&directory, Path::new(&id.to_string()))?;
            let mut input = File::open(path)?;
            if !input.metadata()?.is_file()
                || &copy_digest(&mut input, &mut std::io::sink())? != expected
            {
                return Err(Error::Authentication);
            }
        }
        Ok(Self {
            directory,
            receipt,
            manifest,
        })
    }

    pub fn revision(&self) -> Uuid {
        self.receipt.revision
    }

    pub fn manifest(&self) -> &EncryptedManifest {
        &self.manifest
    }

    pub fn encrypted_manifest(&self) -> &[u8] {
        &self.receipt.encrypted_manifest
    }

    pub fn pending_objects(&self) -> &BTreeMap<Uuid, FileDigest> {
        &self.receipt.objects
    }

    pub fn open_object(&self, id: Uuid) -> Result<File> {
        if !self.receipt.objects.contains_key(&id) {
            return Err(Error::Invalid("object is not in this upload".into()));
        }
        Ok(File::open(checked_path(
            &self.directory,
            Path::new(&id.to_string()),
        )?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{scope::Entity, snapshot::Snapshot};

    #[test]
    fn restart_reuses_exact_ciphertext_and_rejects_missing_or_corrupt_bytes() {
        let vault = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        fs::write(vault.path().join("people.json"), br#"{"people":[]}"#).unwrap();
        let snapshot = Snapshot::capture(vault.path(), Entity::People, state.path()).unwrap();
        let key = VaultKey::generate(Uuid::new_v4()).unwrap();
        let prepared = key.prepare_version(&snapshot, None, state.path()).unwrap();
        let revision = prepared.revision;
        let id = *prepared.new_objects.keys().next().unwrap();
        let original = fs::read(&prepared.new_objects[&id]).unwrap();
        let spool = UploadSpool::persist(prepared, state.path()).unwrap();
        let sealed = spool.encrypted_manifest().to_vec();
        drop(spool);
        let resumed = UploadSpool::reopen(state.path(), revision, &key).unwrap();
        assert_eq!(resumed.encrypted_manifest(), sealed);
        let mut retried = Vec::new();
        std::io::Read::read_to_end(&mut resumed.open_object(id).unwrap(), &mut retried).unwrap();
        assert_eq!(retried, original);
        let file = state.path().join(revision.to_string()).join(id.to_string());
        fs::write(&file, b"corrupt").unwrap();
        assert!(UploadSpool::reopen(state.path(), revision, &key).is_err());
        fs::remove_file(file).unwrap();
        assert!(UploadSpool::reopen(state.path(), revision, &key).is_err());
    }
}

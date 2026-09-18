use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use libsodium_rs::{crypto_aead::xchacha20poly1305 as aead, crypto_pwhash::argon2id};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::CredentialStore;
use crate::{Error, Result};

const MAX_BYTES: u64 = 1024 * 1024;
const OPS: u64 = 3;
const MEMORY: usize = 64 * 1024 * 1024;
const DOMAIN: &[u8] = b"loofah-credentials-v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    ops: u64,
    memory: usize,
    salt: Vec<u8>,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

pub struct EncryptedFile {
    directory: PathBuf,
    key: Zeroizing<Vec<u8>>,
    salt: Vec<u8>,
}

pub fn private_directory(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(Error::Invalid("credential path must be absolute".into()));
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::Invalid(
                    "credential paths cannot contain symlinks".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    check_metadata(&fs::symlink_metadata(path)?, true)
}

fn check_metadata(meta: &fs::Metadata, directory: bool) -> Result<()> {
    if meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o077 != 0
        || if directory {
            !meta.is_dir()
        } else {
            !meta.is_file() || meta.nlink() != 1
        }
    {
        return Err(Error::Invalid("credential storage must be owned by this user with owner-only permissions and no links".into()));
    }
    Ok(())
}

pub fn read_secret(path: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    check_metadata(&file.metadata()?, false)?;
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Error::Authentication);
    }
    Ok(bytes)
}

impl EncryptedFile {
    pub fn open(directory: &Path, unlock: &[u8], create: bool) -> Result<Self> {
        libsodium_rs::ensure_init().map_err(|_| Error::Authentication)?;
        if unlock.is_empty() || unlock.len() > 4096 {
            return Err(Error::NeedsUnlock);
        }
        private_directory(directory)?;
        let _lock = lock(directory)?;
        let path = directory.join("credentials.enc");
        let existing = match read_secret(&path) {
            Ok(bytes) => Some(parse(&bytes)?),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound && create => None,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::CredentialMissing);
            }
            Err(error) => return Err(error),
        };
        let salt = existing
            .as_ref()
            .map(|value| value.salt.clone())
            .unwrap_or_else(|| libsodium_rs::random::bytes(argon2id::SALTBYTES));
        let key = Zeroizing::new(
            argon2id::pwhash(aead::KEYBYTES, unlock, &salt, OPS, MEMORY)
                .map_err(|_| Error::Authentication)?,
        );
        let store = Self {
            directory: directory.to_owned(),
            key,
            salt,
        };
        if let Some(envelope) = existing {
            store.decrypt(envelope)?;
        } else {
            store.save(&BTreeMap::new())?;
        }
        Ok(store)
    }

    fn decrypt(&self, value: Envelope) -> Result<BTreeMap<String, Zeroizing<Vec<u8>>>> {
        if value.salt != self.salt {
            return Err(Error::Authentication);
        }
        let nonce = aead::Nonce::try_from_slice(&value.nonce).map_err(|_| Error::Authentication)?;
        let key = aead::Key::from_bytes(&self.key).map_err(|_| Error::Authentication)?;
        let bytes = Zeroizing::new(
            aead::decrypt(&value.ciphertext, Some(DOMAIN), &nonce, &key)
                .map_err(|_| Error::NeedsUnlock)?,
        );
        let mut data: BTreeMap<String, Vec<u8>> =
            serde_json::from_slice(&bytes).map_err(|_| Error::Authentication)?;
        Ok(std::mem::take(&mut data)
            .into_iter()
            .map(|(name, bytes)| (name, Zeroizing::new(bytes)))
            .collect())
    }

    fn load(&self) -> Result<BTreeMap<String, Zeroizing<Vec<u8>>>> {
        self.decrypt(parse(&read_secret(
            &self.directory.join("credentials.enc"),
        )?)?)
    }

    fn save(&self, entries: &BTreeMap<String, Zeroizing<Vec<u8>>>) -> Result<()> {
        let data: BTreeMap<&str, &[u8]> = entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_slice()))
            .collect();
        let bytes = Zeroizing::new(serde_json::to_vec(&data).map_err(|_| Error::Authentication)?);
        if bytes.len() > (MAX_BYTES / 5) as usize {
            return Err(Error::Invalid("credential store is full".into()));
        }
        let nonce = aead::Nonce::generate();
        let key = aead::Key::from_bytes(&self.key).map_err(|_| Error::Authentication)?;
        let envelope = Envelope {
            version: 1,
            ops: OPS,
            memory: MEMORY,
            salt: self.salt.clone(),
            nonce: nonce.as_bytes().to_vec(),
            ciphertext: aead::encrypt(&bytes, Some(DOMAIN), &nonce, &key)
                .map_err(|_| Error::Authentication)?,
        };
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)?;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        serde_json::to_writer(&mut temporary, &envelope).map_err(|_| Error::Authentication)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(self.directory.join("credentials.enc"))
            .map_err(|error| Error::Io(error.error))?;
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}

fn parse(bytes: &[u8]) -> Result<Envelope> {
    let value: Envelope = serde_json::from_slice(bytes).map_err(|_| Error::Authentication)?;
    // Fixed versioned parameters prevent attacker-controlled KDF resource exhaustion.
    if value.version != 1
        || value.ops != OPS
        || value.memory != MEMORY
        || value.salt.len() != argon2id::SALTBYTES
        || value.nonce.len() != aead::NPUBBYTES
    {
        return Err(Error::Authentication);
    }
    Ok(value)
}

fn lock(directory: &Path) -> Result<File> {
    private_directory(directory)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(directory.join("credentials.lock"))?;
    check_metadata(&file.metadata()?, false)?;
    file.lock()?;
    Ok(file)
}

impl CredentialStore for EncryptedFile {
    fn read(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let _lock = lock(&self.directory)?;
        Ok(self.load()?.remove(name))
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        if name.len() > 256 || bytes.len() > 16384 {
            return Err(Error::Authentication);
        }
        let _lock = lock(&self.directory)?;
        let mut entries = self.load()?;
        entries.insert(name.into(), Zeroizing::new(bytes.to_vec()));
        self.save(&entries)
    }
    fn remove(&self, name: &str) -> Result<()> {
        let _lock = lock(&self.directory)?;
        let mut entries = self.load()?;
        entries.remove(name);
        self.save(&entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survives_restart_and_never_replaces_on_wrong_unlock_or_missing_file() {
        let directory = tempfile::tempdir().unwrap();
        let canonical = directory.path().canonicalize().unwrap().join("credentials");
        let store = EncryptedFile::open(&canonical, b"local unlock", true).unwrap();
        store.write("session:test", b"account token").unwrap();
        let before = fs::read(canonical.join("credentials.enc")).unwrap();
        assert!(!String::from_utf8_lossy(&before).contains("account token"));
        assert!(EncryptedFile::open(&canonical, b"wrong", true).is_err());
        assert_eq!(before, fs::read(canonical.join("credentials.enc")).unwrap());
        let reopened = EncryptedFile::open(&canonical, b"local unlock", false).unwrap();
        assert_eq!(
            reopened.read("session:test").unwrap().unwrap().as_slice(),
            b"account token"
        );
        store.write("second", b"new secret").unwrap();
        reopened.remove("session:test").unwrap();
        assert_eq!(
            store.read("second").unwrap().unwrap().as_slice(),
            b"new secret"
        );
        fs::remove_file(canonical.join("credentials.enc")).unwrap();
        assert!(EncryptedFile::open(&canonical, b"local unlock", false).is_err());
    }

    #[test]
    fn rejects_tampering_unsafe_permissions_and_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let canonical = directory.path().canonicalize().unwrap().join("credentials");
        let store = EncryptedFile::open(&canonical, b"local unlock", true).unwrap();
        let path = canonical.join("credentials.enc");
        let original = fs::read(&path).unwrap();
        let mut envelope: Envelope = serde_json::from_slice(&original).unwrap();
        envelope.ciphertext[0] ^= 1;
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(store.read("anything").is_err());
        envelope.memory = usize::MAX;
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(EncryptedFile::open(&canonical, b"local unlock", false).is_err());
        fs::write(&path, original).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.read("anything").is_err());
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/dev/null", &path).unwrap();
        assert!(store.read("anything").is_err());
    }
}

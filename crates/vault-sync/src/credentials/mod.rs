use crate::{Error, Result, crypto::VaultKey};
use libsodium_rs::crypto_sign::KeyPair;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

pub mod file;
#[cfg(target_os = "linux")]
pub mod secret_service;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedIdentity {
    version: u32,
    device_id: Uuid,
    recovery_kit: Vec<u8>,
    device_seed: Vec<u8>,
}

impl Drop for SavedIdentity {
    fn drop(&mut self) {
        self.recovery_kit.zeroize();
        self.device_seed.zeroize();
    }
}

pub type DeviceIdentity = (VaultKey, Uuid, KeyPair);

pub trait CredentialStore: Send + Sync {
    fn read(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>>;
    fn write(&self, name: &str, bytes: &[u8]) -> Result<()>;
    fn remove(&self, name: &str) -> Result<()>;

    fn save_session(&self, account: Uuid, credential: &[u8]) -> Result<()> {
        if credential.is_empty() || credential.len() > 4096 {
            return Err(Error::Authentication);
        }
        self.write(&format!("session:{account}"), credential)
    }
    fn session(&self, account: Uuid) -> Result<Zeroizing<Vec<u8>>> {
        self.read(&format!("session:{account}"))?
            .ok_or(Error::CredentialMissing)
    }
    fn forget_session(&self, account: Uuid) -> Result<()> {
        self.remove(&format!("session:{account}"))
    }
    /// The UI must ask the user to import their saved recovery kit before calling this.
    /// Store root and device identity together before acknowledging enrollment.
    fn save_confirmed_identity(
        &self,
        root: &VaultKey,
        device_id: Uuid,
        device: &KeyPair,
        confirmed_kit: &[u8],
    ) -> Result<()> {
        if !libsodium_rs::utils::memcmp(&root.recovery_kit(), confirmed_kit) {
            return Err(Error::Authentication);
        }
        let seed = Zeroizing::new(
            libsodium_rs::crypto_sign::secret_key_to_seed(&device.secret_key)
                .map_err(|_| Error::Authentication)?,
        );
        let saved = SavedIdentity {
            version: 1,
            device_id,
            recovery_kit: confirmed_kit.to_vec(),
            device_seed: seed.to_vec(),
        };
        let bytes = Zeroizing::new(serde_json::to_vec(&saved).map_err(|_| Error::Authentication)?);
        self.write(&format!("vault:{}", root.vault()), &bytes)
    }

    fn identity(&self, vault: Uuid) -> Result<DeviceIdentity> {
        let bytes = Zeroizing::new(
            self.read(&format!("vault:{vault}"))?
                .ok_or(Error::CredentialMissing)?,
        );
        let saved: SavedIdentity =
            serde_json::from_slice(&bytes).map_err(|_| Error::Authentication)?;
        if saved.version != 1 {
            return Err(Error::Authentication);
        }
        let root = VaultKey::from_recovery_kit(&saved.recovery_kit, vault)?;
        let device = KeyPair::from_seed(&saved.device_seed).map_err(|_| Error::Authentication)?;
        Ok((root, saved.device_id, device))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Keychain,
    SecretService,
    EncryptedFile,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    version: u32,
    backend: Backend,
    initialized: bool,
}

pub fn open(
    directory: &std::path::Path,
    environment: crate::remote::Environment,
    requested: Option<Backend>,
    unlock: Option<&[u8]>,
) -> Result<std::sync::Arc<dyn CredentialStore>> {
    use std::{fs, io::Write, os::unix::fs::OpenOptionsExt};
    if !directory.join("credential-backend.json").exists()
        && directory.join("connection.json").is_file()
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let meta = fs::symlink_metadata(directory)?;
        if meta.is_dir() && meta.uid() == rustix::process::geteuid().as_raw() {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
    }
    file::private_directory(directory)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("backend.lock"))?;
    lock.lock()?;
    let path = directory.join("credential-backend.json");
    let saved = match file::read_secret(&path) {
        Ok(bytes) => {
            Some(serde_json::from_slice::<Selection>(&bytes).map_err(|_| Error::Authentication)?)
        }
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if saved.as_ref().is_some_and(|value| value.version != 1) {
        return Err(Error::Authentication);
    }
    let legacy = directory.join("connection.json").exists();
    let mut backend = saved
        .as_ref()
        .map(|value| value.backend)
        .or(requested)
        .unwrap_or(if cfg!(target_os = "macos") {
            Backend::Keychain
        } else {
            Backend::SecretService
        });
    if saved.is_none() && requested.is_none() && !legacy {
        #[cfg(target_os = "macos")]
        let available = crate::keychain::Keychain::new(environment)
            .read("backend-probe")
            .is_ok();
        #[cfg(target_os = "linux")]
        let available = secret_service::SecretService::new(environment)
            .read("backend-probe")
            .is_ok();
        if !available {
            backend = Backend::EncryptedFile;
        }
    }
    if requested.is_some_and(|requested| requested != backend)
        || (saved.is_none() && legacy && backend != Backend::Keychain)
    {
        return Err(Error::Invalid("the existing credential backend cannot be changed during connection; unlock the selected store".into()));
    }
    // Persist the selection before creating secrets, so interrupted setup cannot pick a different backend.
    if saved.is_none() {
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(
            &serde_json::to_vec(&Selection {
                version: 1,
                backend,
                initialized: false,
            })
            .map_err(|_| Error::Authentication)?,
        )?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(&path)
            .map_err(|error| Error::Io(error.error))?;
        fs::File::open(directory)?.sync_all()?;
    }
    let store: std::sync::Arc<dyn CredentialStore> = match backend {
        #[cfg(target_os = "macos")]
        Backend::Keychain => Ok(
            std::sync::Arc::new(crate::keychain::Keychain::new(environment))
                as std::sync::Arc<dyn CredentialStore>,
        ),
        #[cfg(target_os = "linux")]
        Backend::SecretService => Ok(std::sync::Arc::new(secret_service::SecretService::new(
            environment,
        )) as std::sync::Arc<dyn CredentialStore>),
        Backend::EncryptedFile => Ok(std::sync::Arc::new(file::EncryptedFile::open(
            directory,
            unlock.ok_or(Error::NeedsUnlock)?,
            !saved.as_ref().is_some_and(|value| value.initialized),
        )?) as std::sync::Arc<dyn CredentialStore>),
        _ => Err(Error::Invalid(
            "selected credential backend is unavailable on this platform".into(),
        )),
    }?;
    if !saved.as_ref().is_some_and(|value| value.initialized) {
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(
            &serde_json::to_vec(&Selection {
                version: 1,
                backend,
                initialized: true,
            })
            .map_err(|_| Error::Authentication)?,
        )?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&path)
            .map_err(|error| Error::Io(error.error))?;
        fs::File::open(directory)?.sync_all()?;
    }
    Ok(store)
}

pub struct Locked;
impl CredentialStore for Locked {
    fn read(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        Err(Error::NeedsUnlock)
    }
    fn write(&self, _: &str, _: &[u8]) -> Result<()> {
        Err(Error::NeedsUnlock)
    }
    fn remove(&self, _: &str) -> Result<()> {
        Err(Error::NeedsUnlock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::Environment;

    #[test]
    fn backend_choice_survives_interrupted_setup_and_never_falls_back() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().canonicalize().unwrap().join("credentials");
        assert!(matches!(
            open(
                &directory,
                Environment::Staging,
                Some(Backend::EncryptedFile),
                None
            ),
            Err(Error::NeedsUnlock)
        ));
        let store = open(
            &directory,
            Environment::Staging,
            None,
            Some(b"local unlock"),
        )
        .unwrap();
        let vault = Uuid::new_v4();
        let root = VaultKey::generate(vault).unwrap();
        let device = KeyPair::generate().unwrap();
        let id = Uuid::new_v4();
        store.save_session(vault, b"session credential").unwrap();
        assert!(
            store
                .save_confirmed_identity(&root, id, &device, b"wrong kit")
                .is_err()
        );
        store
            .save_confirmed_identity(&root, id, &device, &root.recovery_kit())
            .unwrap();
        let restored = open(
            &directory,
            Environment::Staging,
            None,
            Some(b"local unlock"),
        )
        .unwrap();
        let (restored_root, restored_id, restored_pair) = restored.identity(vault).unwrap();
        assert_eq!(
            root.recovery_kit().as_slice(),
            restored_root.recovery_kit().as_slice()
        );
        assert_eq!(id, restored_id);
        assert_eq!(
            device.public_key.as_bytes(),
            restored_pair.public_key.as_bytes()
        );
        assert!(
            open(
                &directory,
                Environment::Staging,
                Some(Backend::Keychain),
                None
            )
            .is_err()
        );
        restored.forget_session(vault).unwrap();
        assert!(matches!(
            store.session(vault),
            Err(Error::CredentialMissing)
        ));
        assert!(store.identity(vault).is_ok());
        std::fs::remove_file(directory.join("credentials.enc")).unwrap();
        assert!(
            open(
                &directory,
                Environment::Staging,
                None,
                Some(b"local unlock")
            )
            .is_err()
        );
    }
}

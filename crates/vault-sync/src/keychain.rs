use libsodium_rs::crypto_sign::KeyPair;
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use crate::{Error, Result, crypto::VaultKey};

pub use crate::remote::Environment;

impl Environment {
    fn service(self) -> &'static str {
        match self {
            Self::Staging => "io.loofah.sync.staging.v1",
            Self::Production => "io.loofah.sync.production.v1",
        }
    }
}

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

pub struct Keychain {
    environment: Environment,
}

impl Keychain {
    pub fn new(environment: Environment) -> Self {
        Self { environment }
    }

    pub fn save_session(&self, account: Uuid, credential: &[u8]) -> Result<()> {
        if credential.is_empty() || credential.len() > 4096 {
            return Err(Error::Authentication);
        }
        set_generic_password(
            self.environment.service(),
            &format!("session:{account}"),
            credential,
        )
        .map_err(|_| Error::Authentication)
    }

    pub fn session(&self, account: Uuid) -> Result<Zeroizing<Vec<u8>>> {
        get_generic_password(self.environment.service(), &format!("session:{account}"))
            .map(Zeroizing::new)
            .map_err(|_| Error::Authentication)
    }

    pub fn forget_session(&self, account: Uuid) -> Result<()> {
        match delete_generic_password(self.environment.service(), &format!("session:{account}")) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == -25300 => Ok(()),
            Err(_) => Err(Error::Authentication),
        }
    }

    /// The UI must ask the user to import their saved recovery kit before calling this.
    /// Store root and device identity together before acknowledging enrollment.
    pub fn save_confirmed_identity(
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
        set_generic_password(
            self.environment.service(),
            &format!("vault:{}", root.vault()),
            &bytes,
        )
        .map_err(|_| Error::Authentication)
    }

    pub fn identity(&self, vault: Uuid) -> Result<(VaultKey, Uuid, KeyPair)> {
        let bytes = Zeroizing::new(
            get_generic_password(self.environment.service(), &format!("vault:{vault}"))
                .map_err(|_| Error::Authentication)?,
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

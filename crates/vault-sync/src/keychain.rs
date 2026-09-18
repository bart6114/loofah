pub use crate::remote::Environment;
use crate::{Error, Result, credentials::CredentialStore};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use zeroize::Zeroizing;

pub struct Keychain {
    environment: Environment,
}
impl Keychain {
    pub fn new(environment: Environment) -> Self {
        Self { environment }
    }
}
impl CredentialStore for Keychain {
    fn read(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        match get_generic_password(self.environment.credential_service(), name) {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(error) if error.code() == -25300 => Ok(None),
            Err(_) => Err(Error::NeedsUnlock),
        }
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        set_generic_password(self.environment.credential_service(), name, bytes)
            .map_err(|_| Error::NeedsUnlock)
    }
    fn remove(&self, name: &str) -> Result<()> {
        match delete_generic_password(self.environment.credential_service(), name) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == -25300 => Ok(()),
            Err(_) => Err(Error::NeedsUnlock),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an unlocked real macOS Keychain"]
    fn disposable_keychain_entry_roundtrip() {
        let store = Keychain::new(Environment::Staging);
        let name = format!("test:{}", uuid::Uuid::new_v4());
        store.write(&name, b"disposable-test-value").unwrap();
        let read = store.read(&name);
        let removed = store.remove(&name);
        assert_eq!(read.unwrap().unwrap().as_slice(), b"disposable-test-value");
        removed.unwrap();
        assert!(store.read(&name).unwrap().is_none());
    }
}

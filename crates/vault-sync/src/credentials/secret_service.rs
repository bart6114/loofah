use super::CredentialStore;
use crate::{Error, Result, remote::Environment};
use zeroize::Zeroizing;

pub struct SecretService {
    environment: Environment,
}
impl SecretService {
    pub fn new(environment: Environment) -> Self {
        Self { environment }
    }
    fn entry(&self, name: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(self.environment.credential_service(), name)
            .map_err(|_| Error::NeedsUnlock)
    }
}
impl CredentialStore for SecretService {
    fn read(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        match self.entry(name)?.get_secret() {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(Error::NeedsUnlock),
        }
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        self.entry(name)?
            .set_secret(bytes)
            .map_err(|_| Error::NeedsUnlock)
    }
    fn remove(&self, name: &str) -> Result<()> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(Error::NeedsUnlock),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a real unlocked Linux Secret Service session"]
    fn disposable_secret_service_entry_roundtrip() {
        let store = SecretService::new(Environment::Staging);
        let name = format!("test:{}", uuid::Uuid::new_v4());
        store.write(&name, b"disposable-test-value").unwrap();
        let read = store.read(&name);
        let removed = store.remove(&name);
        assert_eq!(read.unwrap().unwrap().as_slice(), b"disposable-test-value");
        removed.unwrap();
        assert!(store.read(&name).unwrap().is_none());
    }
}

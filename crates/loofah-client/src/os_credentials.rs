use hypr_loofah_protocol::{ApplicationError, ErrorCode};

use crate::{CredentialFuture, CredentialStore, failure};

pub struct OsCredentials {
    service: String,
}
impl OsCredentials {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

impl CredentialStore for OsCredentials {
    fn load<'a>(&'a self, account: &'a str) -> CredentialFuture<'a, Option<String>> {
        let service = self.service.clone();
        let account = account.to_string();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let entry =
                    keyring::Entry::new(&service, &account).map_err(|_| credential_error())?;
                match entry.get_password() {
                    Ok(value) => Ok(Some(value)),
                    Err(keyring::Error::NoEntry) => Ok(None),
                    Err(_) => Err(credential_error()),
                }
            })
            .await
            .map_err(|_| credential_error())?
        })
    }
    fn save<'a>(&'a self, account: &'a str, value: Option<&'a str>) -> CredentialFuture<'a, ()> {
        let service = self.service.clone();
        let account = account.to_string();
        let value = value.map(str::to_owned);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let entry =
                    keyring::Entry::new(&service, &account).map_err(|_| credential_error())?;
                if let Some(value) = value {
                    entry.set_password(&value).map_err(|_| credential_error())
                } else {
                    match entry.delete_credential() {
                        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                        Err(_) => Err(credential_error()),
                    }
                }
            })
            .await
            .map_err(|_| credential_error())?
        })
    }
}
fn credential_error() -> ApplicationError {
    failure(
        ErrorCode::Internal,
        "Cannot access saved server credentials. Unlock the device credential store and try again.",
    )
}

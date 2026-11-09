use crate::{Error, Result};
use hypr_storage::vault::VaultLease;
#[cfg(windows)]
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
use unix as native;
#[cfg(windows)]
use windows as native;

pub(crate) trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
pub(crate) type Stream = Box<dyn Io>;
pub(crate) use native::Listener;

#[derive(Debug, Clone)]
pub struct EndpointRoot {
    path: Arc<PathBuf>,
}
impl EndpointRoot {
    pub fn system() -> Result<Self> {
        Self::new(native::system_root()?)
    }
    pub fn new(path: PathBuf) -> Result<Self> {
        native::ensure_root(&path)?;
        Ok(Self {
            path: Arc::new(path.canonicalize()?),
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn endpoint(&self, vault: &Path) -> Result<Endpoint> {
        let vault = vault.canonicalize()?;
        let key = hypr_storage::vault::canonical_vault_key(&vault);
        Ok(Endpoint {
            address: native::address(self.path(), &key)?,
            key,
        })
    }
}

#[derive(Clone)]
pub(crate) struct Endpoint {
    pub(crate) address: native::Address,
    pub(crate) key: String,
}
impl Endpoint {
    pub(crate) async fn bind(&self, lease: &VaultLease) -> Result<Listener> {
        if self.key != hypr_storage::vault::canonical_vault_key(lease.canonical_vault()) {
            return Err(Error::protocol("Owner lease does not match endpoint"));
        }
        native::bind(&self.address).await
    }
    pub(crate) async fn connect(&self) -> Result<Stream> {
        native::connect(&self.address).await
    }
}

#[cfg(windows)]
pub(crate) fn hash(bytes: &[u8]) -> String {
    crate::hex(&Sha256::digest(bytes))
}

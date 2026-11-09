use std::{future::Future, path::Path, pin::Pin, sync::Arc};

use hypr_loofah_protocol::{LocalCliOperation, LocalCliOperationKind, LocalCliResponse};
use hypr_storage::vault::VaultLease;

mod blob;
#[cfg(feature = "core")]
mod core;
#[cfg(feature = "core")]
pub use core::CoreOwner;
mod client;
mod platform;
mod progress;
mod server;
mod wire;

pub use blob::Blob;
pub use client::{OwnerClient, OwnerResolution};
pub use platform::EndpointRoot;
pub use progress::ProgressReporter;
pub use server::OwnerServer;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Protocol(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("Local owner stopped")]
    Stopped,
    #[error(
        "Local application worker stopped unexpectedly; vault ownership is retained for review"
    )]
    ApplicationWorkerLost,
    #[error(
        "Local operation outcome is unknown; inspect the vault before retrying request {request_id}"
    )]
    Unknown { request_id: String },
}
impl Error {
    pub(crate) fn protocol(message: &str) -> Self {
        Self::Protocol(message.into())
    }
}

pub struct OwnerRequest {
    pub request_id: String,
    pub operation: LocalCliOperation,
    pub blob: Option<Blob>,
    pub progress: ProgressReporter,
}

pub struct OwnerResponse {
    pub response: LocalCliResponse,
    pub blob: Option<Blob>,
}

pub type OwnerFuture = Pin<Box<dyn Future<Output = OwnerResponse> + Send + 'static>>;
pub trait OwnerHandler: Send + Sync + 'static {
    fn operations(&self) -> Vec<LocalCliOperationKind>;
    fn execute(&self, request: OwnerRequest) -> OwnerFuture;
}

pub(crate) fn validate_temporary_root(path: &Path) -> Result<()> {
    // Transfer artifacts must stay in a current-user directory, never a wire-supplied path.
    EndpointRoot::new(path.to_owned()).map(|_| ())
}

pub(crate) fn owner_pin(lease: &VaultLease) -> Arc<std::fs::File> {
    lease.pin()
}

#[cfg(test)]
mod tests;

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

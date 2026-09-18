#![forbid(unsafe_code)]

pub mod credentials;
pub mod crypto;
pub mod engine;
pub mod enrollment;
#[cfg(target_os = "macos")]
pub mod keychain;
pub mod owner;
pub mod pairing;
pub mod remote;
pub mod replica;
pub mod runtime;
pub mod scheduler;
pub mod scope;
pub mod snapshot;
pub mod spool;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("credential store is locked or unavailable; unlock the selected store and retry")]
    NeedsUnlock,
    #[error("saved credential is missing; connect or import your recovery kit")]
    CredentialMissing,
    #[error("sync I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid sync data: {0}")]
    Invalid(String),
    #[error("local content changed; preserve and reconcile before retrying")]
    Conflict,
    #[error("recording is not finalized")]
    Recording,
    #[error("insufficient disk space for sync staging")]
    DiskFull,
    #[error("sync state requires recovery")]
    RecoveryRequired,
    #[error("encrypted content failed authentication")]
    Authentication,
    #[error("sync network request failed")]
    Network(#[from] reqwest::Error),
    #[error("cloud sync request failed ({status}: {code})")]
    Cloud { status: u16, code: String },
    #[error("vault store: {0}")]
    Store(#[from] hypr_vault_write::StoreError),
}

pub type Result<T> = std::result::Result<T, Error>;

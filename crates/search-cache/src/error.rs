#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Local search cache is missing, incomplete, or incompatible; run loof init")]
    NotReady,
    #[error("Local search cache is corrupt ({0}); run loof init")]
    Corrupt(String),
    #[error("Local search cache is busy; retry after the other operation finishes")]
    Busy,
    #[error("Vault changed during reconciliation; retry")]
    Changed,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Tantivy(#[from] tantivy::TantivyError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Vault(#[from] hypr_vault_read::Error),
    #[error("{0}")]
    InvalidInput(String),
}
pub type Result<T> = std::result::Result<T, Error>;

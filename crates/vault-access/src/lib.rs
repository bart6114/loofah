pub mod client;
pub mod connection;
pub mod filesystem;
pub mod recovery;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Versions = BTreeMap<String, Option<String>>;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Call {
    pub operation: String,
    #[serde(default)]
    pub args: Value,
    pub mutation_id: Option<String>,
    #[serde(default)]
    pub expected: Versions,
}

impl Call {
    pub fn query(operation: impl Into<String>, args: Value) -> Self {
        Self {
            operation: operation.into(),
            args,
            mutation_id: None,
            expected: Versions::new(),
        }
    }

    pub fn mutation(operation: impl Into<String>, args: Value, expected: Versions) -> Self {
        Self {
            operation: operation.into(),
            args,
            mutation_id: Some(uuid::Uuid::new_v4().to_string()),
            expected,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Reply {
    pub data: Value,
    #[serde(default)]
    pub versions: Versions,
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error, utoipa::ToSchema)]
#[error("{code}: {message}")]
pub struct Error {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            operation_id: None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::new("storage", value.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::new("invalid_data", value.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait VaultAccess: Send + Sync {
    fn invoke(&self, call: Call) -> impl std::future::Future<Output = Result<Reply>> + Send;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Editor,
    Viewer,
}

impl Role {
    pub fn can_write(self) -> bool {
        self != Self::Viewer
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct User {
    pub id: String,
    pub issuer: String,
    pub subject: String,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Vault {
    pub id: String,
    pub name: String,
    pub personal: bool,
    pub members: BTreeMap<String, Role>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServerInfo {
    pub id: String,
    pub api_version: u32,
    pub authentication: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Credentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub user: User,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChangeEvent {
    pub cursor: String,
    pub vault_id: Option<String>,
    pub entity: String,
    pub ids: Vec<String>,
    pub reset: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Upload {
    pub id: String,
    pub session_id: String,
    pub filename: String,
    pub size: u64,
    pub offset: u64,
    pub sha256: String,
    pub expected: Option<String>,
    pub finished: bool,
}

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn validate_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 255
        || value.starts_with('.')
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::new("invalid_request", "invalid identifier"));
    }
    Ok(())
}

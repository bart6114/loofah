#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Cache(#[from] hypr_search_cache::Error),
    #[error(
        "Vault write persisted, but command completion failed: {reason}. Do not repeat the write; run loof init to refresh the cache"
    )]
    WritePersisted {
        reason: String,
        session_ids: Vec<String>,
    },
    #[error("{0} not found")]
    NotFound(String),
    #[error("Loofah vault not found at {0}; start Loofah once or pass --vault-path")]
    VaultNotFound(std::path::PathBuf),
    #[error("output file already exists at {0}; pass --force to overwrite it")]
    OutputExists(std::path::PathBuf),
    #[error("{action} failed: {reason}")]
    Operation {
        action: &'static str,
        reason: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<hypr_agent_access::Error> for Error {
    fn from(error: hypr_agent_access::Error) -> Self {
        match error {
            hypr_agent_access::Error::Cache(error) => Self::Cache(error),
            hypr_agent_access::Error::NotFound(what) => Self::NotFound(what),
            hypr_agent_access::Error::InvalidInput(reason) => {
                Self::operation("search meetings", reason)
            }
            hypr_agent_access::Error::Vault { action, reason } => Self::operation(action, reason),
        }
    }
}

impl Error {
    pub fn operation(action: &'static str, reason: impl Into<String>) -> Self {
        Self::Operation {
            action,
            reason: reason.into(),
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            Self::NotFound(_) => 2,
            Self::VaultNotFound(_) => 3,
            Self::OutputExists(_) => 4,
            Self::Operation { .. } | Self::Cache(_) | Self::WritePersisted { .. } => 1,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Cache(hypr_search_cache::Error::NotReady) => "cache_not_ready",
            Self::Cache(hypr_search_cache::Error::Corrupt(_)) => "cache_corrupt",
            Self::Cache(hypr_search_cache::Error::Busy) => "cache_busy",
            Self::Cache(_) => "cache_reconciliation_failed",
            Self::WritePersisted { .. } => "write_persisted",
            Self::NotFound(_) => "not_found",
            Self::VaultNotFound(_) => "vault_not_found",
            Self::OutputExists(_) => "output_exists",
            Self::Operation { .. } => "operation_failed",
        }
    }

    pub fn to_json(&self) -> String {
        let mut response = serde_json::json!({
            "schema_version": crate::output::JSON_SCHEMA_VERSION,
            "error": {
                "code": self.code(),
                "message": self.to_string(),
                "exit_code": self.exit_code(),
            }
        });
        if let Self::WritePersisted { session_ids, .. } = self {
            response["error"]["write_persisted"] = serde_json::json!(true);
            response["error"]["session_ids"] = serde_json::json!(session_ids);
        }
        serde_json::to_string(&response).expect("error response is always serializable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_error_has_stable_machine_readable_code() {
        let error = Error::NotFound("meeting 'missing'".to_string());
        let response: serde_json::Value = serde_json::from_str(&error.to_json()).unwrap();

        assert_eq!(response["schema_version"], "2");
        assert_eq!(response["error"]["code"], "not_found");
        assert_eq!(response["error"]["exit_code"], 2);
    }
}

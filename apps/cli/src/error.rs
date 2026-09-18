#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{reason}")]
    Sync { code: &'static str, reason: String },
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
            Self::Operation { .. } | Self::Sync { .. } => 1,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::VaultNotFound(_) => "vault_not_found",
            Self::OutputExists(_) => "output_exists",
            Self::Operation { .. } => "operation_failed",
            Self::Sync { code, .. } => code,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": crate::output::JSON_SCHEMA_VERSION,
            "error": {
                "code": self.code(),
                "message": self.to_string(),
                "exit_code": self.exit_code(),
            }
        }))
        .expect("error response is always serializable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_error_has_stable_machine_readable_code() {
        let error = Error::NotFound("meeting 'missing'".to_string());
        let response: serde_json::Value = serde_json::from_str(&error.to_json()).unwrap();

        assert_eq!(response["schema_version"], "1");
        assert_eq!(response["error"]["code"], "not_found");
        assert_eq!(response["error"]["exit_code"], 2);
    }
}

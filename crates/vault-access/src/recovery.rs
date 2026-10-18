use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    Call, Error, Result,
    filesystem::{atomic_json, safe_path},
    validate_id,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Destination {
    pub server_id: String,
    pub user_id: String,
    pub vault_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMutation {
    #[serde(default)]
    pub attempted: bool,
    #[serde(default)]
    pub created_at_ms: u64,
    pub destination: Destination,
    pub call: Call,
    pub error: Option<Error>,
}

pub struct RecoverySpool {
    root: PathBuf,
}

impl RecoverySpool {
    pub fn new(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    pub fn save(&self, pending: &PendingMutation) -> Result<()> {
        let id = pending
            .call
            .mutation_id
            .as_deref()
            .ok_or_else(|| Error::new("invalid_request", "only mutations may be spooled"))?;
        validate_id(id)?;
        atomic_json(&safe_path(&self.root, &format!("{id}.json"))?, pending)
    }

    pub fn list(&self, destination: &Destination) -> Result<Vec<PendingMutation>> {
        let mut pending = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_file()
                || entry.path().extension().and_then(|s| s.to_str()) != Some("json")
            {
                continue;
            }
            let item: PendingMutation = serde_json::from_slice(&std::fs::read(entry.path())?)?;
            if item.destination == *destination {
                pending.push(item);
            }
        }
        pending.sort_by_key(|item| item.created_at_ms);
        Ok(pending)
    }

    pub fn acknowledge(&self, id: &str) -> Result<()> {
        validate_id(id)?;
        std::fs::remove_file(safe_path(&self.root, &format!("{id}.json"))?)?;
        std::fs::File::open(&self.root)?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

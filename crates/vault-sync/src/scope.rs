use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::{Error, Result};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FILES: usize = 100_000;
pub const MAX_PATH_BYTES: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Entity {
    Session(String),
    People,
    Tags,
    Tasks,
}

impl Entity {
    pub fn root(&self) -> Result<PathBuf> {
        match self {
            Self::Session(id) => {
                hypr_vault_read::paths::validate_session_id(id)
                    .map_err(|_| Error::Invalid("invalid session identity".into()))?;
                Ok(Path::new("sessions").join(id))
            }
            _ => Ok(PathBuf::new()),
        }
    }

    pub fn registry_name(&self) -> Option<&'static str> {
        match self {
            Self::Session(_) => None,
            Self::People => Some("people.json"),
            Self::Tags => Some("tags.json"),
            Self::Tasks => Some("tasks.json"),
        }
    }

    pub fn validate_path(&self, path: &str) -> Result<()> {
        if path.is_empty() || path.len() > MAX_PATH_BYTES || path.contains(['\\', '\0', ':']) {
            return Err(Error::Invalid("unsafe content path".into()));
        }
        let parts: Vec<_> = path.split('/').collect();
        if parts
            .iter()
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with('.'))
        {
            return Err(Error::Invalid("unsafe content path".into()));
        }
        let allowed = match self {
            Self::Session(_) => match parts.as_slice() {
                [name] => hypr_vault_read::SESSION_OWNED_FILES.contains(name),
                ["enhanced", name] => name.ends_with(".md"),
                ["attachments", ..] => parts.len() > 1,
                ["audio", name] => matches!(
                    Path::new(name).extension().and_then(|s| s.to_str()),
                    Some("mp3" | "wav" | "ogg")
                ),
                _ => false,
            },
            _ => self.registry_name() == Some(path),
        };
        if !allowed {
            return Err(Error::Invalid("path is outside synced content".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_content_only() {
        let entity = Entity::Session("one".into());
        for path in [
            "_meta.json",
            "notes.md",
            "_memo.md",
            "audio.mp3",
            "enhanced/doc.md",
            "attachments/image.png",
            "attachments/sub/file.pdf",
            "audio/recording.wav",
        ] {
            entity.validate_path(path).unwrap();
        }
        for path in [
            "config.json",
            "people.json",
            "image.png",
            "notes.txt",
            "audio.peaks.json",
            "audio_mic.wav",
            "audio.mp3.tmp",
            "enhanced/.hidden.md",
            "attachments/../notes.md",
            "attachments//file",
            "attachments/.DS_Store",
            "/notes.md",
            "attachments/a\\b",
            "attachments/./file",
            "enhanced/sub/doc.md",
        ] {
            assert!(entity.validate_path(path).is_err(), "{path}");
        }
        assert!(Entity::People.validate_path("people.json").is_ok());
        assert!(Entity::People.validate_path("tasks.json").is_err());
    }
}

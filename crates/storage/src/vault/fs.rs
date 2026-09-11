use std::path::Path;

use serde::Serialize;

use super::path::{CONFIG_FILENAME, VAULT_PATH_KEY};
use crate::fs::copy_dir_recursive;

/// What a picked storage folder currently holds, so the frontend can shape the
/// change-location dialog around the user's likely intent (move into empty,
/// switch to an existing vault, or create a subfolder inside a busy directory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum VaultDirKind {
    EmptyOrMissing,
    Vault,
    Other,
}

pub fn classify_vault_dir(path: &Path) -> std::io::Result<VaultDirKind> {
    if is_empty_or_missing_dir(path)? {
        return Ok(VaultDirKind::EmptyOrMissing);
    }
    if path.join("sessions").is_dir() || path.join(CONFIG_FILENAME).is_file() {
        return Ok(VaultDirKind::Vault);
    }
    Ok(VaultDirKind::Other)
}

// `search_index` is deliberately absent: the Tantivy cache lives under the global base
// (which coincides with the vault at the default location), so copying it would snapshot
// a live index and removing it would delete one; it rebuilds from the vault anyway.
const VAULT_DIRECTORIES: &[&str] = &[
    "sessions",
    "humans",
    "organizations",
    "chats",
    "prompts",
    "plugins",
    "templates",
    ".trash",
];

const VAULT_FILES: &[&str] = &[
    "AGENTS.md",
    "settings.json",
    "config.json",
    "events.json",
    "calendars.json",
    "templates.json",
    "store.json",
    "people.json",
    "tasks.json",
    "tags.json",
];

pub async fn copy_vault_items(src: &Path, dst: &Path) -> std::io::Result<()> {
    for dir_name in VAULT_DIRECTORIES {
        let src_dir = src.join(dir_name);
        let dst_dir = dst.join(dir_name);

        if src_dir.exists() && src_dir.is_dir() {
            tokio::fs::create_dir_all(&dst_dir).await?;
            copy_dir_recursive(&src_dir, &dst_dir, None).await?;
        }
    }

    for file_name in VAULT_FILES {
        let src_file = src.join(file_name);
        let dst_file = dst.join(file_name);

        if src_file.exists() && src_file.is_file() {
            tokio::fs::copy(&src_file, &dst_file).await?;
        }
    }

    Ok(())
}

pub fn is_empty_or_missing_dir(path: &Path) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(true);
    }

    Ok(std::fs::read_dir(path)?.next().transpose()?.is_none())
}

pub async fn remove_vault_items(path: &Path) -> std::io::Result<()> {
    for dir_name in VAULT_DIRECTORIES {
        let dir = path.join(dir_name);
        if dir.exists() && dir.is_dir() {
            tokio::fs::remove_dir_all(&dir).await?;
        }
    }

    for file_name in VAULT_FILES {
        let file = path.join(file_name);
        if file.exists() && file.is_file() {
            tokio::fs::remove_file(&file).await?;
        }
    }

    Ok(())
}

pub fn set_vault_path(config: &mut serde_json::Value, path: &Path) {
    if let Some(obj) = config.as_object_mut() {
        obj.insert(
            VAULT_PATH_KEY.to_string(),
            serde_json::Value::String(path.to_string_lossy().to_string()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn copy_vault_items_copies_only_vault() {
        let temp = tempdir().unwrap();
        let src = temp.path().join("src");
        let dst = temp.path().join("dst");

        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::create_dir_all(src.join("sessions")).unwrap();
        fs::write(src.join("sessions").join("test.json"), "session").unwrap();
        fs::create_dir_all(src.join("humans")).unwrap();
        fs::write(src.join("humans").join("person.md"), "human").unwrap();
        fs::write(src.join("events.json"), "events").unwrap();
        fs::write(src.join("settings.json"), "settings").unwrap();
        fs::write(src.join("config.json"), "config").unwrap();

        fs::write(src.join("store.json"), "store").unwrap();
        fs::create_dir_all(src.join("models")).unwrap();
        fs::write(src.join("models").join("model.gguf"), "model").unwrap();
        fs::create_dir_all(src.join(".trash").join("2026-08-17")).unwrap();
        fs::write(
            src.join(".trash").join("2026-08-17").join("deleted.md"),
            "gone",
        )
        .unwrap();
        fs::create_dir_all(src.join("search_index")).unwrap();
        fs::write(src.join("search_index").join("meta.json"), "tantivy").unwrap();

        copy_vault_items(&src, &dst).await.unwrap();

        assert!(dst.join("sessions").join("test.json").exists());
        assert!(dst.join("humans").join("person.md").exists());
        assert!(dst.join("events.json").exists());
        assert!(dst.join("settings.json").exists());
        assert!(dst.join("config.json").exists());
        assert!(
            dst.join(".trash")
                .join("2026-08-17")
                .join("deleted.md")
                .exists()
        );

        assert!(dst.join("store.json").exists());
        assert!(!dst.join("models").exists());
        assert!(!dst.join("search_index").exists());
    }

    /// The remove side must mirror the copy side: the live Tantivy index (global base ==
    /// vault base at the default location) must survive a move's cleanup pass.
    #[tokio::test]
    async fn remove_vault_items_leaves_search_index_alone() {
        let temp = tempdir().unwrap();
        let src = temp.path().join("src");

        fs::create_dir_all(src.join("sessions")).unwrap();
        fs::create_dir_all(src.join("search_index")).unwrap();
        fs::write(src.join("search_index").join("meta.json"), "tantivy").unwrap();
        fs::create_dir_all(src.join(".trash")).unwrap();
        fs::write(src.join("config.json"), "config").unwrap();

        remove_vault_items(&src).await.unwrap();

        assert!(!src.join("sessions").exists());
        assert!(!src.join(".trash").exists());
        assert!(!src.join("config.json").exists());
        assert!(src.join("search_index").join("meta.json").exists());
    }

    #[tokio::test]
    async fn copy_vault_items_handles_missing_items() {
        let temp = tempdir().unwrap();
        let src = temp.path().join("src");
        let dst = temp.path().join("dst");

        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join("events.json"), "events").unwrap();

        copy_vault_items(&src, &dst).await.unwrap();

        assert!(dst.join("events.json").exists());
        assert!(!dst.join("sessions").exists());
    }

    #[test]
    fn classify_vault_dir_covers_all_shapes() {
        let temp = tempdir().unwrap();

        let missing = temp.path().join("missing");
        assert_eq!(
            classify_vault_dir(&missing).unwrap(),
            VaultDirKind::EmptyOrMissing
        );

        let empty = temp.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert_eq!(
            classify_vault_dir(&empty).unwrap(),
            VaultDirKind::EmptyOrMissing
        );

        let vault = temp.path().join("vault");
        fs::create_dir_all(vault.join("sessions")).unwrap();
        assert_eq!(classify_vault_dir(&vault).unwrap(), VaultDirKind::Vault);

        let config_only = temp.path().join("config-only");
        fs::create_dir_all(&config_only).unwrap();
        fs::write(config_only.join("config.json"), "{}").unwrap();
        assert_eq!(
            classify_vault_dir(&config_only).unwrap(),
            VaultDirKind::Vault
        );

        let obsidian = temp.path().join("obsidian");
        fs::create_dir_all(obsidian.join(".obsidian")).unwrap();
        assert_eq!(classify_vault_dir(&obsidian).unwrap(), VaultDirKind::Other);

        let other = temp.path().join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("random.txt"), "x").unwrap();
        assert_eq!(classify_vault_dir(&other).unwrap(), VaultDirKind::Other);
    }

    #[test]
    fn is_empty_or_missing_dir_accepts_missing_path() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("missing");

        assert!(is_empty_or_missing_dir(&path).unwrap());
    }

    #[test]
    fn is_empty_or_missing_dir_accepts_empty_directory() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("empty");
        fs::create_dir_all(&path).unwrap();

        assert!(is_empty_or_missing_dir(&path).unwrap());
    }

    #[test]
    fn is_empty_or_missing_dir_rejects_populated_directory() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("populated");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("note.md"), "note").unwrap();

        assert!(!is_empty_or_missing_dir(&path).unwrap());
    }

    #[test]
    fn set_vault_path_sets_path() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("vault");

        let mut config = serde_json::json!({});
        set_vault_path(&mut config, &path);

        assert_eq!(
            config.get(VAULT_PATH_KEY).and_then(|v| v.as_str()),
            Some(path.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn set_vault_path_preserves_existing_fields() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("vault");

        let mut config = serde_json::json!({"theme": "dark", "language": "en"});
        set_vault_path(&mut config, &path);

        assert_eq!(config.get("theme").and_then(|v| v.as_str()), Some("dark"));
        assert_eq!(config.get("language").and_then(|v| v.as_str()), Some("en"));
        assert_eq!(
            config.get(VAULT_PATH_KEY).and_then(|v| v.as_str()),
            Some(path.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn set_vault_path_overwrites_existing() {
        let temp = tempdir().unwrap();
        let old_path = temp.path().join("old");
        let new_path = temp.path().join("new");

        let mut config = serde_json::json!({ VAULT_PATH_KEY: old_path.to_string_lossy() });
        set_vault_path(&mut config, &new_path);

        assert_eq!(
            config.get(VAULT_PATH_KEY).and_then(|v| v.as_str()),
            Some(new_path.to_string_lossy().as_ref())
        );
    }
}

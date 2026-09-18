use serde::{Deserialize, Serialize};

use super::{SessionStore, StoreError, paths};

/// One tag, file-canonical in the vault-root `tags.json`. The id is the normalized
/// (lowercased) name itself — unlike people's lossy slug, two names normalizing
/// identically are by definition the same tag, so no collision suffixing is needed.
/// Sessions keep storing raw tag strings in `_meta.json` and degrade gracefully if
/// `tags.json` disappears.
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
pub struct TagItem {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct TagsFile {
    #[serde(default)]
    tags: Vec<TagItem>,
}

/// Trim, strip a leading `#`, lowercase. `None` when nothing is left — the strict
/// charset filter stays on the frontend; this is only what file-level dedupe needs.
fn normalize_tag_name(raw: &str) -> Option<String> {
    hypr_vault_read::normalize_tag_name(raw)
}

impl SessionStore {
    /// A missing `tags.json` is an empty registry, and an unparseable one must never
    /// take the typeahead down with it — sessions keep their raw tag strings either way.
    async fn read_tags(&self) -> Result<Vec<TagItem>, StoreError> {
        let path = self.vault_base.join(paths::tags_path());
        tokio::task::spawn_blocking(move || {
            let raw = match std::fs::read(&path) {
                Ok(raw) => raw,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
                Err(e) => {
                    tracing::warn!("failed to read tags.json; treating as empty: {e}");
                    return Vec::new();
                }
            };
            match serde_json::from_slice::<TagsFile>(&raw) {
                Ok(file) => file.tags,
                Err(e) => {
                    tracing::warn!("failed to parse tags.json; treating as empty: {e}");
                    Vec::new()
                }
            }
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {e}")))
    }

    pub async fn list_tags(&self) -> Result<Vec<TagItem>, StoreError> {
        let _guard = self.lock_reads().await?;
        let mut tags = self.read_tags().await?;
        tags.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok(tags)
    }

    /// Registry is append-only: an existing tag is returned untouched, otherwise it is
    /// created under its normalized name. The guard spans read and write: `tags.json`
    /// is whole-file rewritten, so two concurrent ensures without it could drop each
    /// other's entry.
    pub async fn ensure_tag(&self, name: &str) -> Result<TagItem, StoreError> {
        let Some(normalized) = normalize_tag_name(name) else {
            return Err(StoreError::Io("tag name cannot be empty".to_string()));
        };

        let guard = self.lock_writes().await?;

        let tags = self.read_tags().await?;
        if let Some(existing) = tags.iter().find(|t| t.id == normalized) {
            return Ok(existing.clone());
        }

        let tag = TagItem {
            id: normalized.clone(),
            name: normalized,
        };
        self.append_registry_locked(
            &guard,
            paths::tags_path(),
            "tags",
            serde_json::to_value(&tag).map_err(|error| StoreError::Serialize(error.to_string()))?,
        )
        .await?;

        self.index_upsert_tag(&tag);
        self.notify_index_changed(super::IndexEntity::Tags, vec![tag.id.clone()]);
        Ok(tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ensure_tag_creates_file_lazily_and_reuses_case_insensitively() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        assert!(!vault.path().join("tags.json").exists());

        let created = store.ensure_tag("Project-X").await.unwrap();
        assert_eq!(created.id, "project-x");
        assert_eq!(created.name, "project-x");
        assert!(vault.path().join("tags.json").exists());

        let mtime_after_create = std::fs::metadata(vault.path().join("tags.json"))
            .unwrap()
            .modified()
            .unwrap();

        let reused = store.ensure_tag("#PROJECT-x").await.unwrap();
        assert_eq!(reused, created);
        let mtime_after_reuse = std::fs::metadata(vault.path().join("tags.json"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(mtime_after_create, mtime_after_reuse);

        let listed = store.list_tags().await.unwrap();
        assert_eq!(listed, vec![created]);
    }

    #[tokio::test]
    async fn ensure_tag_rejects_empty_names() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        assert!(store.ensure_tag("   ").await.is_err());
        assert!(store.ensure_tag("#").await.is_err());
        assert!(!vault.path().join("tags.json").exists());
    }

    #[tokio::test]
    async fn concurrent_ensures_of_same_name_yield_one_tag() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());

        let (a, b) = tokio::join!(store.ensure_tag("Hiring"), store.ensure_tag("hiring"));
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a.id, b.id);

        let listed = store.list_tags().await.unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[tokio::test]
    async fn unparseable_tags_file_reads_as_empty_but_is_not_overwritten() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::write(vault.path().join("tags.json"), b"{not json").unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());

        assert_eq!(store.list_tags().await.unwrap(), vec![]);

        assert!(store.ensure_tag("standup").await.is_err());
        assert_eq!(
            std::fs::read(vault.path().join("tags.json")).unwrap(),
            b"{not json"
        );
        assert_eq!(store.list_tags().await.unwrap(), vec![]);
    }

    #[tokio::test]
    async fn list_tags_sorts_alphabetically() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());

        store.ensure_tag("zebra").await.unwrap();
        store.ensure_tag("alpha").await.unwrap();
        store.ensure_tag("hiring").await.unwrap();

        let names: Vec<String> = store
            .list_tags()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, vec!["alpha", "hiring", "zebra"]);
    }
}

use serde::{Deserialize, Serialize};

use super::{SessionStore, StoreError, paths};

/// One person, file-canonical in the vault-root `people.json`. The id doubles as the
/// value speaker hints store, so it is generated human-readable (`bob_peters`) — a
/// transcript keeps degrading gracefully if `people.json` disappears.
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
pub struct PersonItem {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct PeopleFile {
    #[serde(default)]
    people: Vec<PersonItem>,
}

fn person_id_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut pending_separator = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            if pending_separator && !slug.is_empty() {
                slug.push('_');
            }
            pending_separator = false;
            slug.push(c);
        } else {
            pending_separator = true;
        }
    }

    if slug.is_empty() {
        "person".to_string()
    } else {
        slug
    }
}

fn unique_person_id(existing: &[PersonItem], name: &str) -> String {
    let base = person_id_slug(name);
    if !existing.iter().any(|p| p.id == base) {
        return base;
    }

    let mut counter = 2;
    loop {
        let candidate = format!("{base}_{counter}");
        if !existing.iter().any(|p| p.id == candidate) {
            return candidate;
        }
        counter += 1;
    }
}

impl SessionStore {
    /// A missing `people.json` is an empty registry, and an unparseable one must never
    /// take rendering down with it — labels fall back to the raw hint value either way.
    async fn read_people(&self) -> Result<Vec<PersonItem>, StoreError> {
        let path = self.vault_base.join(paths::people_path());
        tokio::task::spawn_blocking(move || {
            let raw = match std::fs::read(&path) {
                Ok(raw) => raw,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
                Err(e) => {
                    tracing::warn!("failed to read people.json; treating as empty: {e}");
                    return Vec::new();
                }
            };
            match serde_json::from_slice::<PeopleFile>(&raw) {
                Ok(file) => file.people,
                Err(e) => {
                    tracing::warn!("failed to parse people.json; treating as empty: {e}");
                    Vec::new()
                }
            }
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {e}")))
    }

    pub async fn list_people(&self) -> Result<Vec<PersonItem>, StoreError> {
        let _guard = self.lock_reads().await?;
        let mut people = self.read_people().await?;
        people.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(people)
    }

    /// Case-insensitive name match returns the existing person untouched (renaming a
    /// speaker must never mutate the registry); otherwise the person is created with a
    /// collision-free generated id. The guard spans read and write: `people.json` is
    /// whole-file rewritten, so two concurrent ensures without it could both mint
    /// `bob_peters` or drop each other's entry.
    pub async fn ensure_person(&self, name: &str) -> Result<PersonItem, StoreError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Io("person name cannot be empty".to_string()));
        }

        let guard = self.lock_writes().await?;

        let mut people = self.read_people().await?;
        let name_lower = name.to_lowercase();
        if let Some(existing) = people.iter().find(|p| p.name.to_lowercase() == name_lower) {
            return Ok(existing.clone());
        }

        let person = PersonItem {
            id: unique_person_id(&people, name),
            name: name.to_string(),
        };
        people.push(person.clone());

        let bytes = serde_json::to_vec_pretty(&PeopleFile { people })
            .map_err(|e| StoreError::Serialize(e.to_string()))?;
        self.write_file_locked(&guard, paths::people_path(), bytes)
            .await?;

        self.index_upsert_person(&person);
        self.notify_index_changed(super::IndexEntity::People, vec![person.id.clone()]);
        Ok(person)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(id: &str, name: &str) -> PersonItem {
        PersonItem {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn slugifies_names() {
        assert_eq!(person_id_slug("Bob Peters"), "bob_peters");
        assert_eq!(person_id_slug("  José! "), "jos");
        assert_eq!(person_id_slug("李明"), "person");
        assert_eq!(person_id_slug("a - b -- c"), "a_b_c");
        assert_eq!(person_id_slug("O'Brien, Anne-Marie"), "o_brien_anne_marie");
        assert_eq!(person_id_slug("!!!"), "person");
    }

    #[test]
    fn suffixes_colliding_ids() {
        let existing = vec![person("bob_peters", "Bob Peters")];
        assert_eq!(unique_person_id(&existing, "Bob! Peters"), "bob_peters_2");

        let existing = vec![
            person("bob_peters", "Bob Peters"),
            person("bob_peters_2", "Bob Peters"),
        ];
        assert_eq!(unique_person_id(&existing, "Bob Peters"), "bob_peters_3");
    }

    #[tokio::test]
    async fn ensure_person_creates_file_lazily_and_reuses_case_insensitively() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        assert!(!vault.path().join("people.json").exists());

        let created = store.ensure_person("Bob Peters").await.unwrap();
        assert_eq!(created.id, "bob_peters");
        assert_eq!(created.name, "Bob Peters");
        assert!(vault.path().join("people.json").exists());

        let mtime_after_create = std::fs::metadata(vault.path().join("people.json"))
            .unwrap()
            .modified()
            .unwrap();

        let reused = store.ensure_person("bob peters").await.unwrap();
        assert_eq!(reused, created);
        let mtime_after_reuse = std::fs::metadata(vault.path().join("people.json"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(mtime_after_create, mtime_after_reuse);

        let listed = store.list_people().await.unwrap();
        assert_eq!(listed, vec![created]);
    }

    #[tokio::test]
    async fn ensure_person_rejects_empty_names() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        assert!(store.ensure_person("   ").await.is_err());
        assert!(!vault.path().join("people.json").exists());
    }

    #[tokio::test]
    async fn concurrent_ensures_of_same_name_yield_one_person() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());

        let (a, b) = tokio::join!(
            store.ensure_person("Bob Peters"),
            store.ensure_person("bob peters")
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a.id, b.id);

        let listed = store.list_people().await.unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[tokio::test]
    async fn unparseable_people_file_is_treated_as_empty() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::write(vault.path().join("people.json"), b"{not json").unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());

        assert_eq!(store.list_people().await.unwrap(), vec![]);

        let created = store.ensure_person("Kim").await.unwrap();
        assert_eq!(created.id, "kim");
        assert_eq!(store.list_people().await.unwrap(), vec![created]);
    }

    #[tokio::test]
    async fn distinct_names_with_same_slug_get_suffixed_ids() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());

        let first = store.ensure_person("Bob Peters").await.unwrap();
        let second = store.ensure_person("Bob-Peters").await.unwrap();
        assert_eq!(first.id, "bob_peters");
        assert_eq!(second.id, "bob_peters_2");
    }
}

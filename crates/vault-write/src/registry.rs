use super::{SessionStore, StoreError, WriteGuard};

impl SessionStore {
    pub(crate) async fn append_registry_locked(
        &self,
        guard: &WriteGuard,
        relative: impl AsRef<std::path::Path>,
        field: &'static str,
        item: serde_json::Value,
    ) -> Result<(), StoreError> {
        let relative = relative.as_ref().to_owned();
        let path = self.vault_base.join(&relative);
        let bytes = tokio::task::spawn_blocking(move || {
            let mut value: serde_json::Value = match std::fs::read(path) {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .map_err(|error| StoreError::Serialize(error.to_string()))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
                Err(error) => return Err(StoreError::Io(error.to_string())),
            };
            let object = value
                .as_object_mut()
                .ok_or_else(|| StoreError::Serialize("invalid registry object".into()))?;
            let items = object
                .entry(field)
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .ok_or_else(|| StoreError::Serialize("invalid registry entries".into()))?;
            items.push(item);
            serde_json::to_vec_pretty(&value)
                .map_err(|error| StoreError::Serialize(error.to_string()))
        })
        .await
        .map_err(|error| StoreError::Io(error.to_string()))??;
        self.write_file_locked(guard, relative, bytes).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn adding_registry_entries_preserves_unknown_fields_and_refuses_corrupt_files() {
        let vault = tempfile::tempdir().unwrap();
        let people = serde_json::json!({"future":{"revision":2}, "people":[{"id":"existing", "name":"Existing", "future":{"email":"example@example.test"}}]});
        let tags = serde_json::json!({"future":true,"tags":[{"id":"existing","name":"existing","color":"purple"}]});
        std::fs::write(vault.path().join("people.json"), people.to_string()).unwrap();
        std::fs::write(vault.path().join("tags.json"), tags.to_string()).unwrap();
        let store = SessionStore::new(vault.path().into());
        store.ensure_person("Added person").await.unwrap();
        store.ensure_tag("Added tag").await.unwrap();
        for (name, before, field) in [
            ("people.json", people, "people"),
            ("tags.json", tags, "tags"),
        ] {
            let after: serde_json::Value =
                serde_json::from_slice(&std::fs::read(vault.path().join(name)).unwrap()).unwrap();
            assert_eq!(after["future"], before["future"]);
            assert_eq!(after[field][0], before[field][0]);
            assert_eq!(after[field].as_array().unwrap().len(), 2);
        }
        std::fs::write(vault.path().join("people.json"), b"{broken").unwrap();
        assert!(store.ensure_person("Another person").await.is_err());
        assert_eq!(
            std::fs::read(vault.path().join("people.json")).unwrap(),
            b"{broken"
        );
    }
}

use crate::{
    PersonItem, SessionStore, StoreError, file_snapshot::FileSnapshot,
    metadata_revision::UniqueValue, paths,
};
use serde_json::Value;
use std::io::Read;
impl SessionStore {
    pub async fn ensure_person_checked(&self, name: String) -> Result<PersonItem, StoreError> {
        hypr_loofah_protocol::validate_person_name(&name)
            .map_err(|cause| StoreError::Serialize(cause.into()))?;
        let store = self.clone();
        tokio::spawn(async move {
            store.ensure_ready()?;
            let guard = store.lock_writes().await;
            let path = store.vault_base.join(paths::people_path());
            let root = store.vault_base.clone();
            let budget = store.content_limits.metadata_bytes;
            let (person, bytes, original) = store
                .spawn_blocking(move || {
                    let directory = std::fs::symlink_metadata(root).map_err(io)?;
                    if directory.file_type().is_symlink() || !directory.is_dir() {
                        return Err(io("People directory is not regular"));
                    }
                    let original = crate::media::open_regular_file(&path)?
                        .map(FileSnapshot::new)
                        .transpose()?;
                    let mut raw = match &original {
                        None => serde_json::json!({"people":[]}),
                        Some(file) => {
                            let mut bytes = Vec::new();
                            file.file()
                                .take((budget as u64).saturating_add(1))
                                .read_to_end(&mut bytes)
                                .map_err(io)?;
                            if bytes.len() > budget {
                                return Err(StoreError::ResourceLimit(
                                    "People registry exceeds its size limit".into(),
                                ));
                            }
                            serde_json::from_slice::<UniqueValue>(&bytes)
                                .map_err(json)?
                                .0
                        }
                    };
                    let values = raw
                        .get_mut("people")
                        .and_then(Value::as_array_mut)
                        .ok_or_else(|| io("People registry requires its original people array"))?;
                    let people = values
                        .iter()
                        .map(|v| serde_json::from_value::<PersonItem>(v.clone()).map_err(json))
                        .collect::<Result<Vec<_>, _>>()?;
                    let mut ids = std::collections::HashSet::new();
                    if people
                        .iter()
                        .any(|p| p.id.is_empty() || !ids.insert(p.id.as_str()))
                    {
                        return Err(io("People registry contains ambiguous identities"));
                    }
                    let name = name.trim();
                    let folded = name.to_lowercase();
                    if let Some(person) = people.iter().find(|p| p.name.to_lowercase() == folded) {
                        return Ok((person.clone(), None, original));
                    }
                    let person = PersonItem {
                        id: crate::people::unique_person_id(&people, name),
                        name: name.into(),
                    };
                    values.push(serde_json::to_value(&person).map_err(json)?);
                    let bytes = crate::transcript_editor::encode(&raw, budget)?;
                    Ok::<_, StoreError>((person, Some(bytes), original))
                })
                .await
                .map_err(io)??;
            if let Some(bytes) = bytes {
                let path = store.vault_base.join(paths::people_path());
                let current = crate::media::open_regular_file(&path)?
                    .map(FileSnapshot::new)
                    .transpose()?;
                let unchanged = match (&original, &current) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.unchanged(b)?,
                    _ => false,
                };
                if !unchanged {
                    return Err(StoreError::Conflict(
                        "People registry changed; read it before trying again".into(),
                    ));
                }
                store
                    .write_file_locked(&guard, paths::people_path(), bytes)
                    .await?;
                store.index_upsert_person(&person);
                store.notify_index_changed(crate::IndexEntity::People, vec![person.id.clone()]);
            }
            Ok(person)
        })
        .await
        .map_err(io)?
    }
}
fn io(cause: impl std::fmt::Display) -> StoreError {
    StoreError::Io(cause.to_string())
}
fn json(cause: impl std::fmt::Display) -> StoreError {
    StoreError::Serialize(cause.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn checked_people_preserves_unknown_fields_and_refuses_corruption() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path().to_path_buf());
        let path = root.path().join("people.json");
        std::fs::write(
            &path,
            br#"{"future":{"keep":true},"people":[{"id":"alice","name":"Alice","extra":42}]}"#,
        )
        .unwrap();
        assert_eq!(
            store
                .ensure_person_checked("alice".into())
                .await
                .unwrap()
                .id,
            "alice"
        );
        store.ensure_person_checked("Bob".into()).await.unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["people"][0]["extra"], 42);
        assert_eq!(value["future"]["keep"], true);
        for raw in [
            br#"{"people":[]"#.as_slice(),
            br#"{"people":[],"people":[]}"#,
            br#"{"future_format":true}"#,
        ] {
            std::fs::write(&path, raw).unwrap();
            assert!(store.ensure_person_checked("New".into()).await.is_err());
            assert_eq!(std::fs::read(&path).unwrap(), raw);
        }
    }
}

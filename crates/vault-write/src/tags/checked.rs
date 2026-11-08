use std::path::Path;

use hypr_loofah_protocol::MetadataSaveOutcome;
use serde::Deserialize;

use super::{TagItem, normalize_tag_name};
use crate::{SessionStore, StoreError, metadata_revision::validate_metadata_size, paths};

impl SessionStore {
    pub async fn accept_tag_suggestion_with_recovery(
        &self,
        operation_root: Option<&Path>,
        id: &str,
        name: &str,
        expected_revision: &str,
    ) -> Result<Option<MetadataSaveOutcome>, StoreError> {
        self.checked_tag_suggestion(operation_root, id, name, expected_revision, true)
            .await
    }

    pub async fn dismiss_tag_suggestion_with_recovery(
        &self,
        operation_root: Option<&Path>,
        id: &str,
        name: &str,
        expected_revision: &str,
    ) -> Result<Option<MetadataSaveOutcome>, StoreError> {
        self.checked_tag_suggestion(operation_root, id, name, expected_revision, false)
            .await
    }

    async fn checked_tag_suggestion(
        &self,
        operation_root: Option<&Path>,
        id: &str,
        name: &str,
        expected_revision: &str,
        accept: bool,
    ) -> Result<Option<MetadataSaveOutcome>, StoreError> {
        let name = normalize_tag_name(name)
            .ok_or_else(|| StoreError::Io("tag name cannot be empty".into()))?;
        let expected_revision = expected_revision.to_owned();
        self.recovered_read(operation_root, id, move |store, snapshot, id| async move {
            store.ensure_ready()?;
            let guard = &snapshot._guard;
            let Some(current) = store.metadata_snapshot_locked(guard, &id).await? else {
                return Ok(None);
            };
            if current.revision != expected_revision {
                return Ok(Some(MetadataSaveOutcome::Conflict(current)));
            }
            let mut meta = current.meta.clone();
            let Some(state) = &mut meta.tag_suggestions else {
                return Ok(Some(MetadataSaveOutcome::Saved(current)));
            };
            let before = state.items.len();
            state.items.retain(|suggestion| suggestion != &name);
            if state.items.len() == before {
                return Ok(Some(MetadataSaveOutcome::Saved(current)));
            }
            if accept {
                if !meta
                    .tags
                    .iter()
                    .filter_map(|tag| normalize_tag_name(tag))
                    .any(|tag| tag == name)
                {
                    meta.tags.push(name.clone());
                    meta.tags.sort();
                }
            } else if !state.dismissed.contains(&name) {
                state.dismissed.push(name.clone());
                state.dismissed.sort();
            }
            validate_metadata_size(&meta, store.content_limits.metadata_bytes)?;
            let metadata = store.checked_tag_metadata(&meta, accept).await?;
            let registration = if accept {
                store.checked_tag_registration(&name).await?
            } else {
                None
            };
            // Registry I/O can overlap an external metadata edit despite the app's lock.
            let Some(latest) = store.metadata_snapshot_locked(guard, &id).await? else {
                return Ok(None);
            };
            if latest.revision != expected_revision {
                return Ok(Some(MetadataSaveOutcome::Conflict(latest)));
            }
            let registered = registration.is_some();
            if let Some((bytes, tag)) = registration {
                store
                    .write_file_locked(guard, paths::tags_path(), bytes)
                    .await?;
                store.index_upsert_tag(&tag);
                store.notify_index_changed(crate::IndexEntity::Tags, vec![tag.id]);
            }
            let latest = store
                .metadata_snapshot_locked(guard, &id)
                .await
                .map_err(|error| {
                    if registered {
                        error.after_commit()
                    } else {
                        error
                    }
                })?
                .ok_or_else(|| {
                    StoreError::Io("Session disappeared after tag registration".into())
                })?;
            if latest.revision != expected_revision {
                return Ok(Some(MetadataSaveOutcome::Conflict(latest)));
            }
            let directory = paths::validated_session_dir(&id)?;
            store
                .write_file_locked(guard, paths::meta_path_in(&directory), metadata)
                .await?;
            store.index_upsert_meta(&meta);
            store.notify_index_changed(crate::IndexEntity::Sessions, vec![id.clone()]);
            let saved = store
                .metadata_snapshot_locked(guard, &id)
                .await
                .map_err(StoreError::after_commit)?
                .ok_or_else(|| StoreError::Io("Session disappeared after tag update".into()))?;
            Ok(Some(MetadataSaveOutcome::Saved(saved)))
        })
        .await
    }

    async fn checked_tag_metadata(
        &self,
        meta: &crate::SessionMeta,
        accept: bool,
    ) -> Result<Vec<u8>, StoreError> {
        let directory = paths::validated_session_dir(&meta.id)?;
        let path = self.vault_base.join(paths::meta_path_in(&directory));
        let meta = meta.clone();
        let limit = self.content_limits.metadata_bytes;
        tokio::task::spawn_blocking(move || {
            let mut value = read_json(&path, limit)?
                .ok_or_else(|| StoreError::Io("Session metadata disappeared".into()))?;
            let object = value.as_object_mut().ok_or_else(|| {
                StoreError::Serialize("Session metadata must be an object".into())
            })?;
            if accept {
                object.insert(
                    "tags".into(),
                    serde_json::to_value(meta.tags).map_err(serialize_error)?,
                );
            }
            let state = object
                .get_mut("tag_suggestions")
                .and_then(serde_json::Value::as_object_mut)
                .ok_or_else(|| StoreError::Serialize("Tag suggestions must be an object".into()))?;
            let changed = meta
                .tag_suggestions
                .ok_or_else(|| StoreError::Serialize("Tag suggestions disappeared".into()))?;
            state.insert(
                "items".into(),
                serde_json::to_value(changed.items).map_err(serialize_error)?,
            );
            if !accept {
                state.insert(
                    "dismissed".into(),
                    serde_json::to_value(changed.dismissed).map_err(serialize_error)?,
                );
            }
            let bytes = serde_json::to_vec_pretty(&value).map_err(serialize_error)?;
            if bytes.len() > limit {
                return Err(StoreError::ResourceLimit(
                    "Session metadata exceeds its size limit".into(),
                ));
            }
            Ok(bytes)
        })
        .await
        .map_err(|error| StoreError::Io(format!("tag metadata worker failed: {error}")))?
    }

    async fn checked_tag_registration(
        &self,
        name: &str,
    ) -> Result<Option<(Vec<u8>, TagItem)>, StoreError> {
        let path = self.vault_base.join(paths::tags_path());
        let name = name.to_owned();
        let limit = self.content_limits.metadata_bytes;
        tokio::task::spawn_blocking(move || {
            let mut value =
                read_json(&path, limit)?.unwrap_or_else(|| serde_json::json!({"tags":[]}));
            let object = value
                .as_object_mut()
                .ok_or_else(|| StoreError::Serialize("Tag registry must be an object".into()))?;
            let tags = object
                .entry("tags")
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .ok_or_else(|| {
                    StoreError::Serialize("Tag registry entries must be an array".into())
                })?;
            let mut ids = std::collections::HashSet::new();
            for raw in tags.iter() {
                let tag: TagItem = serde_json::from_value(raw.clone())
                    .map_err(|error| StoreError::Serialize(error.to_string()))?;
                if !ids.insert(tag.id.clone()) {
                    return Err(StoreError::Serialize(
                        "Duplicate tag registry identifier".into(),
                    ));
                }
            }
            if ids.contains(&name) {
                return Ok(None);
            }
            let tag = TagItem {
                id: name.clone(),
                name,
            };
            tags.push(
                serde_json::to_value(&tag)
                    .map_err(|error| StoreError::Serialize(error.to_string()))?,
            );
            let bytes = serde_json::to_vec_pretty(&value)
                .map_err(|error| StoreError::Serialize(error.to_string()))?;
            if bytes.len() > limit {
                return Err(registry_limit());
            }
            Ok(Some((bytes, tag)))
        })
        .await
        .map_err(|error| StoreError::Io(format!("tag registration worker failed: {error}")))?
    }
}

fn registry_limit() -> StoreError {
    StoreError::ResourceLimit("Tag registry exceeds its size limit".into())
}
fn io_error(error: std::io::Error) -> StoreError {
    StoreError::Io(error.to_string())
}

fn serialize_error(error: serde_json::Error) -> StoreError {
    StoreError::Serialize(error.to_string())
}

fn read_json(path: &Path, limit: usize) -> Result<Option<serde_json::Value>, StoreError> {
    use std::io::Read;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(StoreError::Io(
                "Tag editing source is not a regular file".into(),
            ));
        }
        Ok(meta) if meta.len() > limit as u64 => {
            return Err(StoreError::ResourceLimit(
                "Tag editing source exceeds its size limit".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
        _ => {}
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(io_error)?
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > limit {
        return Err(StoreError::ResourceLimit(
            "Tag editing source exceeds its size limit".into(),
        ));
    }
    serde_json::from_slice::<UniqueValue>(&bytes)
        .map(|value| Some(value.0))
        .map_err(serialize_error)
}

struct UniqueValue(serde_json::Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|n| UniqueValue(n.into()))
                    .ok_or_else(|| E::custom("Invalid JSON number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(serde_json::Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(values.into()))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("Duplicate JSON object key"));
                    }
                    values.insert(key, map.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(values.into()))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod tests;

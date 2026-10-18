use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, atomic::Ordering};

use hypr_vault_access::{
    Call, Error, Reply, Result, User, Versions,
    filesystem::{Change, VaultStorage, safe_path},
};
use serde_json::{Value, json};

use crate::{Server, VaultRuntime, dispatch};

pub fn store_error(error: hypr_vault_write::StoreError) -> Error {
    let code = if matches!(error, hypr_vault_write::StoreError::Conflict(_)) {
        "conflict"
    } else {
        "storage"
    };
    Error::new(code, error.to_string())
}

pub async fn recover(runtime: &VaultRuntime) -> Result<()> {
    if runtime.failed.load(Ordering::Acquire) {
        let _view = runtime.view_gate.write().await;
        let storage = runtime.storage.clone();
        tokio::task::spawn_blocking(move || storage.recover())
            .await
            .map_err(|error| Error::new("storage", error.to_string()))??;
        let report = runtime.store.rebuild_index().await.map_err(store_error)?;
        if !report.errors.is_empty() {
            return Err(Error::new(
                "invalid_data",
                "recovered vault could not be indexed",
            ));
        }
        *runtime.headers.write().unwrap() = runtime
            .store
            .session_list_headers()
            .into_iter()
            .map(|header| (header.id.clone(), header))
            .collect();
        let keys = document_keys(&runtime.storage.content, &runtime.store.session_ids())?
            .into_iter()
            .filter(|key| !key.ends_with("transcript.json"))
            .collect::<Vec<_>>();
        *runtime.revisions.write().unwrap() = runtime.storage.versions(&keys)?;
        runtime.index_changed(runtime.store.session_ids());
        runtime.failed.store(false, Ordering::Release);
    }
    Ok(())
}

pub fn document_keys(root: &Path, sessions: &[String]) -> Result<Vec<String>> {
    let mut keys = vec![
        "tags.json".into(),
        "people.json".into(),
        "tasks.json".into(),
    ];
    for id in sessions {
        hypr_vault_access::validate_id(id)?;
        for name in [
            "_meta.json",
            "notes.md",
            "_memo.md",
            "summary.md",
            "transcript.json",
            "tasks.json",
        ] {
            keys.push(format!("sessions/{id}/{name}"));
        }
        let prefix = format!("sessions/{id}/enhanced");
        match std::fs::read_dir(safe_path(root, &prefix)?) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') || !name.ends_with(".md") {
                        continue;
                    }
                    let key = format!("{prefix}/{name}");
                    let path = safe_path(root, &key)?;
                    if path.is_file() {
                        keys.push(key);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}

async fn affected(runtime: &VaultRuntime, call: &Call) -> Result<Vec<String>> {
    let args = &call.args;
    let mut ids = BTreeSet::new();
    for id in [
        args["session_id"].as_str(),
        args["meta"]["id"].as_str(),
        args["doc"]["session_id"].as_str(),
    ]
    .into_iter()
    .flatten()
    {
        hypr_vault_access::validate_id(id)?;
        ids.insert(id.to_string());
    }
    if let Some(id) = args["source_id"].as_str() {
        if let Some(doc) = runtime.store.enhanced_doc_get(id) {
            ids.insert(doc.session_id);
        } else if runtime.store.session_get(id).is_some() {
            ids.insert(id.into());
        }
    }
    if let Some(id) = args["doc_id"].as_str() {
        hypr_vault_access::validate_id(id)?;
        if let Some(doc) = runtime.store.enhanced_doc_get(id) {
            ids.insert(doc.session_id);
        }
    }
    if let Some(id) = args["transcript_id"].as_str()
        && let Some(transcript) = runtime
            .store
            .transcript_get(id)
            .await
            .map_err(store_error)?
    {
        ids.insert(transcript.session_id);
    }
    if call.operation == "session_move_tasks" {
        ids.extend(runtime.store.session_ids());
    }
    Ok(ids.into_iter().collect())
}

fn snapshot(
    root: &Path,
    target: &Path,
    sessions: &[String],
    include_transcript: bool,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut result = BTreeMap::new();
    for key in document_keys(root, sessions)? {
        if key.ends_with("transcript.json") && !include_transcript {
            continue;
        }
        let source = safe_path(root, &key)?;
        let bytes = match std::fs::read(&source) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let destination = safe_path(target, &key)?;
        if key.ends_with(".json") {
            canonical_json(&key, &bytes)?;
        }
        std::fs::create_dir_all(destination.parent().unwrap())?;
        std::fs::write(destination, &bytes)?;
        result.insert(key, bytes);
    }
    Ok(result)
}

fn canonical_json(key: &str, bytes: &[u8]) -> Result<Value> {
    fn roundtrip<T: serde::de::DeserializeOwned + serde::Serialize>(bytes: &[u8]) -> Result<Value> {
        Ok(serde_json::to_value(
            serde_json::from_slice::<T>(bytes)
                .map_err(|error| Error::new("invalid_data", error.to_string()))?,
        )?)
    }
    #[derive(serde::Serialize, serde::Deserialize)]
    struct People {
        people: Vec<hypr_vault_write::PersonItem>,
    }
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Tags {
        tags: Vec<hypr_vault_write::TagItem>,
    }
    if key == "people.json" {
        roundtrip::<People>(bytes)
    } else if key == "tags.json" {
        roundtrip::<Tags>(bytes)
    } else if key.ends_with("tasks.json") {
        roundtrip::<hypr_vault_read::TasksFile>(bytes)
    } else if key.ends_with("transcript.json") {
        roundtrip::<hypr_fs_format::TranscriptJson>(bytes)
    } else {
        roundtrip::<hypr_vault_read::SessionMeta>(bytes)
    }
}

fn retain_extensions(original: &Value, known: &Value, updated: &mut Value) {
    match (original, known, updated) {
        (Value::Object(original), Value::Object(known), Value::Object(updated)) => {
            for (key, value) in original {
                if let Some(shape) = known.get(key) {
                    if let Some(next) = updated.get_mut(key) {
                        retain_extensions(value, shape, next);
                    }
                } else {
                    updated.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        (Value::Array(original), Value::Array(known), Value::Array(updated)) => {
            for (value, shape) in original.iter().zip(known) {
                let identity = |candidate: &Value| {
                    if let Some(id) = value["id"].as_str() {
                        candidate["id"] == id
                    } else if value.get("start_ms").is_some() {
                        ["start_ms", "end_ms", "channel", "text"]
                            .iter()
                            .all(|key| candidate[key] == value[key])
                    } else {
                        candidate == shape
                    }
                };
                if let Some(next) = updated.iter_mut().find(|candidate| identity(candidate)) {
                    retain_extensions(value, shape, next);
                }
            }
        }
        _ => {}
    }
}

pub async fn invoke(server: Arc<Server>, user: &User, vault_id: &str, call: Call) -> Result<Reply> {
    let query = dispatch::is_query(&call.operation);
    server.authorize(user, vault_id, !query, false).await?;
    let runtime = server.runtime(vault_id).await?;
    if runtime.failed.load(Ordering::Acquire) {
        let _gate = runtime.gate.write().await;
        recover(&runtime).await?;
    }
    if matches!(
        call.operation.as_str(),
        "session_get"
            | "session_list_headers"
            | "session_ids"
            | "session_has_transcript"
            | "session_enhanced_docs"
            | "enhanced_doc_get"
            | "session_transcript_metadata"
            | "session_find_by_tracking_id"
            | "session_summary_get"
    ) {
        // Content and revisions share a publication gate, so an old editor can never save using a newer unseen revision.
        let _view = runtime.view_gate.read().await;
        server.authorize(user, vault_id, false, false).await?;
        let ids = affected(&runtime, &call).await?;
        let data = if call.operation == "session_list_headers" {
            header_page(&runtime, &call)?
        } else {
            dispatch::execute(&runtime.store, &call.operation, &call.args).await?
        };
        let revisions = runtime.revisions.read().unwrap();
        let mut versions = Versions::new();
        for key in ["people.json", "tags.json", "tasks.json"] {
            versions.insert(key.into(), revisions.get(key).cloned().flatten());
        }
        for id in ids {
            let prefix = format!("sessions/{id}/");
            for (key, value) in revisions
                .range(prefix.clone()..)
                .take_while(|(key, _)| key.starts_with(&prefix))
            {
                versions.insert(key.clone(), value.clone());
            }
        }
        return Ok(Reply { data, versions });
    }
    if query {
        let _gate = runtime.gate.read().await;
        server.authorize(user, vault_id, false, false).await?;
        if runtime.failed.load(Ordering::Acquire) {
            return Err(Error::new("commit_pending", "vault recovery is required"));
        }
        let ids = affected(&runtime, &call).await?;
        let data = if call.operation == "search" {
            if !runtime.search_ready.load(Ordering::Acquire) {
                return Err(Error::new("indexing", "search is being prepared"));
            }
            let cache = runtime.search.clone();
            let request = dispatch::arg(&call.args, "request")?;
            tokio::task::spawn_blocking(move || {
                let result = cache
                    .search(request)
                    .map_err(|error| Error::new("storage", error.to_string()))?;
                Ok::<_, Error>(serde_json::to_value(result)?)
            })
            .await
            .map_err(|error| Error::new("storage", error.to_string()))??
        } else if call.operation == "session_list_headers" {
            header_page(&runtime, &call)?
        } else {
            dispatch::execute(&runtime.store, &call.operation, &call.args).await?
        };
        let storage = runtime.storage.clone();
        let include_transcript = call.operation.contains("transcript");
        let versions = tokio::task::spawn_blocking(move || {
            storage.versions(
                &document_keys(&storage.content, &ids)?
                    .into_iter()
                    .filter(|key| include_transcript || !key.ends_with("transcript.json"))
                    .collect::<Vec<_>>(),
            )
        })
        .await
        .map_err(|error| Error::new("storage", error.to_string()))??;
        return Ok(Reply { data, versions });
    }
    let mutation_id = call
        .mutation_id
        .as_deref()
        .ok_or_else(|| Error::new("invalid_request", "mutation_id is required"))?;
    hypr_vault_access::validate_id(mutation_id)?;
    let operation_id = format!("{}_{mutation_id}", user.id);
    let fingerprint = dispatch::fingerprint(&user.id, &call)?;
    let _gate = runtime.gate.write().await;
    server.authorize(user, vault_id, true, false).await?;
    recover(&runtime).await?;
    if let Some(reply) = runtime
        .storage
        .operation(&operation_id, Some(&fingerprint))?
    {
        return Ok(reply);
    }
    let ids = affected(&runtime, &call).await?;
    for id in &ids {
        if runtime.storage.deleted(id)? && call.operation != "session_restore" {
            return Err(Error::new("conflict", "session was deleted"));
        }
    }
    let (changes, data) = if call.operation == "artifact_remove" {
        let session = dispatch::text(&call.args, "session_id")?;
        hypr_vault_access::validate_id(session)?;
        let key = dispatch::text(&call.args, "key")?;
        let suffix = key
            .strip_prefix(&format!("sessions/{session}/"))
            .ok_or_else(|| Error::new("invalid_request", "artifact must belong to the session"))?;
        if let Some(name) = suffix.strip_prefix("attachments/") {
            if name.is_empty() || name.starts_with('.') || name.contains('/') || name.contains('\\')
            {
                return Err(Error::new("invalid_request", "invalid managed attachment"));
            }
        } else if !["audio.mp3", "audio.wav", "audio.ogg", "audio.peaks.json"].contains(&suffix) {
            return Err(Error::new("invalid_request", "unsupported artifact"));
        }
        if !call.expected.contains_key(key) {
            return Err(Error::new(
                "precondition_required",
                "read the artifact before deleting it",
            ));
        }
        (vec![Change::Remove(key.into())], Value::Null)
    } else if call.operation == "session_delete" || call.operation == "session_restore" {
        let id = dispatch::text(&call.args, "session_id")?;
        hypr_vault_access::validate_id(id)?;
        if call.operation == "session_delete" {
            if runtime.store.session_get(id).is_none() {
                return Err(Error::new("not_found", "session not found"));
            }
            if !call
                .expected
                .contains_key(&format!("sessions/{id}/_meta.json"))
            {
                return Err(Error::new(
                    "precondition_required",
                    "read the session before deleting it",
                ));
            }
            (vec![Change::DeleteSession(id.into())], Value::Null)
        } else {
            if !runtime.storage.deleted(id)? {
                return Ok(Reply {
                    data: json!(false),
                    versions: Versions::new(),
                });
            }
            (vec![Change::RestoreSession(id.into())], json!(true))
        }
    } else {
        let temporary = tempfile::tempdir_in(runtime.storage.state.parent().unwrap())?;
        let source = runtime.storage.content.clone();
        let target = temporary.path().to_path_buf();
        let snapshot_ids = ids.clone();
        let include_transcript = call.operation.contains("transcript");
        let before = tokio::task::spawn_blocking(move || {
            snapshot(&source, &target, &snapshot_ids, include_transcript)
        })
        .await
        .map_err(|error| Error::new("storage", error.to_string()))??;
        let staged = hypr_vault_write::SessionStore::new(temporary.path().to_path_buf());
        let report = staged.rebuild_index().await.map_err(store_error)?;
        if !report.errors.is_empty() {
            return Err(Error::new(
                "invalid_data",
                "session snapshot contains invalid data",
            ));
        }
        let data = dispatch::execute(&staged, &call.operation, &call.args).await?;
        staged.flush_all().await.map_err(store_error)?;
        let after = document_keys(temporary.path(), &ids)?
            .into_iter()
            .map(|key| {
                let bytes = std::fs::read(safe_path(temporary.path(), &key)?);
                match bytes {
                    Ok(bytes) => Ok(Some((key, bytes))),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(Error::from(error)),
                }
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<BTreeMap<_, _>>();
        let mut changes = Vec::new();
        for key in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
            if before.get(key) == after.get(key) {
                continue;
            }
            // Derived task/registry updates use the current serialized state; document replacement requires a client revision.
            let primary = key.ends_with("notes.md")
                || key.ends_with("summary.md")
                || key.contains("/enhanced/")
                || key.ends_with("transcript.json")
                || (call.operation == "session_write_meta" && key.ends_with("_meta.json"))
                || call.operation == "session_replace_tasks";
            if primary
                && !dispatch::is_atomic(&call.operation)
                && before.contains_key(key)
                && !call.expected.contains_key(key)
            {
                return Err(Error::new(
                    "precondition_required",
                    format!("read {key} before replacing it"),
                ));
            }
            match after.get(key) {
                Some(bytes) => {
                    let mut bytes = bytes.clone();
                    if key.ends_with(".json")
                        && !key.ends_with("_meta.json")
                        && let Some(previous) = before.get(key)
                    {
                        let original = serde_json::from_slice(previous)?;
                        let mut updated = serde_json::from_slice(&bytes)?;
                        retain_extensions(&original, &canonical_json(key, previous)?, &mut updated);
                        bytes = serde_json::to_vec_pretty(&updated)?;
                    }
                    changes.push(Change::Write(key.clone(), bytes));
                }
                None => changes.push(Change::Remove(key.clone())),
            }
        }
        (changes, data)
    };
    let storage = runtime.storage.clone();
    let mut touched = BTreeSet::new();
    for change in &changes {
        match change {
            Change::Write(key, _) | Change::Publish(key, _, _) | Change::Remove(key) => {
                touched.insert(key.clone());
            }
            Change::DeleteSession(id) => {
                touched.insert(format!("sessions/{id}/_meta.json"));
            }
            Change::RestoreSession(_) => {}
        }
    }
    let expected = call
        .expected
        .into_iter()
        .filter(|(key, _)| touched.contains(key))
        .collect();
    let result = tokio::task::spawn_blocking(move || {
        storage.commit(&operation_id, &fingerprint, &expected, changes, data)
    })
    .await
    .map_err(|error| Error::new("storage", error.to_string()))?;
    if result
        .as_ref()
        .is_err_and(|error| error.code == "commit_pending")
    {
        runtime.failed.store(true, Ordering::Release);
    }
    let reply = result?;
    runtime.failed.store(true, Ordering::Release);
    let _view = runtime.view_gate.write().await;
    let mut headers_changed = false;
    for id in &ids {
        runtime
            .store
            .refresh_session(id)
            .await
            .map_err(store_error)?;
        let header =
            runtime
                .store
                .session_get(id)
                .map(|record| hypr_vault_write::SessionListHeader {
                    id: id.clone(),
                    title: record.meta.title,
                    created_at: record.meta.created_at,
                    folder: record.meta.folder,
                    tags: record.meta.tags,
                    author: record.meta.author,
                    has_transcript_words: runtime.store.session_has_transcript(id),
                });
        let mut headers = runtime.headers.write().unwrap();
        if headers.get(id) != header.as_ref() {
            headers_changed = true;
        }
        if let Some(header) = header {
            headers.insert(id.clone(), header);
        } else {
            headers.remove(id);
        }
    }
    runtime.store.index_refresh_people().await;
    runtime.store.index_refresh_tags().await;
    runtime
        .revisions
        .write()
        .unwrap()
        .extend(reply.versions.clone());
    if matches!(
        call.operation.as_str(),
        "session_delete" | "session_restore"
    ) {
        let keys = document_keys(&runtime.storage.content, &ids)?
            .into_iter()
            .filter(|key| !key.ends_with("transcript.json"))
            .collect::<Vec<_>>();
        runtime
            .revisions
            .write()
            .unwrap()
            .extend(runtime.storage.versions(&keys)?);
    }
    runtime.failed.store(false, Ordering::Release);
    runtime.index_changed(ids.clone());
    let mut entities = BTreeSet::new();
    for key in reply.versions.keys() {
        let entity = if key == "people.json" {
            "people"
        } else if key == "tags.json" {
            "tags"
        } else if key.ends_with("tasks.json") {
            "tasks"
        } else if key.ends_with("transcript.json") {
            "transcripts"
        } else if key.ends_with("summary.md") || key.contains("/enhanced/") {
            "docs"
        } else if key.contains("/attachments/") || key.contains("/audio.") {
            "artifacts"
        } else {
            "sessions"
        };
        entities.insert(entity);
    }
    if matches!(
        call.operation.as_str(),
        "session_delete" | "session_restore"
    ) {
        entities.extend(["sessions", "docs", "transcripts", "tasks", "artifacts"]);
    }
    if headers_changed {
        entities.insert("session_headers");
    }
    for entity in entities {
        server.notify(Some(vault_id.into()), entity, ids.clone());
    }
    Ok(reply)
}

fn header_page(runtime: &VaultRuntime, call: &Call) -> Result<Value> {
    let after = call.args["after"].as_str().unwrap_or("");
    let limit = call.args["limit"].as_u64().unwrap_or(200).clamp(1, 500) as usize;
    let headers = runtime.headers.read().unwrap();
    let values = headers
        .range((
            std::ops::Bound::Excluded(after.to_string()),
            std::ops::Bound::Unbounded,
        ))
        .take(limit)
        .map(|(_, header)| header)
        .collect::<Vec<_>>();
    Ok(serde_json::to_value(values)?)
}

#[cfg(test)]
mod tests;

use super::*;
use crate::submissions::{check_file, lock, prepare_directory, scope_key};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
const MAX_BYTES: u64 = 16 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct MediaSubmission {
    pub schema_version: u32,
    pub scope: MediaScope,
    pub declaration: MediaDeclaration,
    pub admission_revision: Option<String>,
}
pub struct MediaSubmissions {
    root: PathBuf,
    io: Arc<tokio::sync::Semaphore>,
}
impl MediaSubmissions {
    pub fn new(root: PathBuf) -> Result<Arc<Self>> {
        if !root.is_absolute()
            || root
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
        {
            return Err(invalid("Media receipt storage must be absolute"));
        }
        Ok(Arc::new(Self {
            root,
            io: Arc::new(tokio::sync::Semaphore::new(2)),
        }))
    }
    pub async fn prepare(
        self: &Arc<Self>,
        scope: MediaScope,
        declaration: MediaDeclaration,
    ) -> Result<(MediaSubmission, bool)> {
        validate_media_declaration(&declaration).map_err(invalid)?;
        let submission = MediaSubmission {
            schema_version: 1,
            scope: scope.clone(),
            declaration,
            admission_revision: None,
        };
        self.run(scope, move |directory| {
            prepare_directory(directory)?;
            let _lock = lock(&directory.join(".lock"))?;
            let path = directory.join(format!("{}.json", submission.declaration.request_id));
            if let Some(existing) = read(&path)? {
                if existing.scope != submission.scope
                    || existing.declaration != submission.declaration
                {
                    return Err(failure(
                        ErrorCode::Conflict,
                        "Original media request already contains different input",
                    ));
                }
                return Ok((existing, false));
            }
            if std::fs::read_dir(directory).map_err(io)?.take(4097).count() > 4096 {
                return Err(failure(
                    ErrorCode::ResourceLimit,
                    "Media receipt inventory is full; existing requests were retained",
                ));
            }
            let bytes = serde_json::to_string(&submission).map_err(io)?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(failure(
                    ErrorCode::ResourceLimit,
                    "Media declaration exceeds its receipt budget",
                ));
            }
            check_file(&path)?;
            hypr_storage::fs::atomic_write(&path, &bytes).map_err(io)?;
            sync(directory)?;
            Ok((submission, true))
        })
        .await
    }
    pub async fn observe_admission(
        self: &Arc<Self>,
        submission: &MediaSubmission,
        admission: &MediaAdmission,
    ) -> Result<MediaSubmission> {
        validate_status(&admission.status)?;
        if admission.declaration != submission.declaration
            || admission.status.declaration != submission.declaration
            || admission.revision != admission.status.revision
        {
            return Err(bad_reply("Original media admission changed"));
        }
        let submission = submission.clone();
        let revision = admission.revision.clone();
        self.run(submission.scope.clone(), move |directory| {
            let _lock = lock(&directory.join(".lock"))?;
            let path = directory.join(format!("{}.json", submission.declaration.request_id));
            let mut original = read(&path)?.ok_or_else(damaged)?;
            if original.scope != submission.scope
                || original.declaration != submission.declaration
                || original
                    .admission_revision
                    .as_ref()
                    .is_some_and(|old| old != &revision)
            {
                return Err(damaged());
            }
            if original.admission_revision.is_none() {
                original.admission_revision = Some(revision);
                let bytes = serde_json::to_string(&original).map_err(io)?;
                if bytes.len() as u64 > MAX_BYTES {
                    return Err(damaged());
                }
                check_file(&path)?;
                hypr_storage::fs::atomic_write(&path, &bytes).map_err(io)?;
                sync(directory)?;
            }
            Ok(original)
        })
        .await
    }
    pub async fn load(
        self: &Arc<Self>,
        scope: MediaScope,
        request: String,
    ) -> Result<Option<MediaSubmission>> {
        uuid(&request)?;
        self.run(scope.clone(), move |directory| {
            let value = read(&directory.join(format!("{request}.json")))?;
            if value
                .as_ref()
                .is_some_and(|v| v.scope != scope || v.declaration.request_id != request)
            {
                return Err(damaged());
            }
            Ok(value)
        })
        .await
    }
    pub async fn list(self: &Arc<Self>, scope: MediaScope) -> Result<Vec<MediaSubmission>> {
        self.run(scope.clone(), move |directory| {
            if !exists_directory(directory)? {
                return Ok(vec![]);
            }
            let mut values = Vec::new();
            let mut bytes = 0u64;
            for (count, entry) in std::fs::read_dir(directory).map_err(io)?.enumerate() {
                if count > 4096 {
                    return Err(damaged());
                }
                let entry = entry.map_err(io)?;
                let name = entry.file_name().into_string().map_err(|_| damaged())?;
                if name == ".lock" {
                    continue;
                }
                let id = name.strip_suffix(".json").ok_or_else(damaged)?;
                uuid(id)?;
                bytes = bytes
                    .checked_add(entry.metadata().map_err(io)?.len())
                    .filter(|n| *n <= 64 * 1024 * 1024)
                    .ok_or_else(damaged)?;
                let value = read(&entry.path())?.ok_or_else(damaged)?;
                if value.scope != scope || value.declaration.request_id != id {
                    return Err(damaged());
                }
                values.push(value);
            }
            values.sort_by(|a, b| a.declaration.request_id.cmp(&b.declaration.request_id));
            Ok(values)
        })
        .await
    }
    async fn run<T: Send + 'static>(
        self: &Arc<Self>,
        scope: MediaScope,
        action: impl FnOnce(&Path) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        validate_scope(&scope)?;
        let scope = serde_json::to_string(&scope).map_err(io)?;
        let permit = self
            .io
            .clone()
            .try_acquire_owned()
            .map_err(|_| failure(ErrorCode::Unavailable, "Media receipt storage is busy"))?;
        let owner = self.clone();
        tokio::task::spawn_blocking(move || {
            let path = owner.root.join(scope_key(&scope));
            exists_directory(&path)?;
            let result = action(&path);
            drop(owner);
            drop(permit);
            result
        })
        .await
        .map_err(io)?
    }
}
fn read(path: &Path) -> Result<Option<MediaSubmission>> {
    check_file(path)?;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(e)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(damaged());
    }
    let raw: serde_json::Value = serde_json::from_slice(&bytes).map_err(io)?;
    let value: MediaSubmission = serde_json::from_value(raw.clone()).map_err(io)?;
    if value.schema_version != 1 || serde_json::to_value(&value).map_err(io)? != raw {
        return Err(damaged());
    }
    if value
        .admission_revision
        .as_ref()
        .is_some_and(|revision| !valid_revision(revision))
    {
        return Err(damaged());
    }
    validate_scope(&value.scope)?;
    validate_media_declaration(&value.declaration).map_err(|_| damaged())?;
    Ok(Some(value))
}
fn validate_scope(scope: &MediaScope) -> Result<()> {
    uuid(&scope.profile_id)?;
    let server = url::Url::parse(&scope.server).map_err(|_| damaged())?;
    hypr_loofah_protocol::validate_endpoint(&server, false).map_err(|_| damaged())?;
    if server.path() != "/"
        || server.query().is_some()
        || scope.member_id.is_empty()
        || scope.member_id.len() > 512
        || scope.vault_id.is_empty()
        || scope.vault_id.len() > 512
        || scope
            .member_id
            .chars()
            .chain(scope.vault_id.chars())
            .any(char::is_control)
    {
        return Err(damaged());
    }
    Ok(())
}
fn exists_directory(path: &Path) -> Result<bool> {
    for part in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match std::fs::symlink_metadata(part) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => return Err(damaged()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(io(e)),
        }
    }
    Ok(true)
}
fn sync(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path).and_then(|f| f.sync_all()).map_err(io)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn io(_: impl std::fmt::Display) -> ApplicationError {
    damaged()
}
fn damaged() -> ApplicationError {
    failure(
        ErrorCode::Unavailable,
        "Original media request storage requires review; its files were retained",
    )
}

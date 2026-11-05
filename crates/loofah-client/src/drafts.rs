use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use hypr_loofah_protocol::{ApplicationError, ErrorCode, NoteSnapshot};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::{Result, failure};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct DraftText {
    pub markdown: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NoteDraftRecovery {
    pub scope: String,
    pub session_id: String,
    pub baseline: NoteSnapshot,
    pub draft: DraftText,
    pub uncertain: bool,
}

#[derive(Serialize, Deserialize)]
struct StoredDraft {
    schema_version: u32,
    editor_id: String,
    recovery: NoteDraftRecovery,
}

pub struct DraftCache {
    root: PathBuf,
    gate: Arc<Mutex<()>>,
    record_limit: usize,
    scope_limit: u64,
}

impl DraftCache {
    pub fn new(root: PathBuf) -> Result<Self> {
        if !root.is_absolute() {
            return Err(failure(
                ErrorCode::Validation,
                "Draft cache root must be absolute",
            ));
        }
        Ok(Self {
            root,
            gate: Arc::new(Mutex::new(())),
            record_limit: 10 * 1024 * 1024,
            scope_limit: 128 * 1024 * 1024,
        })
    }

    pub async fn get(
        self: &Arc<Self>,
        editor_id: String,
        scope: String,
        session_id: String,
    ) -> Result<Option<NoteDraftRecovery>> {
        coordinate(&editor_id, &scope, &session_id)?;
        let cache = self.clone();
        tokio::task::spawn_blocking(move || {
            let path = cache.path(&editor_id, &scope, &session_id);
            cache.check_path(&path)?;
            cache.read(&path, &editor_id, &scope, &session_id)
        })
        .await
        .map_err(|_| worker_error())?
    }

    pub async fn put(
        self: &Arc<Self>,
        editor_id: String,
        recovery: NoteDraftRecovery,
    ) -> Result<()> {
        coordinate(&editor_id, &recovery.scope, &recovery.session_id)?;
        let cache = self.clone();
        let guard = self.gate.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let bytes = serde_json::to_string(&StoredDraft {
                schema_version: 1,
                editor_id: editor_id.clone(),
                recovery: recovery.clone(),
            })
            .map_err(|_| worker_error())?;
            if bytes.len() > cache.record_limit {
                return Err(limit("The draft exceeds the recovery size limit"));
            }
            let path = cache.path(&editor_id, &recovery.scope, &recovery.session_id);
            cache.prepare(&path)?;
            cache.read(&path, &editor_id, &recovery.scope, &recovery.session_id)?;
            let previous = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            let mut total = 0_u64;
            let mut files = 0;
            for entry in std::fs::read_dir(path.parent().unwrap()).map_err(io_error)? {
                let entry = entry.map_err(io_error)?;
                if !owned_filename(&entry.file_name().to_string_lossy()) {
                    continue;
                }
                if entry.file_type().map_err(io_error)?.is_symlink() {
                    return Err(damaged());
                }
                let metadata = entry.metadata().map_err(io_error)?;
                if !metadata.is_file() {
                    return Err(damaged());
                }
                total = total.saturating_add(metadata.len());
                files += 1;
            }
            if total
                .saturating_sub(previous)
                .saturating_add(bytes.len() as u64)
                > cache.scope_limit
                || (previous == 0 && files >= 512)
            {
                return Err(limit(
                    "Draft recovery storage is full. Existing drafts were kept.",
                ));
            }
            hypr_storage::fs::atomic_write(&path, &bytes).map_err(io_error)?;
            sync_parent(&path)?;
            Ok(())
        })
        .await
        .map_err(|_| worker_error())?
    }

    pub async fn remove(
        self: &Arc<Self>,
        editor_id: String,
        scope: String,
        session_id: String,
    ) -> Result<()> {
        coordinate(&editor_id, &scope, &session_id)?;
        let cache = self.clone();
        let guard = self.gate.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let path = cache.path(&editor_id, &scope, &session_id);
            cache.check_path(&path)?;
            if cache
                .read(&path, &editor_id, &scope, &session_id)?
                .is_some()
            {
                std::fs::remove_file(&path).map_err(io_error)?;
                sync_parent(&path)?;
            }
            Ok(())
        })
        .await
        .map_err(|_| worker_error())?
    }

    fn path(&self, editor_id: &str, scope: &str, session_id: &str) -> PathBuf {
        self.root.join(key(scope)).join(format!(
            "{}.json",
            key(&serde_json::to_string(&(session_id, editor_id)).unwrap())
        ))
    }

    fn check_path(&self, path: &Path) -> Result<()> {
        for candidate in [&self.root, path.parent().unwrap(), path] {
            match std::fs::symlink_metadata(candidate) {
                Ok(metadata) if metadata.file_type().is_symlink() => return Err(damaged()),
                Ok(metadata) if candidate == path && !metadata.is_file() => return Err(damaged()),
                Ok(metadata) if candidate != path && !metadata.is_dir() => return Err(damaged()),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(())
    }

    fn prepare(&self, path: &Path) -> Result<()> {
        self.check_path(path)?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(io_error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for directory in [&self.root, path.parent().unwrap()] {
                std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                    .map_err(io_error)?;
            }
        }
        self.check_path(path)
    }

    fn read(
        &self,
        path: &Path,
        editor_id: &str,
        scope: &str,
        session_id: &str,
    ) -> Result<Option<NoteDraftRecovery>> {
        match std::fs::metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
            Ok(metadata) if metadata.len() > self.record_limit as u64 => return Err(damaged()),
            Ok(_) => {}
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(io_error)?
            .take(self.record_limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > self.record_limit {
            return Err(damaged());
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| damaged())?;
        if value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
        {
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "This saved draft requires a different Loofah version. The file was kept.",
            ));
        }
        let stored: StoredDraft = serde_json::from_value(value).map_err(|_| damaged())?;
        if stored.editor_id != editor_id
            || stored.recovery.scope != scope
            || stored.recovery.session_id != session_id
        {
            return Err(damaged());
        }
        Ok(Some(stored.recovery))
    }
}

fn coordinate(editor_id: &str, scope: &str, session_id: &str) -> Result<()> {
    if editor_id.is_empty()
        || editor_id.len() > 512
        || scope.is_empty()
        || scope.len() > 4096
        || session_id.is_empty()
        || session_id.len() > 512
    {
        return Err(failure(
            ErrorCode::Validation,
            "Invalid draft account, vault or session scope",
        ));
    }
    Ok(())
}
fn key(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn owned_filename(name: &str) -> bool {
    name.strip_suffix(".json")
        .is_some_and(|name| name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
}
fn limit(message: &str) -> ApplicationError {
    failure(ErrorCode::ResourceLimit, message)
}
fn damaged() -> ApplicationError {
    failure(
        ErrorCode::Internal,
        "A saved draft could not be read safely. Its files were kept for recovery.",
    )
}
fn worker_error() -> ApplicationError {
    failure(ErrorCode::Internal, "The draft recovery worker stopped")
}
fn io_error(cause: std::io::Error) -> ApplicationError {
    tracing::error!(%cause, "device draft recovery I/O failed");
    failure(
        ErrorCode::Internal,
        "Cannot save or read the device recovery draft.",
    )
}
fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(path.parent().unwrap())
            .and_then(|parent| parent.sync_all())
            .map_err(io_error)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recovery(scope: &str, session_id: &str, body: &str) -> NoteDraftRecovery {
        NoteDraftRecovery {
            scope: scope.into(),
            session_id: session_id.into(),
            baseline: NoteSnapshot {
                markdown: Some("Original".into()),
                title: "Title".into(),
                revision: "baseline".into(),
            },
            draft: DraftText {
                markdown: body.into(),
                title: "Title".into(),
            },
            uncertain: true,
        }
    }

    #[tokio::test]
    async fn simultaneous_windows_keep_independent_recovery_drafts_for_the_same_note() {
        let root = tempfile::tempdir().unwrap();
        let cache = Arc::new(DraftCache::new(root.path().join("cache")).unwrap());
        let first = recovery("scope", "same", "First window draft");
        let second = recovery("scope", "same", "Second window draft");
        cache.put("main".into(), first.clone()).await.unwrap();
        cache
            .put("note-window".into(), second.clone())
            .await
            .unwrap();
        assert_eq!(
            cache
                .get("main".into(), "scope".into(), "same".into())
                .await
                .unwrap(),
            Some(first)
        );
        assert_eq!(
            cache
                .get("note-window".into(), "scope".into(), "same".into())
                .await
                .unwrap(),
            Some(second.clone())
        );
        cache
            .remove("main".into(), "scope".into(), "same".into())
            .await
            .unwrap();
        assert_eq!(
            cache
                .get("note-window".into(), "scope".into(), "same".into())
                .await
                .unwrap(),
            Some(second)
        );
    }

    #[tokio::test]
    async fn drafts_survive_restart_and_same_session_ids_remain_isolated_by_scope() {
        let root = tempfile::tempdir().unwrap();
        let cache = Arc::new(DraftCache::new(root.path().join("cache")).unwrap());
        for scope in [
            "server:alice:private",
            "server:bob:private",
            "server:alice:shared",
        ] {
            cache
                .put("main".into(), recovery(scope, "same", scope))
                .await
                .unwrap();
        }
        let reopened = Arc::new(DraftCache::new(root.path().join("cache")).unwrap());
        for scope in [
            "server:alice:private",
            "server:bob:private",
            "server:alice:shared",
        ] {
            assert_eq!(
                reopened
                    .get("main".into(), scope.into(), "same".into())
                    .await
                    .unwrap(),
                Some(recovery(scope, "same", scope))
            );
        }
        reopened
            .remove("main".into(), "server:alice:private".into(), "same".into())
            .await
            .unwrap();
        assert!(
            reopened
                .get("main".into(), "server:alice:private".into(), "same".into())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            reopened
                .get("main".into(), "server:bob:private".into(), "same".into())
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn untrusted_coordinates_are_hashed_and_damaged_or_future_drafts_are_not_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let cache = Arc::new(DraftCache::new(root.path().join("cache")).unwrap());
        let draft = recovery("../../account", "../../session", "Private text");
        cache.put("main".into(), draft.clone()).await.unwrap();
        let path = cache.path("main", &draft.scope, &draft.session_id);
        assert!(path.starts_with(root.path().join("cache")));
        for bytes in [
            b"{broken".as_slice(),
            b"{\"schema_version\":2,\"recovery\":{}}".as_slice(),
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(
                cache
                    .get("main".into(), draft.scope.clone(), draft.session_id.clone())
                    .await
                    .is_err()
            );
            assert!(cache.put("main".into(), draft.clone()).await.is_err());
            assert!(
                cache
                    .remove("main".into(), draft.scope.clone(), draft.session_id.clone())
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[tokio::test]
    async fn quotas_and_size_limits_preserve_the_existing_recovery_copy() {
        let root = tempfile::tempdir().unwrap();
        let mut cache = DraftCache::new(root.path().join("cache")).unwrap();
        cache.record_limit = 1024;
        cache.scope_limit = 1400;
        let cache = Arc::new(cache);
        let original = recovery("scope", "session", "Small draft");
        cache.put("main".into(), original.clone()).await.unwrap();
        assert_eq!(
            cache
                .put(
                    "main".into(),
                    recovery("scope", "session", &"x".repeat(2048))
                )
                .await
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimit
        );
        cache
            .put("main".into(), recovery("scope", "second", &"x".repeat(600)))
            .await
            .unwrap();
        assert_eq!(
            cache
                .put("main".into(), recovery("scope", "third", &"x".repeat(600)))
                .await
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimit
        );
        assert_eq!(
            cache
                .get("main".into(), "scope".into(), "session".into())
                .await
                .unwrap(),
            Some(original)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_cannot_redirect_a_recovery_read_write_or_delete() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let cache = Arc::new(DraftCache::new(root.path().join("cache")).unwrap());
        let draft = recovery("scope", "session", "Private text");
        cache.put("main".into(), draft.clone()).await.unwrap();
        let path = cache.path("main", &draft.scope, &draft.session_id);
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let victim = root.path().join("victim");
        std::fs::write(&victim, "Unrelated").unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(&victim, &path).unwrap();
        assert!(
            cache
                .get("main".into(), draft.scope.clone(), draft.session_id.clone())
                .await
                .is_err()
        );
        assert!(cache.put("main".into(), draft.clone()).await.is_err());
        assert!(
            cache
                .remove("main".into(), draft.scope, draft.session_id)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(victim).unwrap(), "Unrelated");
    }
}

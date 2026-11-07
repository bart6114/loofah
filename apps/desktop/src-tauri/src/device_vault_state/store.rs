use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

use super::*;

const FILE_LIMIT: usize = 2 * 1024 * 1024;
const PIN_LIMIT: usize = 256 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    #[serde(default)]
    pub recent_sessions: Vec<String>,
    #[serde(default)]
    pub pinned_tabs: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    schema_version: u32,
    legacy_bound_scope: Option<String>,
    scopes: BTreeMap<String, Entry>,
}

impl Default for Data {
    fn default() -> Self {
        Self {
            schema_version: 1,
            legacy_bound_scope: None,
            scopes: BTreeMap::new(),
        }
    }
}

pub(super) struct DeviceStore {
    root: PathBuf,
    pub(super) gate: Mutex<()>,
    io: Arc<tokio::sync::Semaphore>,
    _lease: File,
}

impl DeviceStore {
    pub async fn open(parent: PathBuf) -> Result<Arc<Self>, ApplicationError> {
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&parent).map_err(|_| unavailable())?;
            let parent = std::fs::canonicalize(parent).map_err(|_| unavailable())?;
            let root = parent.join("device-vault-state");
            match std::fs::symlink_metadata(&root) {
                Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                    return Err(unavailable());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let mut builder = std::fs::DirBuilder::new();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::DirBuilderExt;
                        builder.mode(0o700);
                    }
                    builder.create(&root).map_err(|_| unavailable())?;
                    sync_parent(&root)?;
                }
                Err(_) => return Err(unavailable()),
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| unavailable())?;
            }
            let lock = root.join(".lock");
            regular(&lock)?;
            let mut options = File::options();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let lease = options.open(&lock).map_err(|_| unavailable())?;
            lease.try_lock().map_err(|_| unavailable())?;
            sync_parent(&lock)?;
            Ok(Arc::new(Self {
                root,
                gate: Mutex::new(()),
                io: Arc::new(tokio::sync::Semaphore::new(2)),
                _lease: lease,
            }))
        })
        .await
        .map_err(|_| unavailable())?
    }

    pub async fn run<T: Send + 'static>(
        self: &Arc<Self>,
        scope: String,
        local: Option<Arc<VaultContext>>,
        operation: impl FnOnce(&mut Entry) -> Result<T, ApplicationError> + Send + 'static,
        changed: impl FnOnce(String) + Send + 'static,
    ) -> Result<T, ApplicationError> {
        validate_scope(&scope)?;
        let permit = self
            .io
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| unavailable())?;
        let storage = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let guard = storage.gate.lock().map_err(|_| unavailable())?;
            let meta = std::fs::symlink_metadata(&storage.root).map_err(|_| unavailable())?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(unavailable());
            }
            let path = storage.root.join("state.json");
            let mut data = load(&path)?;
            let before = data.clone();
            if data.legacy_bound_scope.is_none()
                && let Some(context) = local
                && context.events.vault_id() == scope
            {
                let legacy = legacy_entry(context.store.vault_base())?;
                let entry = data.scopes.entry(scope.clone()).or_default();
                for id in legacy.recent_sessions {
                    if entry.recent_sessions.len() < 10 && !entry.recent_sessions.contains(&id) {
                        entry.recent_sessions.push(id);
                    }
                }
                if entry.pinned_tabs.is_none() {
                    entry.pinned_tabs = legacy.pinned_tabs;
                }
                data.legacy_bound_scope = Some(scope.clone());
            }
            let mut entry = data.scopes.get(&scope).cloned().unwrap_or_default();
            let result = operation(&mut entry)?;
            if data.scopes.contains_key(&scope) || entry != Entry::default() {
                data.scopes.insert(scope.clone(), entry);
            }
            validate(&data)?;
            let notify = data != before;
            if notify {
                let bytes = serde_json::to_string(&data).map_err(|_| unavailable())?;
                if bytes.len() > FILE_LIMIT {
                    return Err(invalid("Device vault preference capacity reached"));
                }
                regular(&path)?;
                hypr_storage::fs::atomic_write(&path, &bytes).map_err(|_| unavailable())?;
                sync_parent(&path)?;
            }
            drop(guard);
            if notify {
                changed(scope);
            }
            Ok(result)
        })
        .await
        .map_err(|_| unavailable())?
    }
}

fn validate_scope(scope: &str) -> Result<(), ApplicationError> {
    if scope.is_empty() || scope.len() > 8192 || scope.chars().any(char::is_control) {
        return Err(invalid("Invalid device vault scope"));
    }
    Ok(())
}

pub(super) fn validate_pins(value: &str) -> Result<(), ApplicationError> {
    if value.len() > PIN_LIMIT {
        return Err(invalid("Pinned tab state is too large"));
    }
    let value: serde_json::Value =
        serde_json::from_str(value).map_err(|_| invalid("Invalid pinned tab state"))?;
    if value.as_array().is_none_or(|tabs| tabs.len() > 128) {
        return Err(invalid("Invalid pinned tab state"));
    }
    Ok(())
}

fn validate(data: &Data) -> Result<(), ApplicationError> {
    if data.schema_version != 1 || data.scopes.len() > 128 {
        return Err(unavailable());
    }
    if let Some(scope) = &data.legacy_bound_scope {
        validate_scope(scope)?;
    }
    for (scope, entry) in &data.scopes {
        validate_scope(scope)?;
        let mut seen = HashSet::new();
        if entry.recent_sessions.len() > 10 {
            return Err(unavailable());
        }
        for id in &entry.recent_sessions {
            if hypr_loofah_protocol::validate_session_id(id).is_err() || !seen.insert(id) {
                return Err(unavailable());
            }
        }
        if let Some(pins) = &entry.pinned_tabs {
            validate_pins(pins)?;
        }
    }
    Ok(())
}

fn regular(path: &Path) -> Result<(), ApplicationError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(unavailable()),
    }
}

fn read(path: &Path, limit: usize) -> Result<Option<Vec<u8>>, ApplicationError> {
    regular(path)?;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(unavailable()),
    };
    if file.metadata().map_err(|_| unavailable())?.len() > limit as u64 {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() > limit {
        return Err(unavailable());
    }
    Ok(Some(bytes))
}

fn load(path: &Path) -> Result<Data, ApplicationError> {
    let data: Data = match read(path, FILE_LIMIT)? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|_| unavailable())?,
        None => Data::default(),
    };
    validate(&data)?;
    Ok(data)
}

fn legacy_entry(vault: &Path) -> Result<Entry, ApplicationError> {
    let Some(bytes) = read(&vault.join("store.json"), FILE_LIMIT)? else {
        return Ok(Entry::default());
    };
    let data: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    let Some(desktop) = data.get("desktop").and_then(serde_json::Value::as_str) else {
        return Ok(Entry::default());
    };
    let desktop: serde_json::Value = serde_json::from_str(desktop).map_err(|_| unavailable())?;
    let mut entry = Entry::default();
    if let Some(value) = desktop
        .get("RecentlyOpenedSessions")
        .and_then(serde_json::Value::as_str)
    {
        let ids: Vec<String> = serde_json::from_str(value).map_err(|_| unavailable())?;
        for id in ids {
            if entry.recent_sessions.len() < 10
                && hypr_loofah_protocol::validate_session_id(&id).is_ok()
                && !entry.recent_sessions.contains(&id)
            {
                entry.recent_sessions.push(id);
            }
        }
    }
    if let Some(value) = desktop
        .get("PinnedTabs")
        .and_then(serde_json::Value::as_str)
    {
        validate_pins(value)?;
        entry.pinned_tabs = Some(value.into());
    }
    Ok(entry)
}

fn sync_parent(path: &Path) -> Result<(), ApplicationError> {
    #[cfg(unix)]
    File::open(path.parent().ok_or_else(unavailable)?)
        .and_then(|file| file.sync_all())
        .map_err(|_| unavailable())?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

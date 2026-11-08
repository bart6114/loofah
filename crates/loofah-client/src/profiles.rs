use std::{
    collections::BTreeMap,
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use hypr_loofah_protocol::ErrorCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::{CredentialFuture, CredentialStore, Result, failure};

mod desktop;
pub use desktop::{
    DesktopStartupSaveOutcome, DesktopStartupSelection, DesktopStartupSnapshot,
    SaveDesktopStartupRequest,
};

const RECORD_LIMIT: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub server: String,
    pub member_id: Option<String>,
    pub vault_id: Option<String>,
    pub credential_accounts: Vec<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredProfile {
    schema_version: u32,
    profile: ConnectionProfile,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}
struct LoadedProfile {
    stored: StoredProfile,
    raw: Vec<u8>,
}

pub struct ConnectionProfiles {
    root: PathBuf,
}
pub struct ProfileLease {
    store: Arc<ConnectionProfiles>,
    state: Arc<Mutex<LoadedProfile>>,
    _lock: File,
}

impl ConnectionProfiles {
    pub fn new(root: PathBuf) -> Result<Arc<Self>> {
        if !root.is_absolute() {
            return Err(failure(
                ErrorCode::Validation,
                "Connection profile storage must be absolute",
            ));
        }
        Ok(Arc::new(Self { root }))
    }

    pub async fn list(self: &Arc<Self>) -> Result<Vec<ConnectionProfile>> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.list_sync())
            .await
            .map_err(|_| worker_error())?
    }

    pub async fn open(self: &Arc<Self>, name: &str) -> Result<Arc<ProfileLease>> {
        validate_name(name)?;
        let name = name.to_owned();
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let first = store.read(&name)?.ok_or_else(not_found)?;
            let lock = store.lock(&first.stored.profile.id)?;
            let current = store.read(&name)?.ok_or_else(not_found)?;
            if first.stored.profile.id != current.stored.profile.id {
                return Err(changed());
            }
            Ok(Arc::new(ProfileLease {
                store,
                state: Arc::new(Mutex::new(current)),
                _lock: lock,
            }))
        })
        .await
        .map_err(|_| worker_error())?
    }

    pub async fn create(self: &Arc<Self>, name: &str, server: &str) -> Result<Arc<ProfileLease>> {
        validate_name(name)?;
        let server = server_origin(server)?;
        let name = name.to_owned();
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let _create = store.lock("create")?;
            if store.read(&name)?.is_some() {
                return Err(failure(
                    ErrorCode::Conflict,
                    "A connection profile with this name already exists",
                ));
            }
            if store.list_sync()?.len() >= 32 {
                return Err(failure(
                    ErrorCode::ResourceLimit,
                    "Too many saved connection profiles",
                ));
            }
            let id = uuid::Uuid::new_v4().to_string();
            let lock = store.lock(&id)?;
            let stored = StoredProfile {
                schema_version: 1,
                profile: ConnectionProfile {
                    id,
                    name,
                    server,
                    member_id: None,
                    vault_id: None,
                    credential_accounts: Vec::new(),
                    extra: Default::default(),
                },
                extra: Default::default(),
            };
            let raw = serde_json::to_vec_pretty(&stored).map_err(|_| damaged())?;
            store.write(&stored.profile.name, &raw)?;
            Ok(Arc::new(ProfileLease {
                store,
                state: Arc::new(Mutex::new(LoadedProfile { stored, raw })),
                _lock: lock,
            }))
        })
        .await
        .map_err(|_| worker_error())?
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(format!("{}.json", hash(name.as_bytes())))
    }
    fn list_sync(&self) -> Result<Vec<ConnectionProfile>> {
        check_directory(&self.root)?;
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(cause) => return Err(io_error(cause)),
        };
        let mut result = Vec::new();
        for (count, entry) in entries.enumerate() {
            if count >= 4096 {
                return Err(failure(
                    ErrorCode::ResourceLimit,
                    "Too many connection profile directory entries. Files were kept.",
                ));
            }
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.strip_suffix(".json").is_some_and(is_hash) {
                continue;
            }
            let Some(loaded) = read_file(&entry.path())? else {
                continue;
            };
            if self.path(&loaded.stored.profile.name) != entry.path() {
                return Err(damaged());
            }
            if result.len() >= 32 {
                return Err(failure(
                    ErrorCode::ResourceLimit,
                    "Too many saved connection profiles. Files were kept.",
                ));
            }
            result.push(loaded.stored.profile);
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }
    fn read(&self, name: &str) -> Result<Option<LoadedProfile>> {
        check_directory(&self.root)?;
        let value = read_file(&self.path(name))?;
        if value
            .as_ref()
            .is_some_and(|value| value.stored.profile.name != name)
        {
            return Err(damaged());
        }
        Ok(value)
    }
    fn prepare(&self) -> Result<()> {
        let locks = self.root.join(".locks");
        check_directory(&self.root)?;
        check_directory(&locks)?;
        std::fs::create_dir_all(&locks).map_err(io_error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for directory in [&self.root, &locks] {
                std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                    .map_err(io_error)?;
            }
        }
        check_directory(&self.root)?;
        check_directory(&locks)
    }
    fn lock(&self, id: &str) -> Result<File> {
        self.prepare()?;
        let path = self.root.join(".locks").join(format!("{id}.lock"));
        check_file(&path)?;
        let mut options = File::options();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(io_error)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(failure(
                        ErrorCode::Unavailable,
                        "Another command is using this connection profile. Retry after it finishes.",
                    ));
                }
                Err(std::fs::TryLockError::Error(cause)) => return Err(io_error(cause)),
            }
        }
    }
    fn write(&self, name: &str, raw: &[u8]) -> Result<()> {
        if raw.len() > RECORD_LIMIT {
            return Err(failure(
                ErrorCode::ResourceLimit,
                "The connection profile exceeds its size limit",
            ));
        }
        self.prepare()?;
        let path = self.path(name);
        check_file(&path)?;
        let value = std::str::from_utf8(raw).map_err(|_| damaged())?;
        hypr_storage::fs::atomic_write(&path, value).map_err(io_error)?;
        sync_parent(&path)
    }
}

impl ProfileLease {
    pub async fn profile(&self) -> ConnectionProfile {
        self.state.lock().await.stored.profile.clone()
    }

    pub async fn save(self: &Arc<Self>, mut profile: ConnectionProfile) -> Result<()> {
        validate_profile(&profile)?;
        let mut state = self.state.clone().lock_owned().await;
        let lease = self.clone();
        tokio::task::spawn_blocking(move || {
            let previous = &state.stored.profile;
            if (
                profile.id.as_str(),
                profile.name.as_str(),
                profile.server.as_str(),
            ) != (
                previous.id.as_str(),
                previous.name.as_str(),
                previous.server.as_str(),
            ) {
                return Err(failure(
                    ErrorCode::Validation,
                    "A connection profile's identity and server cannot be replaced",
                ));
            }
            let current = lease.store.read(&previous.name)?.ok_or_else(changed)?;
            if current.raw != state.raw {
                return Err(changed());
            }
            profile.extra = previous.extra.clone();
            let stored = StoredProfile {
                schema_version: 1,
                profile,
                extra: state.stored.extra.clone(),
            };
            let raw = serde_json::to_vec_pretty(&stored).map_err(|_| damaged())?;
            lease.store.write(&stored.profile.name, &raw)?;
            *state = LoadedProfile { stored, raw };
            Ok(())
        })
        .await
        .map_err(|_| worker_error())?
    }

    pub async fn remove(self: &Arc<Self>) -> Result<()> {
        let state = self.state.clone().lock_owned().await;
        let lease = self.clone();
        tokio::task::spawn_blocking(move || {
            let name = &state.stored.profile.name;
            let current = lease.store.read(name)?.ok_or_else(not_found)?;
            if current.raw != state.raw {
                return Err(changed());
            }
            let path = lease.store.path(name);
            std::fs::remove_file(&path).map_err(io_error)?;
            sync_parent(&path)
        })
        .await
        .map_err(|_| worker_error())?
    }

    // Detached refresh workers must keep the process lock after their caller is cancelled.
    pub fn credentials(
        self: &Arc<Self>,
        inner: Arc<dyn CredentialStore>,
    ) -> Arc<dyn CredentialStore> {
        Arc::new(LeasedCredentials {
            inner,
            _lease: self.clone(),
        })
    }
}

struct LeasedCredentials {
    inner: Arc<dyn CredentialStore>,
    _lease: Arc<ProfileLease>,
}
impl CredentialStore for LeasedCredentials {
    fn load<'a>(&'a self, account: &'a str) -> CredentialFuture<'a, Option<String>> {
        self.inner.load(account)
    }
    fn save<'a>(&'a self, account: &'a str, value: Option<&'a str>) -> CredentialFuture<'a, ()> {
        self.inner.save(account, value)
    }
}

fn read_file(path: &Path) -> Result<Option<LoadedProfile>> {
    let Some(raw) = read_record(path, RECORD_LIMIT)? else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| damaged())?;
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err(failure(
            ErrorCode::IncompatibleVersion,
            "This saved connection profile requires a different Loofah version. Its file was kept.",
        ));
    }
    let stored: StoredProfile = serde_json::from_slice(&raw).map_err(|_| damaged())?;
    validate_profile(&stored.profile).map_err(|_| damaged())?;
    Ok(Some(LoadedProfile { stored, raw }))
}

fn read_record(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
    check_file(path)?;
    use std::io::Read;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(cause) => return Err(io_error(cause)),
    };
    if file.metadata().map_err(io_error)?.len() > limit as u64 {
        return Err(damaged());
    }
    let mut raw = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(io_error)?;
    if raw.len() > limit {
        return Err(damaged());
    }
    Ok(Some(raw))
}
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(failure(
            ErrorCode::Validation,
            "Profile names use 1-64 letters, digits, hyphens or underscores",
        ));
    }
    Ok(())
}
fn validate_profile(profile: &ConnectionProfile) -> Result<()> {
    validate_name(&profile.name)?;
    if !uuid::Uuid::parse_str(&profile.id).is_ok_and(|id| id.to_string() == profile.id)
        || profile.server != server_origin(&profile.server)?
        || profile.credential_accounts.len() > 8
        || profile
            .credential_accounts
            .iter()
            .any(|account| !is_hash(account))
        || [&profile.member_id, &profile.vault_id].iter().any(|field| {
            field.as_ref().is_some_and(|value| {
                value.is_empty() || value.len() > 512 || value.contains(['/', '\\', '\0'])
            })
        })
    {
        return Err(failure(
            ErrorCode::Validation,
            "Invalid saved connection profile",
        ));
    }
    Ok(())
}
fn server_origin(server: &str) -> Result<String> {
    let url = url::Url::parse(server)
        .map_err(|_| failure(ErrorCode::Validation, "Invalid server address"))?;
    crate::validate_endpoint(&url, false)?;
    if url.path() != "/" || url.query().is_some() {
        return Err(failure(
            ErrorCode::Validation,
            "Use the server's HTTPS origin without a path or query",
        ));
    }
    Ok(url.to_string())
}
fn check_directory(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => Err(damaged()),
        Ok(_) => Ok(()),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(cause) => Err(io_error(cause)),
    }
}
fn check_file(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => Err(damaged()),
        Ok(_) => Ok(()),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(cause) => Err(io_error(cause)),
    }
}
fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path.parent().unwrap())
        .and_then(|file| file.sync_all())
        .map_err(io_error)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn hash(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn is_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn not_found() -> hypr_loofah_protocol::ApplicationError {
    failure(ErrorCode::NotFound, "Saved server profile not found")
}
fn changed() -> hypr_loofah_protocol::ApplicationError {
    failure(
        ErrorCode::Conflict,
        "The saved connection profile changed. Reload it before trying again. Its file was kept.",
    )
}
fn damaged() -> hypr_loofah_protocol::ApplicationError {
    failure(
        ErrorCode::Internal,
        "A saved connection profile could not be read safely. Its file was kept.",
    )
}
fn worker_error() -> hypr_loofah_protocol::ApplicationError {
    failure(ErrorCode::Internal, "The connection profile worker stopped")
}
fn io_error(cause: std::io::Error) -> hypr_loofah_protocol::ApplicationError {
    tracing::error!(%cause,"connection profile storage failed");
    failure(
        ErrorCode::Internal,
        "Cannot access saved connection profile storage",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn profiles_survive_restart_without_putting_credentials_in_their_metadata() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("profiles");
        let store = ConnectionProfiles::new(directory.clone()).unwrap();
        assert!(store.list().await.unwrap().is_empty());
        assert!(!directory.exists());
        assert_eq!(
            store.open("missing").await.err().unwrap().code,
            ErrorCode::NotFound
        );
        assert!(!directory.exists());
        let lease = store
            .create("work", "https://company.example")
            .await
            .unwrap();
        let mut profile = lease.profile().await;
        assert_eq!(profile.server, "https://company.example/");
        profile.member_id = Some("member".into());
        profile.vault_id = Some("shared".into());
        profile.credential_accounts.push("a".repeat(64));
        lease.save(profile.clone()).await.unwrap();
        let bytes = std::fs::read(store.path("work")).unwrap();
        assert!(
            !String::from_utf8(bytes.clone())
                .unwrap()
                .contains("refresh")
        );
        assert_eq!(
            store
                .create("work", "https://other.example")
                .await
                .err()
                .unwrap()
                .code,
            ErrorCode::Conflict
        );
        drop(lease);
        drop(store);
        let store = ConnectionProfiles::new(directory).unwrap();
        let lease = store.open("work").await.unwrap();
        assert_eq!(lease.profile().await, profile);
        assert_eq!(store.list().await.unwrap(), vec![profile]);
        assert_eq!(std::fs::read(store.path("work")).unwrap(), bytes);
        lease.remove().await.unwrap();
        assert!(store.list().await.unwrap().is_empty());
        assert_eq!(
            lease.save(lease.profile().await).await.unwrap_err().code,
            ErrorCode::Conflict
        );
    }

    #[tokio::test]
    async fn independent_profiles_do_not_block_each_other_but_same_profile_operations_are_exclusive()
     {
        let root = tempfile::tempdir().unwrap();
        let store = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
        let first = store
            .create("first", "https://company.example")
            .await
            .unwrap();
        let second = store
            .create("second", "https://company.example")
            .await
            .unwrap();
        let id = first.profile().await.id;
        let probe = File::options()
            .read(true)
            .write(true)
            .open(store.root.join(".locks").join(format!("{id}.lock")))
            .unwrap();
        assert!(matches!(
            probe.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        let pending = store.open("first");
        tokio::pin!(pending);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut pending)
                .await
                .is_err()
        );
        drop(first);
        let next = tokio::time::timeout(Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next.profile().await.name, "first");
        assert_eq!(second.profile().await.name, "second");
    }

    #[tokio::test]
    async fn profile_updates_preserve_future_fields_and_refuse_external_edits_and_identity_changes()
    {
        let root = tempfile::tempdir().unwrap();
        let store = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
        let lease = store
            .create("work", "https://company.example")
            .await
            .unwrap();
        drop(lease);
        let path = store.path("work");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["future_top"] = json!({"keep":true});
        value["profile"]["future_field"] = json!(["keep"]);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let lease = store.open("work").await.unwrap();
        let mut profile = lease.profile().await;
        profile.member_id = Some("member".into());
        lease.save(profile.clone()).await.unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["future_top"], value["future_top"]);
        assert_eq!(
            saved["profile"]["future_field"],
            value["profile"]["future_field"]
        );
        profile.server = "https://other.example/".into();
        assert_eq!(
            lease.save(profile).await.unwrap_err().code,
            ErrorCode::Validation
        );
        let mut raw = std::fs::read(&path).unwrap();
        raw.push(b'\n');
        std::fs::write(&path, &raw).unwrap();
        assert_eq!(
            lease.save(lease.profile().await).await.unwrap_err().code,
            ErrorCode::Conflict
        );
        assert_eq!(lease.remove().await.unwrap_err().code, ErrorCode::Conflict);
        assert_eq!(std::fs::read(&path).unwrap(), raw);
    }

    #[tokio::test]
    async fn damaged_future_misplaced_and_oversized_profiles_are_kept_without_being_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let store = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
        let lease = store
            .create("work", "https://company.example")
            .await
            .unwrap();
        drop(lease);
        let path = store.path("work");
        let original = std::fs::read(&path).unwrap();
        for (bytes, code) in [
            (b"invalid JSON".to_vec(), ErrorCode::Internal),
            (
                serde_json::to_vec(&json!({"schema_version":2})).unwrap(),
                ErrorCode::IncompatibleVersion,
            ),
            (vec![b'x'; RECORD_LIMIT + 1], ErrorCode::Internal),
        ] {
            std::fs::write(&path, &bytes).unwrap();
            assert_eq!(store.open("work").await.err().unwrap().code, code);
            assert_eq!(
                store
                    .create("work", "https://company.example")
                    .await
                    .err()
                    .unwrap()
                    .code,
                code
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        std::fs::write(&path, &original).unwrap();
        std::fs::rename(&path, store.path("other")).unwrap();
        assert!(store.list().await.is_err());
        assert!(store.open("other").await.is_err());
        assert_eq!(std::fs::read(store.path("other")).unwrap(), original);
    }

    #[tokio::test]
    async fn profile_input_validation_and_limits_fail_before_replacing_a_saved_connection() {
        let root = tempfile::tempdir().unwrap();
        let store = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
        for name in ["../other", "", "with spaces", ".hidden"] {
            assert_eq!(
                store
                    .create(name, "https://company.example")
                    .await
                    .err()
                    .unwrap()
                    .code,
                ErrorCode::Validation
            );
        }
        for server in [
            "http://company.example",
            "https://user:secret@company.example",
            "https://company.example/path",
            "https://company.example/?query=1",
            "https://company.example/#fragment",
        ] {
            assert!(store.create("work", server).await.is_err());
        }
        assert!(!store.root.exists());
        let lease = store
            .create("work", "https://company.example")
            .await
            .unwrap();
        let bytes = std::fs::read(store.path("work")).unwrap();
        let mut profile = lease.profile().await;
        profile.credential_accounts = vec!["a".repeat(64); 9];
        assert!(lease.save(profile).await.is_err());
        assert_eq!(std::fs::read(store.path("work")).unwrap(), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&store.root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(store.path("work"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn profile_and_lock_symlinks_never_read_or_overwrite_foreign_files() {
        let root = tempfile::tempdir().unwrap();
        let store = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
        let lease = store
            .create("work", "https://company.example")
            .await
            .unwrap();
        let profile = lease.profile().await;
        drop(lease);
        let path = store.path("work");
        let foreign = root.path().join("foreign.json");
        let original = std::fs::read(&path).unwrap();
        std::fs::rename(&path, &foreign).unwrap();
        std::os::unix::fs::symlink(&foreign, &path).unwrap();
        assert!(store.open("work").await.is_err());
        assert!(store.list().await.is_err());
        assert_eq!(std::fs::read(&foreign).unwrap(), original);
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(&foreign, &path).unwrap();
        let lock = store
            .root
            .join(".locks")
            .join(format!("{}.lock", profile.id));
        std::fs::remove_file(&lock).unwrap();
        std::fs::write(&foreign, "foreign").unwrap();
        std::os::unix::fs::symlink(&foreign, &lock).unwrap();
        assert!(store.open("work").await.is_err());
        assert_eq!(std::fs::read_to_string(&foreign).unwrap(), "foreign");
    }
}

#[cfg(test)]
mod process_tests {
    use super::*;

    #[test]
    fn lock_probe() {
        let Some(path) = std::env::var_os("LOOFAH_TEST_PROFILE_LOCK") else {
            return;
        };
        let file = File::options().read(true).write(true).open(path).unwrap();
        match (
            file.try_lock(),
            std::env::var("LOOFAH_TEST_PROFILE_LOCK_STATE")
                .unwrap()
                .as_str(),
        ) {
            (Err(std::fs::TryLockError::WouldBlock), "held") => {
                println!("PROFILE_LOCK_HELD_BY_OTHER_PROCESS")
            }
            (Ok(()), "free") => println!("PROFILE_LOCK_AVAILABLE"),
            (result, state) => {
                panic!("Unexpected process-lock result {result:?}, expected {state}")
            }
        }
    }

    #[tokio::test]
    async fn profile_lock_is_exclusive_across_processes_and_released_with_its_owner() {
        let root = tempfile::tempdir().unwrap();
        let store = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
        let lease = store
            .create("work", "https://company.example")
            .await
            .unwrap();
        let path = store
            .root
            .join(".locks")
            .join(format!("{}.lock", lease.profile().await.id));
        let probe = |state: &str| {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "profiles::process_tests::lock_probe",
                    "--nocapture",
                ])
                .env("LOOFAH_TEST_PROFILE_LOCK", &path)
                .env("LOOFAH_TEST_PROFILE_LOCK_STATE", state)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        assert!(probe("held").contains("PROFILE_LOCK_HELD_BY_OTHER_PROCESS"));
        drop(lease);
        assert!(probe("free").contains("PROFILE_LOCK_AVAILABLE"));
    }
}

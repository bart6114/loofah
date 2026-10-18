use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Reply, Result, Versions, validate_id};

pub struct RootLock {
    _file: File,
}

pub fn create_dir_all(path: &Path) -> Result<()> {
    let mut missing = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor
            .parent()
            .ok_or_else(|| Error::new("storage", "directory has no parent"))?;
    }
    for directory in missing.into_iter().rev() {
        match fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists && directory.is_dir() => {}
            Err(error) => return Err(error.into()),
        }
        File::open(directory.parent().unwrap())?.sync_all()?;
        File::open(directory)?.sync_all()?;
    }
    Ok(())
}

impl RootLock {
    pub fn acquire(root: &Path) -> Result<Self> {
        create_dir_all(root)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".owner.lock"))?;
        let acquired = fs4::fs_std::FileExt::try_lock_exclusive(&file)
            .map_err(|_| Error::new("locked", "another process owns this data directory"))?;
        if !acquired {
            return Err(Error::new(
                "locked",
                "another process owns this data directory",
            ));
        }
        Ok(Self { _file: file })
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("storage", "missing parent"))?;
    create_dir_all(parent)?;
    let staging = parent.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staging)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(test)]
        tests::crash_point();
        fs::rename(&staging, path)?;
        #[cfg(test)]
        tests::crash_point();
        File::open(parent)?.sync_all()?;
        #[cfg(test)]
        tests::crash_point();
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(staging);
    }
    result
}

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    atomic_write(path, &serde_json::to_vec_pretty(value)?)
}

pub fn safe_path(root: &Path, key: &str) -> Result<PathBuf> {
    let relative = Path::new(key);
    if key.is_empty()
        || key.contains('\\')
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Error::new("invalid_request", "unsafe resource key"));
    }
    let mut path = root.to_path_buf();
    for part in relative.components() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::new("invalid_request", "symlinks are not supported"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}

pub fn file_version(path: &Path) -> Result<Option<String>> {
    use sha2::{Digest, Sha256};
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(Some(
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    ))
}

pub enum Change {
    Write(String, Vec<u8>),
    Publish(String, PathBuf, String),
    Remove(String),
    DeleteSession(String),
    RestoreSession(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Action {
    Write { key: String, staged: String },
    Remove { key: String },
    DeleteSession { id: String },
    RestoreSession { id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Transaction {
    id: String,
    fingerprint: String,
    actions: Vec<Action>,
    reply: Reply,
    complete: bool,
}

pub trait VaultStorage: Send + Sync {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn versions(&self, keys: &[String]) -> Result<Versions>;
    fn commit(
        &self,
        id: &str,
        fingerprint: &str,
        expected: &Versions,
        changes: Vec<Change>,
        value: Value,
    ) -> Result<Reply>;
}

pub struct FilesystemStorage {
    pub content: PathBuf,
    pub state: PathBuf,
}

impl FilesystemStorage {
    pub fn new(content: PathBuf, state: PathBuf) -> Result<Self> {
        create_dir_all(&content)?;
        if fs::symlink_metadata(&content)?.file_type().is_symlink() {
            return Err(Error::new(
                "invalid_request",
                "vault root cannot be a symlink",
            ));
        }
        create_dir_all(&state.join("journal"))?;
        create_dir_all(&state.join("trash"))?;
        let storage = Self { content, state };
        storage.recover()?;
        Ok(storage)
    }

    pub fn operation(&self, id: &str, fingerprint: Option<&str>) -> Result<Option<Reply>> {
        validate_id(id)?;
        let path = self.state.join("journal").join(id).join("transaction.json");
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut transaction: Transaction = serde_json::from_slice(&bytes)?;
        if fingerprint.is_some_and(|value| value != transaction.fingerprint) {
            return Err(Error::new(
                "invalid_request",
                "mutation ID already used for a different request",
            ));
        }
        if !transaction.complete {
            self.apply(&mut transaction)?;
        }
        Ok(Some(transaction.reply))
    }

    pub fn deleted(&self, id: &str) -> Result<bool> {
        validate_id(id)?;
        Ok(self.state.join("trash").join(id).exists())
    }

    pub fn recover(&self) -> Result<()> {
        for entry in fs::read_dir(self.state.join("journal"))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let manifest = entry.path().join("transaction.json");
            if !manifest.exists() {
                continue;
            }
            let mut transaction: Transaction = serde_json::from_slice(&fs::read(manifest)?)?;
            if !transaction.complete {
                self.apply(&mut transaction)?;
            }
        }
        Ok(())
    }

    fn apply(&self, transaction: &mut Transaction) -> Result<()> {
        let directory = self.state.join("journal").join(&transaction.id);
        for action in &transaction.actions {
            match action {
                Action::Write { key, staged } => {
                    let destination = safe_path(&self.content, key)?;
                    let parent = destination.parent().unwrap();
                    create_dir_all(parent)?;
                    let temp = parent.join(format!(".tmp-{}", transaction.id));
                    if temp.exists() {
                        fs::remove_file(&temp)?;
                    }
                    fs::hard_link(directory.join(staged), &temp)?;
                    File::open(&temp)?.sync_all()?;
                    fs::rename(&temp, &destination)?;
                    File::open(parent)?.sync_all()?;
                }
                Action::Remove { key } => {
                    let path = safe_path(&self.content, key)?;
                    if path.exists() {
                        let archive =
                            safe_path(&self.state.join("trash-files").join(&transaction.id), key)?;
                        create_dir_all(archive.parent().unwrap())?;
                        fs::rename(&path, &archive)?;
                        File::open(path.parent().unwrap())?.sync_all()?;
                        File::open(archive.parent().unwrap())?.sync_all()?;
                    }
                }
                Action::DeleteSession { id } | Action::RestoreSession { id } => {
                    validate_id(id)?;
                    let live = safe_path(&self.content, &format!("sessions/{id}"))?;
                    let trash = self.state.join("trash").join(id);
                    let (source, destination) = if matches!(action, Action::DeleteSession { .. }) {
                        (live, trash)
                    } else {
                        (trash, live)
                    };
                    if source.exists() {
                        if destination.exists() {
                            return Err(Error::new(
                                "conflict",
                                "session destination already exists",
                            ));
                        }
                        create_dir_all(destination.parent().unwrap())?;
                        fs::rename(&source, &destination)?;
                        File::open(source.parent().unwrap())?.sync_all()?;
                        File::open(destination.parent().unwrap())?.sync_all()?;
                    } else if !destination.exists() {
                        return Err(Error::new("storage", "session recovery source is missing"));
                    }
                }
            }
            #[cfg(test)]
            tests::crash_point();
        }
        transaction.complete = true;
        atomic_json(&directory.join("transaction.json"), transaction)?;
        for action in &transaction.actions {
            if let Action::Write { staged, .. } = action {
                let _ = fs::remove_file(directory.join(staged));
            }
        }
        Ok(())
    }
}

impl VaultStorage for FilesystemStorage {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match fs::read(safe_path(&self.content, key)?) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn versions(&self, keys: &[String]) -> Result<Versions> {
        keys.iter()
            .map(|key| Ok((key.clone(), file_version(&safe_path(&self.content, key)?)?)))
            .collect()
    }

    fn commit(
        &self,
        id: &str,
        fingerprint: &str,
        expected: &Versions,
        changes: Vec<Change>,
        value: Value,
    ) -> Result<Reply> {
        validate_id(id)?;
        if let Some(reply) = self.operation(id, Some(fingerprint))? {
            return Ok(reply);
        }
        for (key, expected) in expected {
            if file_version(&safe_path(&self.content, key)?)? != *expected {
                return Err(Error::new(
                    "conflict",
                    format!("{key} changed since it was read"),
                ));
            }
        }
        let directory = self.state.join("journal").join(id);
        create_dir_all(&directory)?;
        let mut versions = BTreeMap::new();
        let mut actions = Vec::new();
        for (index, change) in changes.into_iter().enumerate() {
            let staged = index.to_string();
            match change {
                Change::Write(key, bytes) => {
                    safe_path(&self.content, &key)?;
                    atomic_write(&directory.join(&staged), &bytes)?;
                    versions.insert(key.clone(), Some(crate::digest(&bytes)));
                    actions.push(Action::Write { key, staged });
                }
                Change::Publish(key, source, checksum) => {
                    safe_path(&self.content, &key)?;
                    if directory.join(&staged).exists() {
                        fs::remove_file(directory.join(&staged))?;
                    }
                    fs::hard_link(source, directory.join(&staged))?;
                    File::open(directory.join(&staged))?.sync_all()?;
                    #[cfg(test)]
                    tests::crash_point();
                    versions.insert(key.clone(), Some(checksum));
                    actions.push(Action::Write { key, staged });
                }
                Change::Remove(key) => {
                    safe_path(&self.content, &key)?;
                    versions.insert(key.clone(), None);
                    actions.push(Action::Remove { key });
                }
                Change::DeleteSession(id) => {
                    validate_id(&id)?;
                    actions.push(Action::DeleteSession { id });
                }
                Change::RestoreSession(id) => {
                    validate_id(&id)?;
                    actions.push(Action::RestoreSession { id });
                }
            }
        }
        let mut transaction = Transaction {
            id: id.into(),
            fingerprint: fingerprint.into(),
            actions,
            reply: Reply {
                data: value,
                versions,
            },
            complete: false,
        };
        atomic_json(&directory.join("transaction.json"), &transaction)?;
        File::open(directory.parent().unwrap())?.sync_all()?;
        if let Err(mut error) = self.apply(&mut transaction) {
            error.code = "commit_pending".into();
            error.operation_id = Some(id.into());
            return Err(error);
        }
        Ok(transaction.reply)
    }
}

#[cfg(test)]
mod tests;

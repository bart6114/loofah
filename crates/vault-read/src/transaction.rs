use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// All participating processes lock the same inode. Never unlink or replace this file.
/// External editors do not participate; sync must still check and preserve their edits.
#[derive(Debug)]
pub struct VaultTransaction {
    _file: File,
}

impl VaultTransaction {
    pub fn try_acquire(vault: &Path, exclusive: bool) -> io::Result<Option<Self>> {
        let file = open_lock(vault)?;
        let result = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        match result {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
    }
    pub fn recording(vault: &Path, id: &str) -> io::Result<Self> {
        let file = open_recording_lock(vault, id)?;
        file.try_lock().map_err(io::Error::from)?;
        Ok(Self { _file: file })
    }

    pub fn is_recording(vault: &Path, id: &str) -> io::Result<bool> {
        let file = open_recording_lock(vault, id)?;
        match file.try_lock_shared() {
            Ok(()) => Ok(false),
            Err(std::fs::TryLockError::WouldBlock) => Ok(true),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
    }

    pub fn any_recording(vault: &Path) -> io::Result<bool> {
        for entry in std::fs::read_dir(vault)? {
            let entry = entry?;
            if let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix(".loofah-recording-"))
                && Self::is_recording(vault, id)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn ensure_ready(vault: &Path) -> io::Result<()> {
        match std::fs::symlink_metadata(vault.join(".loofah-sync-pending")) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
            Ok(_) => Err(io::Error::other(
                "vault sync recovery is required before reading or writing",
            )),
        }
    }

    pub fn exclusive(vault: &Path) -> io::Result<Self> {
        let file = open_lock(vault)?;
        file.lock()?;
        Ok(Self { _file: file })
    }

    pub fn shared(vault: &Path) -> io::Result<Self> {
        let file = open_lock(vault)?;
        file.lock_shared()?;
        Ok(Self { _file: file })
    }
}

fn open_recording_lock(vault: &Path, id: &str) -> io::Result<File> {
    crate::paths::validate_session_id(id)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid recording identity"))?;
    let path = vault.join(format!(".loofah-recording-{id}"));
    if let Ok(meta) = std::fs::symlink_metadata(&path)
        && !meta.is_file()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsafe recording lock",
        ));
    }
    open_file(&path)
}

fn open_lock(vault: &Path) -> io::Result<File> {
    if !vault.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "vault is missing"));
    }
    let path = vault.join(".loofah-lock");
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsafe vault lock",
            ));
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    open_file(&path)
}

fn open_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsafe vault lock",
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A concurrently spawned child can inherit another test's lock until exec,
    // so dropping the parent's handle alone would not release that lock yet.
    static PROCESS_LOCK_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn independent_handles_exclude_writers_and_release_on_drop() {
        let _serial = PROCESS_LOCK_TEST.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let reader = VaultTransaction::shared(temp.path()).unwrap();
        let second_reader = VaultTransaction::shared(temp.path()).unwrap();
        let contender = open_lock(temp.path()).unwrap();
        assert!(matches!(
            contender.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(reader);
        assert!(matches!(
            contender.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(second_reader);
        contender.try_lock().unwrap();
    }

    #[test]
    fn missing_vault_is_not_created() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        assert!(VaultTransaction::exclusive(&missing).is_err());
        assert!(!missing.exists());
    }

    #[test]
    fn process_lock_probe() {
        let Some(path) = std::env::var_os("LOOFAH_TRANSACTION_TEST_VAULT") else {
            return;
        };
        let vault = Path::new(&path);
        assert!(
            VaultTransaction::try_acquire(vault, true)
                .unwrap()
                .is_none()
        );
        assert!(
            VaultTransaction::try_acquire(vault, false)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn lock_excludes_a_separate_process() {
        let _serial = PROCESS_LOCK_TEST.lock().unwrap();
        let vault = tempfile::tempdir().unwrap();
        let _guard = VaultTransaction::exclusive(vault.path()).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "transaction::tests::process_lock_probe"])
            .env("LOOFAH_TRANSACTION_TEST_VAULT", vault.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
}

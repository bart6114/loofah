use std::{
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

/// A kernel-held lock. Sharing its pin never reopens or explicitly unlocks it.
#[derive(Debug, Clone)]
pub struct VaultLease {
    canonical: PathBuf,
    file: Arc<File>,
}

#[derive(Debug)]
pub enum LeaseAttempt {
    Acquired(VaultLease),
    Busy,
}

impl VaultLease {
    pub fn try_acquire_existing(vault: &Path) -> io::Result<LeaseAttempt> {
        let canonical = vault.canonicalize()?;
        if !canonical.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Vault path is not a directory",
            ));
        }
        let path = canonical.join(".loofah-vault.lock");
        match path.symlink_metadata() {
            Ok(metadata) => check_lock_metadata(&metadata)?,
            Err(cause) if cause.kind() == io::ErrorKind::NotFound => {}
            Err(cause) => return Err(cause),
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Open the lock entry itself so a reparse point cannot redirect it.
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = options.open(&path)?;
        let opened = file.metadata()?;
        check_lock_metadata(&opened)?;
        let current = path.symlink_metadata()?;
        check_lock_metadata(&current)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (opened.dev(), opened.ino()) != (current.dev(), current.ino()) {
                return Err(io::Error::other("Vault lock changed while opening"));
            }
        }
        match file.try_lock() {
            Ok(()) => Ok(LeaseAttempt::Acquired(Self {
                canonical,
                file: Arc::new(file),
            })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(LeaseAttempt::Busy),
            Err(cause) => Err(io::Error::other(cause)),
        }
    }

    pub fn canonical_vault(&self) -> &Path {
        &self.canonical
    }
    pub fn pin(&self) -> Arc<File> {
        self.file.clone()
    }

    /// Existing owned startup recovery takes the acquired File and returns it.
    /// This succeeds only before sharing, without reopening the lock entry.
    pub fn into_unshared_file(self) -> io::Result<(PathBuf, File)> {
        let file = Arc::try_unwrap(self.file)
            .map_err(|_| io::Error::other("Vault lease is already shared"))?;
        Ok((self.canonical, file))
    }
}

fn check_lock_metadata(metadata: &std::fs::Metadata) -> io::Result<()> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Vault lock must be a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // geteuid has no pointer arguments and only reads this process identity.
        let uid = unsafe { libc::geteuid() };
        if metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Vault lock must belong to the current user and not be writable by other users",
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Vault lock must not be a reparse point",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn acquired(vault: &Path) -> VaultLease {
        match VaultLease::try_acquire_existing(vault).unwrap() {
            LeaseAttempt::Acquired(lease) => lease,
            LeaseAttempt::Busy => panic!("unexpected owner"),
        }
    }
    #[test]
    fn clones_and_file_pins_keep_the_single_lock_until_the_last_owner_drops() {
        let vault = tempfile::tempdir().unwrap();
        let lease = acquired(vault.path());
        let other = lease.clone();
        let pin = lease.pin();
        drop(lease);
        assert!(matches!(
            VaultLease::try_acquire_existing(vault.path()).unwrap(),
            LeaseAttempt::Busy
        ));
        drop(other);
        assert!(matches!(
            VaultLease::try_acquire_existing(vault.path()).unwrap(),
            LeaseAttempt::Busy
        ));
        drop(pin);
        drop(acquired(vault.path()));
    }
    #[test]
    fn startup_transfer_keeps_existing_lock_bytes_and_never_reopens_the_descriptor() {
        let vault = tempfile::tempdir().unwrap();
        let path = vault.path().join(".loofah-vault.lock");
        std::fs::write(&path, b"preserve existing lock bytes").unwrap();
        let lease = acquired(vault.path());
        let (canonical, file) = lease.into_unshared_file().unwrap();
        assert_eq!(canonical, vault.path().canonicalize().unwrap());
        assert!(matches!(
            VaultLease::try_acquire_existing(vault.path()).unwrap(),
            LeaseAttempt::Busy
        ));
        drop(file);
        drop(acquired(vault.path()));
        assert_eq!(
            std::fs::read(path).unwrap(),
            b"preserve existing lock bytes"
        );
    }
    #[test]
    fn shared_transfer_refusal_preserves_the_remaining_owner() {
        let vault = tempfile::tempdir().unwrap();
        let lease = acquired(vault.path());
        let pin = lease.pin();
        assert!(lease.into_unshared_file().is_err());
        assert!(matches!(
            VaultLease::try_acquire_existing(vault.path()).unwrap(),
            LeaseAttempt::Busy
        ));
        drop(pin);
        drop(acquired(vault.path()));
    }
    #[test]
    fn missing_vault_and_nonregular_lock_are_never_seeded_or_replaced() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing");
        assert_eq!(
            VaultLease::try_acquire_existing(&missing)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert!(!missing.exists());
        let path = root.path().join(".loofah-vault.lock");
        std::fs::create_dir(&path).unwrap();
        assert!(VaultLease::try_acquire_existing(root.path()).is_err());
        assert!(path.is_dir());
    }
    #[cfg(unix)]
    #[test]
    fn canonical_vault_aliases_share_ownership_but_lock_symlinks_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let vault = root.path().join("vault");
        std::fs::create_dir(&vault).unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&vault, &alias).unwrap();
        let lease = acquired(&alias);
        assert_eq!(lease.canonical_vault(), vault.canonicalize().unwrap());
        assert!(matches!(
            VaultLease::try_acquire_existing(&vault).unwrap(),
            LeaseAttempt::Busy
        ));
        drop(lease);
        std::fs::remove_file(vault.join(".loofah-vault.lock")).unwrap();
        let target = root.path().join("user-file");
        std::fs::write(&target, b"never change").unwrap();
        std::os::unix::fs::symlink(&target, vault.join(".loofah-vault.lock")).unwrap();
        assert!(VaultLease::try_acquire_existing(&vault).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"never change");
    }
    #[cfg(unix)]
    #[test]
    fn new_locks_are_private_and_existing_other_writable_locks_fail_closed() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let root = tempfile::tempdir().unwrap();
        drop(acquired(root.path()));
        let path = root.path().join(".loofah-vault.lock");
        assert_eq!(path.metadata().unwrap().mode() & 0o777, 0o600);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(VaultLease::try_acquire_existing(root.path()).is_err());
        assert_eq!(path.metadata().unwrap().mode() & 0o777, 0o666);
    }
}

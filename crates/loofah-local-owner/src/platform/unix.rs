use super::Stream;
use crate::{Error, Result};
use std::{
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};
use tokio::net::{UnixListener, UnixStream};

pub(crate) type Address = PathBuf;
pub(crate) struct Listener {
    listener: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
}

fn uid() -> u32 {
    // No pointers or mutable process state are involved in reading the effective uid.
    unsafe { libc::geteuid() }
}

pub(super) fn system_root() -> Result<PathBuf> {
    Ok(Path::new("/tmp")
        .canonicalize()?
        .join(format!("loofah-owner-{}", uid())))
}

pub(super) fn ensure_root(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(Error::protocol("Local owner directory must be absolute"));
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => check_root(&metadata)?,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(path) {
                Ok(()) => {}
                Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(cause) => return Err(cause.into()),
            }
            check_root(&std::fs::symlink_metadata(path)?)?;
        }
        Err(cause) => return Err(cause.into()),
    }
    Ok(())
}
fn check_root(metadata: &std::fs::Metadata) -> Result<()> {
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid()
        || metadata.mode() & 0o077 != 0
    {
        return Err(Error::Unavailable(
            "Local owner directory must be private and belong to the current user".into(),
        ));
    }
    Ok(())
}

pub(super) fn address(root: &Path, key: &str) -> Result<Address> {
    let path = root.join(format!("{}.sock", &key[..32]));
    if path.as_os_str().as_bytes().len() >= 104 {
        return Err(Error::Unavailable(
            "Local owner socket path is too long; choose a shorter private endpoint directory"
                .into(),
        ));
    }
    Ok(path)
}

pub(super) async fn bind(path: &Address) -> Result<Listener> {
    ensure_root(
        path.parent()
            .ok_or_else(|| Error::protocol("Invalid owner endpoint"))?,
    )?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() || metadata.uid() != uid() {
                return Err(Error::Unavailable(
                    "Local owner endpoint is not an owned socket".into(),
                ));
            }
            match tokio::time::timeout(
                std::time::Duration::from_millis(250),
                UnixStream::connect(path),
            )
            .await
            {
                Ok(Ok(_)) => {
                    return Err(Error::Unavailable(
                        "A local owner endpoint is already listening".into(),
                    ));
                }
                Ok(Err(cause))
                    if matches!(
                        cause.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) => {}
                Ok(Err(cause)) => return Err(cause.into()),
                Err(_) => {
                    return Err(Error::Unavailable(
                        "Existing local owner endpoint is unresponsive".into(),
                    ));
                }
            }
            let current = std::fs::symlink_metadata(path)?;
            if (metadata.dev(), metadata.ino()) != (current.dev(), current.ino()) {
                return Err(Error::Unavailable(
                    "Local owner endpoint changed during stale cleanup".into(),
                ));
            }
            std::fs::remove_file(path)?;
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Err(cause) => return Err(cause.into()),
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let metadata = std::fs::symlink_metadata(path)?;
    Ok(Listener {
        listener,
        path: path.clone(),
        identity: (metadata.dev(), metadata.ino()),
    })
}

fn check_peer(stream: &UnixStream) -> Result<()> {
    check_uid(stream.peer_cred()?.uid(), uid())
}

impl Listener {
    pub(crate) async fn accept(&mut self) -> Result<Stream> {
        let (stream, _) = self.listener.accept().await?;
        check_peer(&stream)?;
        Ok(Box::new(stream))
    }
}

pub(super) async fn connect(path: &Address) -> Result<Stream> {
    ensure_root(
        path.parent()
            .ok_or_else(|| Error::protocol("Invalid owner endpoint"))?,
    )?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() || metadata.uid() != uid() || metadata.mode() & 0o077 != 0
    {
        return Err(Error::Unavailable(
            "Local owner socket is not private".into(),
        ));
    }
    let stream = UnixStream::connect(path).await?;
    check_peer(&stream)?;
    Ok(Box::new(stream))
}

impl Drop for Listener {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.file_type().is_socket() && (m.dev(), m.ino()) == self.identity)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn check_uid(actual: u32, expected: u32) -> Result<()> {
    if actual != expected {
        return Err(Error::Unavailable(
            "Local owner peer is not the current user".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn production_socket_root_is_short_and_peer_identity_cannot_change_users() {
        let root = system_root().unwrap();
        assert!(
            address(&root, &"a".repeat(64))
                .unwrap()
                .as_os_str()
                .as_bytes()
                .len()
                < 104
        );
        assert!(check_uid(uid(), uid()).is_ok());
        assert!(check_uid(uid().wrapping_add(1), uid()).is_err());
    }
}

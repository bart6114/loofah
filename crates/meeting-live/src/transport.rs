use crate::*;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

fn uid() -> u32 {
    unsafe { libc::geteuid() }
}
fn io_error(e: impl std::fmt::Display) -> LiveError {
    LiveError::new(
        "desktop_unavailable",
        format!("Cannot connect to Loofah desktop: {e}"),
    )
}

pub fn endpoint(identity: &Identity) -> PathBuf {
    let mut hash = Sha256::new();
    hash.update(identity.channel.as_bytes());
    hash.update([0]);
    hash.update(identity.vault.as_os_str().as_encoded_bytes());
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    PathBuf::from(format!("/tmp/loofah-live-{}", uid())).join(format!("{}.sock", &digest[..32]))
}

fn validate_directory(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path).map_err(io_error)?;
    if !meta.is_dir() || meta.uid() != uid() || meta.mode() & 0o077 != 0 {
        return Err(LiveError::new(
            "unsafe_endpoint",
            "Live endpoint directory must be owned by this user with mode 0700",
        ));
    }
    Ok(())
}
fn validate_socket(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path).map_err(io_error)?;
    if !meta.file_type().is_socket() || meta.uid() != uid() || meta.mode() & 0o077 != 0 {
        return Err(LiveError::new(
            "unsafe_endpoint",
            "Live socket must be owned by this user with mode 0600",
        ));
    }
    Ok(())
}

pub fn verify_peer(stream: &UnixStream) -> Result<()> {
    if stream.peer_cred().map_err(io_error)?.uid() != uid() {
        return Err(LiveError::new(
            "permission_denied",
            "Live tools only accept connections from the same user",
        ));
    }
    Ok(())
}

pub struct Server {
    pub listener: UnixListener,
    path: PathBuf,
    _lock: File,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
impl Server {
    pub fn bind(identity: &Identity) -> Result<Self> {
        Self::bind_at(endpoint(identity))
    }
    fn bind_at(path: PathBuf) -> Result<Self> {
        let directory = path.parent().unwrap();
        match std::fs::DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_error(e)),
        }
        validate_directory(directory)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path.with_extension("lock"))
            .map_err(io_error)?;
        let meta = lock.metadata().map_err(io_error)?;
        if !meta.is_file() || meta.uid() != uid() || meta.mode() & 0o077 != 0 {
            return Err(LiveError::new("unsafe_endpoint", "Unsafe endpoint lock"));
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(LiveError::new(
                "endpoint_in_use",
                "Another desktop instance owns this live endpoint",
            ));
        }
        if path.exists() {
            validate_socket(&path)?;
            std::fs::remove_file(&path).map_err(io_error)?;
        }
        let listener = UnixListener::bind(&path).map_err(io_error)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(io_error)?;
        Ok(Self {
            listener,
            path,
            _lock: lock,
        })
    }
}

pub async fn read<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let len = stream.read_u32().await.map_err(io_error)? as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(LiveError::new(
            "invalid_input",
            "Live protocol frame is too large or empty",
        ));
    }
    let mut bytes = vec![0; len];
    stream.read_exact(&mut bytes).await.map_err(io_error)?;
    serde_json::from_slice(&bytes).map_err(|e| {
        LiveError::new(
            "invalid_input",
            format!("Invalid live protocol message: {e}"),
        )
    })
}
pub async fn write<T: Serialize>(stream: &mut UnixStream, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io_error)?;
    if bytes.len() > MAX_FRAME {
        return Err(LiveError::new(
            "response_too_large",
            "Live response exceeded its size limit",
        ));
    }
    stream
        .write_u32(bytes.len() as u32)
        .await
        .map_err(io_error)?;
    stream.write_all(&bytes).await.map_err(io_error)?;
    Ok(())
}
pub async fn call(envelope: Envelope) -> Result<Response> {
    let path = endpoint(&envelope.identity);
    validate_directory(path.parent().unwrap())?;
    validate_socket(&path)?;
    let mut stream = UnixStream::connect(path).await.map_err(io_error)?;
    verify_peer(&stream)?;
    write(&mut stream, &envelope).await?;
    read(&mut stream).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn socket_round_trip_and_exclusive_ownership() {
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let path = temp.path().join("private/live.sock");
        let server = Server::bind_at(path.clone()).unwrap();
        assert!(Server::bind_at(path.clone()).is_err());
        let client_path = path.clone();
        let task = tokio::spawn(async move {
            let mut stream = UnixStream::connect(client_path).await.unwrap();
            verify_peer(&stream).unwrap();
            write(&mut stream, &Request::Status).await.unwrap();
            read::<Response>(&mut stream).await.unwrap()
        });
        let (mut stream, _) = server.listener.accept().await.unwrap();
        verify_peer(&stream).unwrap();
        assert!(matches!(
            read::<Request>(&mut stream).await.unwrap(),
            Request::Status
        ));
        write(
            &mut stream,
            &Response::Status(Status {
                active_meeting: None,
            }),
        )
        .await
        .unwrap();
        assert!(matches!(task.await.unwrap(), Response::Status(_)));
        drop(server);
        assert!(!path.exists());
        assert!(Server::bind_at(path).is_ok());
    }
    #[tokio::test]
    async fn rejects_oversized_frames_and_insecure_directories() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        a.write_u32(MAX_FRAME as u32 + 1).await.unwrap();
        assert_eq!(
            read::<Request>(&mut b).await.unwrap_err().code,
            "invalid_input"
        );
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Server::bind_at(temp.path().join("live.sock")).is_err());
    }
    #[tokio::test]
    async fn recovers_stale_socket_but_rejects_symlinks() {
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let directory = temp.path().join("private");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let path = directory.join("live.sock");
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        drop(Server::bind_at(path.clone()).unwrap());
        std::os::unix::fs::symlink(temp.path(), &path).unwrap();
        assert!(Server::bind_at(path).is_err());
    }

    #[test]
    fn handshake_rejects_other_versions_and_identities() {
        let identity = Identity {
            channel: "stable".into(),
            vault: "/a".into(),
        };
        let mut envelope = Envelope {
            version: PROTOCOL_VERSION,
            identity: identity.clone(),
            request: Request::Status,
        };
        assert!(envelope.validate(&identity).is_ok());
        envelope.version += 1;
        assert_eq!(
            envelope.validate(&identity).unwrap_err().code,
            "incompatible_protocol"
        );
        envelope.version = PROTOCOL_VERSION;
        envelope.identity.channel = "dev".into();
        assert_eq!(
            envelope.validate(&identity).unwrap_err().code,
            "identity_mismatch"
        );
    }

    #[test]
    fn endpoint_isolated_by_vault_and_channel() {
        let mut identity = Identity {
            channel: "stable".into(),
            vault: "/a".into(),
        };
        let first = endpoint(&identity);
        identity.channel = "dev".into();
        assert_ne!(first, endpoint(&identity));
        identity.channel = "stable".into();
        identity.vault = "/b".into();
        assert_ne!(first, endpoint(&identity));
        assert!(first.as_os_str().len() < 104);
    }
}

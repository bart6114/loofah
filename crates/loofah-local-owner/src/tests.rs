use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use hypr_loofah_protocol::{
    LocalCliOperation, LocalCliOperationKind, LocalCliReply, LocalCliResponse, NoteSaveOutcome,
    NoteSnapshot, SaveNoteRequest,
};
use hypr_storage::vault::{LeaseAttempt, VaultLease};
use tokio::sync::Semaphore;

use super::*;

struct Fixture {
    _root: tempfile::TempDir,
    vault: PathBuf,
    endpoints: EndpointRoot,
    temporary: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::var_os("LOOFAH_OWNER_TEST_ROOT")
            .or_else(|| std::env::var_os("RUNNER_TEMP"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = tempfile::Builder::new()
            .prefix("oo-")
            .tempdir_in(base)
            .unwrap();
        let vault = root.path().join("v");
        std::fs::create_dir(&vault).unwrap();
        let endpoints = EndpointRoot::new(root.path().join("e")).unwrap();
        let temporary = endpoints.path().to_owned();
        Self {
            _root: root,
            vault,
            endpoints,
            temporary,
        }
    }
    fn lease(&self) -> VaultLease {
        match VaultLease::try_acquire_existing(&self.vault).unwrap() {
            LeaseAttempt::Acquired(lease) => lease,
            LeaseAttempt::Busy => panic!("unexpected owner"),
        }
    }
    async fn client(&self) -> OwnerClient {
        match OwnerClient::resolve(&self.vault, &self.endpoints, &self.temporary)
            .await
            .unwrap()
        {
            OwnerResolution::Connected(client) => client,
            OwnerResolution::Acquired(_) => panic!("owner lost its lease"),
        }
    }
}

struct Echo {
    calls: Arc<AtomicUsize>,
    gate: Option<Arc<Semaphore>>,
    text: String,
    blob: Arc<std::sync::Mutex<Option<Blob>>>,
}
impl Echo {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Arc::new(AtomicUsize::new(0)),
            gate: None,
            text: "hello".into(),
            blob: Default::default(),
        })
    }
}
impl OwnerHandler for Echo {
    fn operations(&self) -> Vec<LocalCliOperationKind> {
        vec![
            LocalCliOperationKind::Note,
            LocalCliOperationKind::SaveNote,
            LocalCliOperationKind::Attach,
            LocalCliOperationKind::Attachment,
        ]
    }
    fn execute(&self, request: OwnerRequest) -> OwnerFuture {
        let calls = self.calls.clone();
        let gate = self.gate.clone();
        let text = self.text.clone();
        let stored = self.blob.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::Release);
            if let Some(gate) = gate {
                gate.acquire().await.unwrap().forget();
            }
            let snapshot = |text| NoteSnapshot {
                markdown: Some(text),
                title: "title".into(),
                revision: "r".into(),
            };
            let (reply, blob) = match request.operation {
                LocalCliOperation::SaveNote { input, .. } => (
                    LocalCliReply::NoteSaved {
                        outcome: NoteSaveOutcome::Saved(snapshot(input.markdown)),
                    },
                    None,
                ),
                LocalCliOperation::Attach { .. } => {
                    *stored.lock().unwrap() = request.blob;
                    (
                        LocalCliReply::Attached {
                            attachment_id: "file.bin".into(),
                            relative_path: "sessions/note/attachments/file.bin".into(),
                        },
                        None,
                    )
                }
                LocalCliOperation::Attachment { .. } => (
                    LocalCliReply::Attachment {
                        filename: "file.bin".into(),
                    },
                    stored.lock().unwrap().clone(),
                ),
                _ => (
                    LocalCliReply::Note {
                        snapshot: snapshot(text),
                    },
                    None,
                ),
            };
            OwnerResponse {
                response: LocalCliResponse { result: Ok(reply) },
                blob,
            }
        })
    }
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn held_owner_without_endpoint_never_returns_an_independent_lease() {
    let fixture = Fixture::new();
    let lease = fixture.lease();
    let result = OwnerClient::resolve(&fixture.vault, &fixture.endpoints, &fixture.temporary).await;
    assert!(matches!(result, Err(Error::Unavailable(_))));
    assert!(matches!(
        VaultLease::try_acquire_existing(&fixture.vault).unwrap(),
        LeaseAttempt::Busy
    ));
    drop(lease);
    drop(fixture.lease());
}

#[tokio::test]
async fn client_waits_for_the_original_owner_to_finish_startup() {
    let fixture = Fixture::new();
    let lease = fixture.lease();
    let resolve = OwnerClient::resolve(&fixture.vault, &fixture.endpoints, &fixture.temporary);
    let start = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        OwnerServer::bind(lease, &fixture.endpoints, &fixture.temporary, Echo::new())
            .await
            .unwrap()
    };
    let (resolved, server) = tokio::join!(resolve, start);
    assert!(matches!(resolved, Ok(OwnerResolution::Connected(_))));
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn current_user_connection_binds_vault_and_delivers_a_typed_reply() {
    let fixture = Fixture::new();
    let handler = Echo::new();
    let server = OwnerServer::bind(
        fixture.lease(),
        &fixture.endpoints,
        &fixture.temporary,
        handler.clone(),
    )
    .await
    .unwrap();
    let output = fixture
        .client()
        .await
        .execute(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::Note {
                session_id: "note".into(),
            },
            None,
        )
        .await
        .unwrap();
    assert!(
        matches!(output.response.result.unwrap(),LocalCliReply::Note{snapshot:NoteSnapshot{markdown:Some(ref text),..}} if text=="hello")
    );
    assert_eq!(handler.calls.load(Ordering::Acquire), 1);
    server.shutdown().await.unwrap();
    drop(fixture.lease());
}

#[tokio::test]
async fn large_local_notes_are_chunked_without_a_new_total_content_limit() {
    let fixture = Fixture::new();
    let server = OwnerServer::bind(
        fixture.lease(),
        &fixture.endpoints,
        &fixture.temporary,
        Echo::new(),
    )
    .await
    .unwrap();
    let body = "x".repeat(33 * 1024 * 1024);
    let output = fixture
        .client()
        .await
        .execute(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::SaveNote {
                session_id: "note".into(),
                input: SaveNoteRequest {
                    markdown: body.clone(),
                    title: None,
                    expected_revision: "r".into(),
                },
            },
            None,
        )
        .await
        .unwrap();
    let LocalCliReply::NoteSaved {
        outcome: NoteSaveOutcome::Saved(snapshot),
    } = output.response.result.unwrap()
    else {
        panic!("wrong reply")
    };
    assert_eq!(snapshot.markdown.as_deref(), Some(body.as_str()));
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn raw_blobs_stream_separately_and_survive_exact_hash_verification() {
    let fixture = Fixture::new();
    let input = fixture._root.path().join("input");
    let data = vec![37; 150_001];
    std::fs::write(&input, &data).unwrap();
    let blob = Blob::copy_from(std::fs::File::open(input).unwrap(), &fixture.temporary)
        .await
        .unwrap();
    let handler = Echo::new();
    let server = OwnerServer::bind(
        fixture.lease(),
        &fixture.endpoints,
        &fixture.temporary,
        handler.clone(),
    )
    .await
    .unwrap();
    let output = fixture
        .client()
        .await
        .execute(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::Attach {
                session_id: "note".into(),
                filename: "file.bin".into(),
            },
            Some(blob),
        )
        .await
        .unwrap();
    assert!(matches!(
        output.response.result.unwrap(),
        LocalCliReply::Attached { .. }
    ));
    let output = fixture
        .client()
        .await
        .execute(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::Attachment {
                session_id: "note".into(),
                attachment_id: "file.bin".into(),
            },
            None,
        )
        .await
        .unwrap();
    let bytes = output
        .blob
        .unwrap()
        .with_file(|file| {
            use std::io::Read;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .await
        .unwrap();
    assert_eq!(bytes, data);
    *handler.blob.lock().unwrap() = None;
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn dropped_client_and_shutdown_waiter_cannot_release_an_admitted_operation_owner() {
    let fixture = Fixture::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Semaphore::new(0));
    let handler = Arc::new(Echo {
        calls: calls.clone(),
        gate: Some(gate.clone()),
        text: "saved".into(),
        blob: Default::default(),
    });
    let lease = fixture.lease();
    let original = Arc::downgrade(&lease.pin());
    let server = Arc::new(
        OwnerServer::bind(lease, &fixture.endpoints, &fixture.temporary, handler)
            .await
            .unwrap(),
    );
    let client = fixture.client().await;
    let request = tokio::spawn(async move {
        client
            .execute(
                uuid::Uuid::new_v4().to_string(),
                LocalCliOperation::SaveNote {
                    session_id: "note".into(),
                    input: SaveNoteRequest {
                        markdown: "saved".into(),
                        title: None,
                        expected_revision: "r".into(),
                    },
                },
                None,
            )
            .await
    });
    until(|| calls.load(Ordering::Acquire) == 1).await;
    request.abort();
    let _ = request.await;
    let owner = server.clone();
    let waiter = tokio::spawn(async move { owner.shutdown().await });
    tokio::task::yield_now().await;
    waiter.abort();
    let _ = waiter.await;
    assert!(original.upgrade().is_some());
    assert!(matches!(
        VaultLease::try_acquire_existing(&fixture.vault).unwrap(),
        LeaseAttempt::Busy
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(25), server.wait_for_shutdown())
            .await
            .is_err()
    );
    gate.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), server.wait_for_shutdown())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert!(original.upgrade().is_none());
    drop(fixture.lease());
}

#[tokio::test]
async fn concurrent_startup_has_one_kernel_owner_and_never_falls_back_from_busy() {
    let fixture = Fixture::new();
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let vault = fixture.vault.clone();
        let endpoints = fixture.endpoints.clone();
        let root = fixture.temporary.clone();
        tasks.push(tokio::spawn(async move {
            OwnerClient::resolve(&vault, &endpoints, &root).await
        }));
    }
    let mut leases = Vec::new();
    for task in tasks {
        match task.await.unwrap() {
            Ok(OwnerResolution::Acquired(lease)) => leases.push(lease),
            Err(Error::Unavailable(_)) => {}
            _ => panic!("unexpected owner resolution"),
        }
    }
    assert_eq!(leases.len(), 1);
    drop(leases);
    drop(fixture.lease());
}

#[tokio::test]
async fn malformed_control_is_bounded_and_never_dispatches_an_operation() {
    use tokio::io::AsyncWriteExt;
    let fixture = Fixture::new();
    let handler = Echo::new();
    let server = OwnerServer::bind(
        fixture.lease(),
        &fixture.endpoints,
        &fixture.temporary,
        handler.clone(),
    )
    .await
    .unwrap();
    let endpoint = fixture.endpoints.endpoint(&fixture.vault).unwrap();
    let mut stream = endpoint.connect().await.unwrap();
    stream.write_all(&(16_385u32).to_be_bytes()).await.unwrap();
    assert!(wire::read_control(&mut stream).await.is_err());
    assert_eq!(handler.calls.load(Ordering::Acquire), 0);
    server.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn sockets_and_runtime_directories_are_private_and_stale_cleanup_preserves_unknown_entries() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let fixture = Fixture::new();
    let endpoint = fixture.endpoints.endpoint(&fixture.vault).unwrap();
    std::fs::write(&endpoint.address, b"unknown user data").unwrap();
    assert!(
        OwnerServer::bind(
            fixture.lease(),
            &fixture.endpoints,
            &fixture.temporary,
            Echo::new()
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read(&endpoint.address).unwrap(),
        b"unknown user data"
    );
    std::fs::remove_file(&endpoint.address).unwrap();
    let stale = std::os::unix::net::UnixListener::bind(&endpoint.address).unwrap();
    drop(stale);
    let server = OwnerServer::bind(
        fixture.lease(),
        &fixture.endpoints,
        &fixture.temporary,
        Echo::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::metadata(fixture.endpoints.path()).unwrap().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&endpoint.address).unwrap().mode() & 0o777,
        0o600
    );
    fixture.client().await;
    server.shutdown().await.unwrap();
    assert!(!endpoint.address.exists());
    std::fs::set_permissions(
        fixture.endpoints.path(),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(EndpointRoot::new(fixture.endpoints.path().to_owned()).is_err());
}

#[tokio::test]
async fn owner_death_allows_fallback_only_after_kernel_lease_acquisition() {
    let fixture = Fixture::new();
    let ready = fixture._root.path().join("ready");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tests::owner_child", "--ignored", "--nocapture"])
        .env("LOOFAH_OWNER_CHILD_VAULT", &fixture.vault)
        .env("LOOFAH_OWNER_CHILD_ENDPOINTS", fixture.endpoints.path())
        .env("LOOFAH_OWNER_CHILD_READY", &ready)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    until(|| ready.exists()).await;
    let client = fixture.client().await;
    let old_epoch = client.epoch().to_owned();
    drop(client);
    child.kill().unwrap();
    child.wait().unwrap();
    let lease = match OwnerClient::resolve(&fixture.vault, &fixture.endpoints, &fixture.temporary)
        .await
        .unwrap()
    {
        OwnerResolution::Acquired(lease) => lease,
        _ => panic!("dead owner retained ownership"),
    };
    let server = OwnerServer::bind(lease, &fixture.endpoints, &fixture.temporary, Echo::new())
        .await
        .unwrap();
    assert_ne!(fixture.client().await.epoch(), old_epoch);
    server.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "child process fixture"]
async fn owner_child() {
    let Some(vault) = std::env::var_os("LOOFAH_OWNER_CHILD_VAULT") else {
        return;
    };
    let root = EndpointRoot::new(PathBuf::from(
        std::env::var_os("LOOFAH_OWNER_CHILD_ENDPOINTS").unwrap(),
    ))
    .unwrap();
    let lease = match VaultLease::try_acquire_existing(&PathBuf::from(vault)).unwrap() {
        LeaseAttempt::Acquired(lease) => lease,
        _ => panic!("owner collision"),
    };
    let _server = OwnerServer::bind(lease, &root, root.path(), Echo::new())
        .await
        .unwrap();
    std::fs::write(
        PathBuf::from(std::env::var_os("LOOFAH_OWNER_CHILD_READY").unwrap()),
        b"ready",
    )
    .unwrap();
    std::future::pending::<()>().await;
}

struct PanicHandler;
impl OwnerHandler for PanicHandler {
    fn operations(&self) -> Vec<LocalCliOperationKind> {
        vec![LocalCliOperationKind::SaveNote]
    }
    fn execute(&self, _request: OwnerRequest) -> OwnerFuture {
        Box::pin(async move { panic!("application worker fault") })
    }
}

#[tokio::test]
async fn application_worker_panic_is_unknown_and_cannot_claim_clean_owner_shutdown() {
    let fixture = Fixture::new();
    let lease = fixture.lease();
    let original = Arc::downgrade(&lease.pin());
    let server = OwnerServer::bind(
        lease,
        &fixture.endpoints,
        &fixture.temporary,
        Arc::new(PanicHandler),
    )
    .await
    .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let error = fixture
        .client()
        .await
        .execute(
            id.clone(),
            LocalCliOperation::SaveNote {
                session_id: "note".into(),
                input: SaveNoteRequest {
                    markdown: "saved".into(),
                    title: None,
                    expected_revision: "r".into(),
                },
            },
            None,
        )
        .await
        .err()
        .unwrap();
    assert!(matches!(error,Error::Unknown{request_id} if request_id==id));
    assert!(matches!(
        server.shutdown().await,
        Err(Error::ApplicationWorkerLost)
    ));
    assert!(original.upgrade().is_some());
    assert!(matches!(
        VaultLease::try_acquire_existing(&fixture.vault).unwrap(),
        LeaseAttempt::Busy
    ));
}

#[test]
fn cancelled_blob_copy_retains_original_owner_until_actual_queued_copy_finishes() {
    use std::future::Future;
    let fixture = Fixture::new();
    let source = fixture._root.path().join("source.bin");
    std::fs::write(&source, vec![17u8; 1024 * 1024]).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (entered_send, entered_receive) = std::sync::mpsc::channel();
        let (release_send, release_receive) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            entered_send.send(()).unwrap();
            release_receive.recv().unwrap();
        });
        entered_receive
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        struct Owner {
            lease: Option<VaultLease>,
            dropped: std::sync::mpsc::Sender<()>,
        }
        impl Drop for Owner {
            fn drop(&mut self) {
                drop(self.lease.take());
                let _ = self.dropped.send(());
            }
        }
        let (dropped_send, dropped_receive) = std::sync::mpsc::channel();
        let owner = Owner {
            lease: Some(fixture.lease()),
            dropped: dropped_send,
        };
        let temporary = fixture.temporary.clone();
        let (queued_send, queued_receive) = tokio::sync::oneshot::channel();
        let copy = tokio::spawn(async move {
            let operation = Blob::copy_from_owned(
                std::fs::File::open(source).unwrap(),
                &temporary,
                Box::new(owner),
            );
            tokio::pin!(operation);
            std::future::poll_fn(|cx| match operation.as_mut().poll(cx) {
                std::task::Poll::Pending => std::task::Poll::Ready(()),
                std::task::Poll::Ready(_) => {
                    panic!("copy bypassed the blocked actual I/O executor")
                }
            })
            .await;
            queued_send.send(()).unwrap();
            operation.await
        });
        queued_receive.await.unwrap();
        copy.abort();
        match copy.await {
            Err(cause) => assert!(cause.is_cancelled()),
            Ok(_) => panic!("copy waiter was not cancelled"),
        }
        assert!(dropped_receive.try_recv().is_err());
        assert!(matches!(
            VaultLease::try_acquire_existing(&fixture.vault).unwrap(),
            LeaseAttempt::Busy
        ));
        release_send.send(()).unwrap();
        blocker.await.unwrap();
        dropped_receive
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match VaultLease::try_acquire_existing(&fixture.vault).unwrap() {
                LeaseAttempt::Acquired(_) => break,
                LeaseAttempt::Busy => {
                    // A concurrent fork can retain a CLOEXEC descriptor until exec completes.
                    assert!(
                        std::time::Instant::now() < deadline,
                        "owner outlived actual copy"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    });
}

#[cfg(unix)]
#[test]
fn concurrent_pre_exec_can_retain_a_dropped_owners_cloexec_lease() {
    use std::{
        io::{Read, Write},
        os::{
            fd::AsRawFd,
            unix::{net::UnixStream, process::CommandExt},
        },
        process::Command,
    };

    let fixture = Fixture::new();
    let lease = fixture.lease();
    let (mut parent, child) = UnixStream::pair().unwrap();
    parent
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let spawned = std::thread::spawn(move || {
        let mut command = Command::new("/usr/bin/true");
        // Only async-signal-safe syscalls run between fork and exec.
        unsafe {
            command.pre_exec(move || {
                let fd = child.as_raw_fd();
                let byte = [1u8];
                if libc::write(fd, byte.as_ptr().cast(), 1) != 1 {
                    return Err(std::io::Error::last_os_error());
                }
                let mut poll = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                if libc::poll(&mut poll, 1, 10_000) != 1 {
                    return Err(std::io::Error::from_raw_os_error(libc::ETIMEDOUT));
                }
                let mut release = [0u8];
                if libc::read(fd, release.as_mut_ptr().cast(), 1) != 1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.status()
    });
    let mut entered = [0u8];
    parent.read_exact(&mut entered).unwrap();
    drop(lease);
    let inherited = VaultLease::try_acquire_existing(&fixture.vault).unwrap();
    parent.write_all(&[1]).unwrap();
    assert!(spawned.join().unwrap().unwrap().success());
    assert!(matches!(inherited, LeaseAttempt::Busy));
    drop(inherited);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match VaultLease::try_acquire_existing(&fixture.vault).unwrap() {
            LeaseAttempt::Acquired(_) => break,
            LeaseAttempt::Busy => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "inherited owner was not released"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[cfg(feature = "core")]
#[path = "core_tests.rs"]
mod core_tests;

#[tokio::test]
async fn owner_resolution_retries_a_lost_handshake_until_the_original_lease_is_released() {
    let fixture = Fixture::new();
    let lease = fixture.lease();
    let endpoint = fixture.endpoints.endpoint(lease.canonical_vault()).unwrap();
    let mut listener = endpoint.bind(&lease).await.unwrap();
    let vault = fixture.vault.clone();
    let endpoints = fixture.endpoints.clone();
    let temporary = fixture.temporary.clone();
    let resolving =
        tokio::spawn(async move { OwnerClient::resolve(&vault, &endpoints, &temporary).await });
    let mut io = listener.accept().await.unwrap();
    assert!(matches!(
        wire::read_control(&mut io).await.unwrap(),
        wire::Control::Hello { .. }
    ));
    drop(io);
    drop(listener);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !resolving.is_finished(),
        "A transient handshake loss cannot decide that the original owner has released storage"
    );
    drop(lease);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), resolving)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        OwnerResolution::Acquired(_)
    ));
}

#[path = "progress_transport_tests.rs"]
mod progress_transport;

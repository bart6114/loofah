use super::*;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::AsyncWrite;

fn payload(empty: bool) -> Vec<u8> {
    if empty {
        Vec::new()
    } else {
        (0..3 * RECORDING_CHUNK_LIMIT + 17)
            .map(|index| (index % 251) as u8)
            .collect()
    }
}

fn large_payload() -> Vec<u8> {
    (0..9 * RECORDING_CHUNK_LIMIT + 17)
        .map(|index| (index % 251) as u8)
        .collect()
}

fn lease(name: &str) -> AttachmentLease {
    let bytes = if name == "slow-large.bin" {
        large_payload()
    } else {
        payload(name == "empty.bin")
    };
    AttachmentLease {
        id: "0cfd4e78-6597-41fa-a6b7-2cd4bc26aa39".into(),
        vault_id: "test".into(),
        session_id: "note".into(),
        attachment_id: name.into(),
        idle_timeout_secs: 600,
        metadata: FileMetadata {
            filename: name.into(),
            content_type: "application/octet-stream".into(),
            size_bytes: bytes.len() as u64,
            sha256: hex(&Sha256::digest(&bytes)),
        },
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn open(state: Arc<Provider>, id: &str, name: &str) -> axum::response::Response {
    state.download_opens.fetch_add(1, Ordering::SeqCst);
    let mode = state.attachment_mode.load(Ordering::SeqCst);
    let mut value = lease(name);
    value.id = id.into();
    if mode == 36 {
        value.metadata.sha256 = "f".repeat(64);
    }
    state.attachment_mode.store(30, Ordering::SeqCst);
    Json(value).into_response()
}

pub(super) async fn read(state: Arc<Provider>, headers: HeaderMap) -> axum::response::Response {
    let mode = state.attachment_mode.load(Ordering::SeqCst);
    let empty = headers["if-match"] == format!("\"sha256:{}\"", lease("empty.bin").metadata.sha256);
    let metadata = lease(if empty {
        "empty.bin"
    } else if mode == 49 {
        "slow-large.bin"
    } else {
        "download.bin"
    })
    .metadata;
    assert_eq!(
        headers["if-match"],
        format!("\"sha256:{}\"", metadata.sha256)
    );
    assert_eq!(headers["accept-encoding"], "identity");
    let range = headers.get("range").map(|value| {
        value
            .to_str()
            .unwrap()
            .strip_prefix("bytes=")
            .unwrap()
            .strip_suffix('-')
            .unwrap()
            .parse::<usize>()
            .unwrap()
    });
    let start = range.unwrap_or(0);
    state.download_offsets.lock().await.push(start as u64);
    if matches!(mode, 34 | 36) {
        return (
            StatusCode::NOT_FOUND,
            Json(failure(ErrorCode::NotFound, "Lease expired")),
        )
            .into_response();
    }
    if matches!(mode, 31 | 33 | 35) {
        state.attachment_mode.store(
            match mode {
                31 => 30,
                33 => 34,
                _ => 36,
            },
            Ordering::SeqCst,
        );
    }
    let mut bytes = if mode == 49 {
        large_payload()
    } else {
        payload(empty)
    };
    if mode == 43 && !bytes.is_empty() {
        bytes[2 * RECORDING_CHUNK_LIMIT + 11] ^= 1;
    }
    let bytes = Arc::new(bytes);
    let length = bytes.len() - start;
    let interrupted = matches!(mode, 31 | 33 | 35);
    let stream = futures_util::stream::unfold(
        (bytes, start, false),
        move |(bytes, cursor, sent)| async move {
            if interrupted && sent {
                tokio::time::sleep(Duration::from_millis(100)).await;
                return Some((
                    Err::<Vec<u8>, _>(std::io::Error::new(
                        std::io::ErrorKind::ConnectionReset,
                        "Interrupted fixture",
                    )),
                    (bytes, cursor, true),
                ));
            }
            if cursor >= bytes.len() {
                return None;
            }
            if mode == 46 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            if mode == 47 && sent {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
            let end = (cursor + 64 * 1024).min(bytes.len());
            let data = bytes[cursor..end].to_vec();
            Some((Ok(data), (bytes, end, true)))
        },
    );
    let mut builder = axum::response::Response::builder()
        .status(if range.is_some() || mode == 39 {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(
            "content-type",
            if mode == 37 {
                "image/png"
            } else {
                "application/octet-stream"
            },
        )
        .header(
            "etag",
            if mode == 38 {
                "bad-etag".into()
            } else {
                format!("\"sha256:{}\"", metadata.sha256)
            },
        );
    if mode != 44 {
        builder = builder.header(
            "content-length",
            if mode == 40 { length + 1 } else { length },
        );
    }
    if mode == 41 {
        builder = builder.header("content-encoding", "gzip");
    }
    if range.is_some() {
        builder = builder.header(
            "content-range",
            format!(
                "bytes {start}-{}/{}",
                metadata.size_bytes - 1,
                metadata.size_bytes
            ),
        );
    }
    builder.body(axum::body::Body::from_stream(stream)).unwrap()
}

async fn client(fixture: &Fixture, mode: usize) -> Arc<RemoteClient> {
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("attachment_leases".into());
    fixture.login(&client).await;
    fixture.state.attachment_mode.store(mode, Ordering::SeqCst);
    client
}

#[tokio::test]
async fn downloads_complete_images_larger_than_one_chunk_and_empty_files_over_https() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let client = client(&fixture, 30).await;
    for name in ["download.bin", "empty.bin"] {
        let result = client
            .download_attachment(lease(name), Vec::new(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result, payload(name == "empty.bin"));
    }
    assert_eq!(*fixture.state.download_offsets.lock().await, [0, 0]);
}

#[tokio::test]
async fn healthy_streams_at_500ms_rtt_continue_beyond_the_recording_request_deadline() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(500))).await;
    let client = client(&fixture, 49).await;
    let started = std::time::Instant::now();
    let result = client
        .download_attachment(
            lease("slow-large.bin"),
            Vec::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result, large_payload());
    assert!(started.elapsed() > Duration::from_secs(35));
    assert_eq!(*fixture.state.download_offsets.lock().await, [0]);
}

#[tokio::test]
async fn downloads_resume_received_bytes_and_reopen_only_the_same_generation_after_restart() {
    for mode in [31, 33, 35] {
        let fixture = Fixture::with_tls(Some(Duration::from_millis(150))).await;
        let client = client(&fixture, mode).await;
        let result = client
            .download_attachment(lease("download.bin"), Vec::new(), CancellationToken::new())
            .await;
        if mode == 35 {
            assert_eq!(result.unwrap_err().code, ErrorCode::Conflict);
        } else {
            assert_eq!(result.unwrap(), payload(false));
        }
        let offsets = fixture.state.download_offsets.lock().await;
        assert_eq!(offsets[0], 0);
        assert!(offsets[1] > 0 && offsets[1] < lease("download.bin").metadata.size_bytes);
        if mode == 33 {
            assert_eq!(offsets.len(), 3);
            assert_eq!(offsets[1], offsets[2]);
        }
        assert_eq!(
            fixture.state.download_opens.load(Ordering::SeqCst),
            usize::from(mode != 31)
        );
    }
}

#[tokio::test]
async fn download_headers_and_full_checksum_are_validated_before_a_sink_is_returned() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let client = client(&fixture, 30).await;
    for mode in [37, 38, 39, 40, 41, 43, 44] {
        fixture.state.attachment_mode.store(mode, Ordering::SeqCst);
        let cause = client
            .download_attachment(lease("download.bin"), Vec::new(), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(
            cause.code,
            if mode == 43 {
                ErrorCode::Conflict
            } else {
                ErrorCode::IncompatibleVersion
            },
            "mode {mode}"
        );
    }
}

#[tokio::test]
async fn slow_progress_uses_an_idle_deadline_and_stalls_exhaust_bounded_retries() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let client = client(&fixture, 46).await;
    let started = std::time::Instant::now();
    let (result, ()) = client
        .download_attachment_inner(
            lease("download.bin"),
            Vec::new(),
            (),
            CancellationToken::new(),
            Duration::from_millis(500),
        )
        .await
        .unwrap();
    assert_eq!(result, payload(false));
    assert!(started.elapsed() > Duration::from_millis(500));
    fixture.state.attachment_mode.store(47, Ordering::SeqCst);
    fixture.state.download_offsets.lock().await.clear();
    let cause = client
        .download_attachment_inner(
            lease("download.bin"),
            Vec::new(),
            (),
            CancellationToken::new(),
            Duration::from_millis(500),
        )
        .await
        .unwrap_err();
    assert_eq!(cause.code, ErrorCode::Unavailable);
    assert_eq!(fixture.state.download_offsets.lock().await.len(), 3);
}

struct SlowSink {
    flushing: Arc<tokio::sync::Notify>,
    finish: tokio::sync::oneshot::Receiver<()>,
    dropped: Arc<AtomicBool>,
}
impl AsyncWrite for SlowSink {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.flushing.notify_one();
        match Pin::new(&mut self.finish).poll(cx) {
            Poll::Ready(_) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
impl Drop for SlowSink {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn cancelled_download_callers_retain_admission_until_the_actual_sink_flush_exits() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let client = client(&fixture, 30).await;
    for abort in [true, false] {
        let admission = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = admission.clone().acquire_owned().await.unwrap();
        let flushing = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let (finish, blocked) = tokio::sync::oneshot::channel();
        let sink = SlowSink {
            flushing: flushing.clone(),
            finish: blocked,
            dropped: dropped.clone(),
        };
        let caller = client.clone();
        let cancel = CancellationToken::new();
        let stopped = cancel.clone();
        let request = tokio::spawn(async move {
            caller
                .download_attachment_with_guard(lease("download.bin"), sink, permit, stopped)
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), flushing.notified())
            .await
            .unwrap();
        let request = if abort {
            request.abort();
            let Err(cause) = request.await else {
                panic!("caller was not cancelled")
            };
            assert!(cause.is_cancelled());
            None
        } else {
            cancel.cancel();
            assert!(!request.is_finished());
            Some(request)
        };
        assert!(admission.clone().try_acquire_owned().is_err());
        assert!(!dropped.load(Ordering::SeqCst));
        finish.send(()).unwrap();
        if let Some(request) = request {
            match request.await.unwrap() {
                Err(cause) => assert_eq!(cause.code, ErrorCode::Cancelled),
                Ok(_) => panic!("cancelled download was delivered"),
            }
        }
        let _permit = tokio::time::timeout(Duration::from_secs(2), admission.acquire())
            .await
            .unwrap()
            .unwrap();
        assert!(dropped.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn downloads_verify_a_private_file_before_publication_and_clean_up_failed_sinks() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let client = client(&fixture, 30).await;
    let root = tempfile::tempdir().unwrap();
    let final_path = root.path().join("original.bin");
    let staging = tempfile::NamedTempFile::new_in(root.path()).unwrap();
    let file = tokio::fs::File::from_std(staging.reopen().unwrap());
    let (file, staging) = client
        .download_attachment_with_guard(
            lease("download.bin"),
            file,
            staging,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!final_path.exists());
    file.sync_all().await.unwrap();
    drop(file);
    staging.persist_noclobber(&final_path).unwrap();
    assert_eq!(std::fs::read(&final_path).unwrap(), payload(false));
    fixture.state.attachment_mode.store(43, Ordering::SeqCst);
    let staging = tempfile::NamedTempFile::new_in(root.path()).unwrap();
    let private = staging.path().to_path_buf();
    let file = tokio::fs::File::from_std(staging.reopen().unwrap());
    assert_eq!(
        client
            .download_attachment_with_guard(
                lease("download.bin"),
                file,
                staging,
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert!(!private.exists());
    fixture.state.attachment_mode.store(47, Ordering::SeqCst);
    fixture.state.download_offsets.lock().await.clear();
    let staging = tempfile::NamedTempFile::new_in(root.path()).unwrap();
    let private = staging.path().to_path_buf();
    let file = tokio::fs::File::from_std(staging.reopen().unwrap());
    let cancel = CancellationToken::new();
    let stopped = cancel.clone();
    let request = tokio::spawn(async move {
        client
            .download_attachment_with_guard(lease("download.bin"), file, staging, stopped)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.download_offsets.lock().await.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    assert_eq!(
        request.await.unwrap().unwrap_err().code,
        ErrorCode::Cancelled
    );
    assert!(!private.exists());
    assert_eq!(std::fs::read(final_path).unwrap(), payload(false));
}

#[tokio::test]
async fn download_capability_and_pre_cancelled_tokens_run_before_credential_access() {
    let fixture = Fixture::new().await;
    let client = fixture.client().await;
    assert_eq!(
        client
            .download_attachment(lease("download.bin"), Vec::new(), CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::IncompatibleVersion
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        client
            .download_attachment(lease("download.bin"), Vec::new(), cancel)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(fixture.state.issued.load(Ordering::SeqCst), 0);
    assert!(fixture.credentials.values.lock().await.is_empty());
}

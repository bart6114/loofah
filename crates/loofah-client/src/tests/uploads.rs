use super::*;
use axum::{
    body::{Body, to_bytes},
    extract::Path,
};
use std::io::{self, Read};

fn authenticated(headers: &HeaderMap) {
    assert!(
        headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Bearer access-")
    );
    assert_eq!(headers["x-loofah-api-major"], "1");
    assert!(uuid::Uuid::parse_str(headers["x-request-id"].to_str().unwrap()).is_ok());
}
fn metadata(input: &UploadAttachmentRequest, mode: usize) -> Value {
    let mut value = json!({"filename":"chart + # ü 1.pdf","contentType":"application/pdf","sizeBytes":input.size_bytes,"sha256":input.sha256});
    match mode {
        50 => value["sha256"] = json!("0".repeat(64)),
        51 => value["sizeBytes"] = json!(input.size_bytes + 1),
        52 => value["filename"] = json!("../notes.md"),
        53 => value["contentType"] = json!("text/plain\r\ninjected: true"),
        _ => {}
    }
    value
}
pub(super) async fn receive(
    State(state): State<Arc<Provider>>,
    Path((vault, session)): Path<(String, String)>,
    Query(input): Query<UploadAttachmentRequest>,
    headers: HeaderMap,
    body: Body,
) -> axum::response::Response {
    authenticated(&headers);
    assert_eq!((vault.as_str(), session.as_str()), ("test", "note"));
    assert_eq!(input.filename, "chart + # ü.pdf");
    assert!(headers.get("content-length").is_none());
    assert_eq!(headers["content-type"], "application/octet-stream");
    state.upload_attempts.fetch_add(1, Ordering::SeqCst);
    let bytes = match to_bytes(body, 2 * 1024 * 1024).await {
        Ok(bytes) => bytes.to_vec(),
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let sha: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if bytes.len() as u64 != input.size_bytes || sha != input.sha256 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    state.attachment_uploads.lock().await.push((
        headers["x-request-id"].to_str().unwrap().into(),
        input.clone(),
        bytes,
    ));
    let mode = state.attachment_mode.load(Ordering::SeqCst);
    if mode == 54 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Json(metadata(&input, mode)).into_response()
}
pub(super) async fn status(
    State(state): State<Arc<Provider>>,
    Path((vault, session, upload)): Path<(String, String, String)>,
    Query(input): Query<UploadAttachmentRequest>,
    headers: HeaderMap,
) -> axum::response::Response {
    authenticated(&headers);
    assert_eq!((vault.as_str(), session.as_str()), ("test", "note"));
    if state.attachment_mode.load(Ordering::SeqCst) == 57 {
        return Json(json!({"kind":"uncertain"})).into_response();
    }
    let received = state.attachment_uploads.lock().await;
    let Some((_, declaration, _)) = received.iter().find(|(id, _, _)| id == &upload) else {
        return Json(json!({"kind":"missing"})).into_response();
    };
    assert_eq!(declaration, &input);
    Json(json!({"kind":"committed","value":metadata(&input, state.attachment_mode.load(Ordering::SeqCst))})).into_response()
}

fn upload(bytes: &[u8]) -> AttachmentUpload {
    AttachmentUpload {
        vault_id: "test".into(),
        session_id: "note".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        input: UploadAttachmentRequest {
            filename: "chart + # ü.pdf".into(),
            size_bytes: bytes.len() as u64,
            sha256: Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        },
    }
}
async fn capable(f: &Fixture) -> Arc<RemoteClient> {
    let mut client = f.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("attachment_upload".into());
    client
}
async fn state(
    client: &Arc<RemoteClient>,
    upload: &AttachmentUpload,
) -> Result<AttachmentUploadStatus> {
    let reply = client
        .execute(
            RemoteOperation::AttachmentUploadStatus {
                upload: upload.clone(),
            },
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await?;
    let RemoteReply::AttachmentUploadStatus(status) = reply else {
        panic!("wrong upload status reply");
    };
    Ok(status)
}

#[tokio::test]
async fn binary_uploads_and_status_use_authenticated_https_with_escaped_declarations() {
    let f = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let client = capable(&f).await;
    f.login(&client).await;
    for bytes in [vec![0_u8; 256 * 1024 + 17], Vec::new()] {
        let upload = upload(&bytes);
        assert_eq!(
            state(&client, &upload).await.unwrap(),
            AttachmentUploadStatus::Missing
        );
        let result = client
            .upload_attachment(
                upload.clone(),
                io::Cursor::new(bytes.clone()),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.filename, "chart + # ü 1.pdf");
        assert_eq!(result.sha256, upload.input.sha256);
        assert_eq!(
            state(&client, &upload).await.unwrap(),
            AttachmentUploadStatus::Committed(result)
        );
        assert_eq!(
            f.state.attachment_uploads.lock().await.last().unwrap(),
            &(upload.request_id, upload.input, bytes)
        );
    }
    assert_eq!(f.state.upload_attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn completed_replay_and_uncertain_status_never_consume_the_source_or_start_another_upload() {
    let f = Fixture::new().await;
    let client = capable(&f).await;
    f.login(&client).await;
    let upload = upload(b"payload");
    let original = client
        .upload_attachment(
            upload.clone(),
            io::Cursor::new(b"payload"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let read = Arc::new(AtomicUsize::new(0));
    assert_eq!(
        client
            .upload_attachment(
                upload.clone(),
                CountRead(read.clone()),
                CancellationToken::new()
            )
            .await
            .unwrap(),
        original
    );
    f.state.attachment_mode.store(57, Ordering::SeqCst);
    let error = client
        .upload_attachment(upload, CountRead(read.clone()), CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
    assert_eq!(read.load(Ordering::SeqCst), 0);
    assert_eq!(f.state.upload_attempts.load(Ordering::SeqCst), 1);
}

struct CountRead(Arc<AtomicUsize>);
impl Read for CountRead {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    }
}
#[tokio::test]
async fn unsupported_or_invalid_uploads_fail_before_credentials_network_or_source_reads() {
    let f = Fixture::new().await;
    let read = Arc::new(AtomicUsize::new(0));
    let client = f.client().await;
    let input = upload(b"");
    let error = client
        .upload_attachment(
            input.clone(),
            CountRead(read.clone()),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::IncompatibleVersion);
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert_eq!(error.request_id.as_deref(), Some(input.request_id.as_str()));
    assert_eq!(
        state(&client, &input).await.unwrap_err().code,
        ErrorCode::IncompatibleVersion
    );
    let client = capable(&f).await;
    for mode in ["uuid", "filename", "hash", "vault", "session"] {
        let mut input = input.clone();
        match mode {
            "uuid" => input.request_id = "bad".into(),
            "filename" => input.input.filename = "../notes.md".into(),
            "hash" => input.input.sha256 = "A".repeat(64),
            "vault" => input.vault_id = "../vault".into(),
            _ => input.session_id = ".".into(),
        }
        let error = client
            .upload_attachment(input, CountRead(read.clone()), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
    }
    assert_eq!(read.load(Ordering::SeqCst), 0);
    assert_eq!(f.state.issued.load(Ordering::SeqCst), 0);
    assert_eq!(f.state.upload_attempts.load(Ordering::SeqCst), 0);
}

struct ErrorAfterBytes {
    bytes: io::Cursor<Vec<u8>>,
    panic: bool,
}
impl Read for ErrorAfterBytes {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        let read = self.bytes.read(target)?;
        if read != 0 {
            return Ok(read);
        }
        assert!(!self.panic, "injected source panic");
        Err(io::Error::other("device path must not escape"))
    }
}
#[tokio::test]
async fn incomplete_changed_errored_and_panicking_sources_never_complete_the_request() {
    let f = Fixture::new().await;
    let client = capable(&f).await;
    f.login(&client).await;
    for (attempt, mode) in ["short", "long", "hash", "error", "panic"]
        .into_iter()
        .enumerate()
    {
        let mut input = upload(b"payload");
        let source: Box<dyn Read + Send> = match mode {
            "short" => Box::new(io::Cursor::new(b"short".to_vec())),
            "long" => Box::new(io::Cursor::new(b"payload extra".to_vec())),
            "hash" => {
                input.input.sha256 = "0".repeat(64);
                Box::new(io::Cursor::new(b"payload".to_vec()))
            }
            _ => Box::new(ErrorAfterBytes {
                bytes: io::Cursor::new(b"payload".to_vec()),
                panic: mode == "panic",
            }),
        };
        let error = client
            .upload_attachment(input, source, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
        assert!(!error.message.contains("device path"));
        assert!(f.state.upload_attempts.load(Ordering::SeqCst) <= attempt + 1);
        assert!(f.state.attachment_uploads.lock().await.is_empty());
    }
}

#[tokio::test]
async fn malformed_upload_and_status_metadata_are_refused_without_automatic_mutation_retries() {
    let f = Fixture::new().await;
    let client = capable(&f).await;
    f.login(&client).await;
    for mode in 50..=53 {
        f.state.attachment_mode.store(mode, Ordering::SeqCst);
        let upload = upload(b"payload");
        let error = client
            .upload_attachment(
                upload.clone(),
                io::Cursor::new(b"payload"),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::IncompatibleVersion);
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
        assert_eq!(
            state(&client, &upload).await.unwrap_err().code,
            ErrorCode::IncompatibleVersion
        );
    }
    assert_eq!(f.state.upload_attempts.load(Ordering::SeqCst), 4);
}

struct SlowRead(io::Cursor<Vec<u8>>);
impl Read for SlowRead {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        std::thread::sleep(Duration::from_millis(100));
        let count = target.len().min(4096);
        self.0.read(&mut target[..count])
    }
}
#[tokio::test]
async fn healthy_source_progress_outlives_the_total_idle_duration_and_lost_replies_reconcile() {
    let f = Fixture::new().await;
    let client = capable(&f).await;
    f.login(&client).await;
    let bytes = vec![7_u8; 16 * 4096];
    let upload = upload(&bytes);
    let started = std::time::Instant::now();
    client
        .upload_attachment_inner(
            upload.clone(),
            SlowRead(io::Cursor::new(bytes.clone())),
            CancellationToken::new(),
            Duration::from_millis(500),
        )
        .await
        .unwrap();
    assert!(started.elapsed() > Duration::from_secs(1));
    f.state.attachment_mode.store(54, Ordering::SeqCst);
    let mut lost = upload;
    lost.request_id = uuid::Uuid::new_v4().to_string();
    let error = client
        .upload_attachment_inner(
            lost.clone(),
            io::Cursor::new(bytes),
            CancellationToken::new(),
            Duration::from_millis(200),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
    assert!(matches!(
        state(&client, &lost).await.unwrap(),
        AttachmentUploadStatus::Committed(_)
    ));
    assert_eq!(f.state.upload_attempts.load(Ordering::SeqCst), 2);
}

struct BlockingRead {
    started: Arc<tokio::sync::Notify>,
    release: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    dropped: Arc<AtomicBool>,
}
impl Read for BlockingRead {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        self.started.notify_one();
        let (lock, condition) = &*self.release;
        let mut ready = lock.lock().unwrap();
        while !*ready {
            ready = condition.wait(ready).unwrap();
        }
        Ok(0)
    }
}
impl Drop for BlockingRead {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}
#[tokio::test]
async fn cancellation_and_abandoned_futures_retain_admission_through_actual_source_reads() {
    for abandon in [false, true] {
        let f = Fixture::new().await;
        let client = capable(&f).await;
        f.login(&client).await;
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let dropped = Arc::new(AtomicBool::new(false));
        let source = BlockingRead {
            started: started.clone(),
            release: release.clone(),
            dropped: dropped.clone(),
        };
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let connection = client.clone();
        let task = tokio::spawn(async move {
            connection
                .upload_attachment(upload(b""), source, token)
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), started.notified())
            .await
            .unwrap();
        if abandon {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            cancel.cancel();
            let error = tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::Cancelled);
            assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
        }
        assert!(!dropped.load(Ordering::SeqCst));
        assert_eq!(client.upload_writers.available_permits(), 1);
        let (lock, condition) = &*release;
        *lock.lock().unwrap() = true;
        condition.notify_all();
        tokio::time::timeout(Duration::from_secs(3), async {
            while client.upload_writers.available_permits() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(dropped.load(Ordering::SeqCst));
        assert!(f.state.attachment_uploads.lock().await.is_empty());
    }
}

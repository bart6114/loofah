use super::*;
use crate::uploads::{UploadPart, produce};
use std::io::Read;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

struct OwnedSource<R> {
    source: R,
    guard: Arc<authority::SourcePin>,
}
impl<R: Read> Read for OwnedSource<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.guard.check_current().map_err(std::io::Error::other)?;
        self.source.read(buffer)
    }
}
impl MediaAuthority {
    pub async fn upload_source(
        &self,
        submission: &MediaSubmission,
        admission: &MediaAdmission,
        source: impl Read + Send + 'static,
    ) -> Result<MediaRequestStatus> {
        let request = &submission.declaration.request_id;
        let prepared=async {
            self.validate_submission(submission)?;
            validate_status(&admission.status)?;
            if submission.admission_revision.as_deref()!=Some(admission.revision.as_str()) || admission.declaration!=submission.declaration || admission.status.declaration!=submission.declaration || admission.revision!=admission.status.revision || admission.status.phase!=MediaPhase::Receiving {
                return Err(failure(ErrorCode::Conflict,"Original media request is not awaiting source; inspect or explicitly recover it"));
            }
            let (filename,descriptor)=submission.declaration.operation.upload().ok_or_else(||invalid("This media request has no uploaded source"))?;
            let input=UploadAttachmentRequest{filename:filename.into(),size_bytes:descriptor.size_bytes,sha256:descriptor.sha256.clone()};
            self.check_current().await?;
            let permit=self.client.upload_writers.clone().try_acquire_owned().map_err(|_|failure(ErrorCode::Unavailable,"Media source uploads are busy"))?;
            let guard=self.guard().await?;
            let url=self.client.endpoint(&["vaults",&self.scope().vault_id,"media","requests",request,"source"])?;
            Ok::<_,ApplicationError>((input,Arc::new(permit),guard,url))
        }.await.map_err(|e|outcome(e,request,true,false))?;
        let (input, permit, guard, url) = prepared;
        let cancel = self.cancel.child_token();
        let _cancel_on_drop = cancel.clone().drop_guard();
        let (send, receive) = mpsc::channel(2);
        let (progress, mut progressed) = watch::channel(Instant::now());
        let body = reqwest::Body::wrap_stream(futures_util::stream::unfold(
            (receive, progress, false),
            |(mut receive, progress, ended)| async move {
                if ended {
                    return None;
                }
                let (part, ended) = match receive.recv().await {
                    Some(UploadPart::Bytes(bytes)) => (Ok(bytes), false),
                    Some(UploadPart::Complete) => return None,
                    Some(UploadPart::Failed) | None => (
                        Err(std::io::Error::other(
                            "Original media source did not complete",
                        )),
                        true,
                    ),
                };
                progress.send_replace(Instant::now());
                Some((part, (receive, progress, ended)))
            },
        ));
        let producer_cancel = cancel.clone();
        let stopped = self.client.stop.clone();
        let source_pin = self.pin_source();
        let producer_guard = source_pin.clone();
        let producer_permit = permit.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = producer_permit;
            produce(
                OwnedSource {
                    source,
                    guard: producer_guard,
                },
                input,
                send,
                producer_cancel,
                stopped,
            );
        });
        // No Content-Length: failed or changed source cannot look like a completed upload.
        let send = self
            .client
            .http
            .put(url)
            .bearer_auth(&guard.access)
            .header("x-loofah-api-major", API_MAJOR.to_string())
            .header("x-request-id", request)
            .header("x-loofah-media-revision", &admission.revision)
            .header("content-type", "application/octet-stream")
            .body(body)
            .send();
        let client = self.client.clone();
        let lifetime = self.lifetime.clone();
        let request_id = request.to_owned();
        let declaration = submission.declaration.clone();
        let revision = admission.revision.clone();
        let transfer = tokio::spawn(async move {
            let _permit = permit;
            let _source_pin = source_pin;
            tokio::pin!(send);
            let idle = Duration::from_secs(35);
            let work = async {
                let mut progress_open = true;
                let response = loop {
                    let deadline = *progressed.borrow_and_update() + idle;
                    tokio::select! {
                        response = &mut send => break response.map_err(|_|crate::unavailable())?,
                        _ = tokio::time::sleep_until(deadline) => return Err(crate::unavailable()),
                        changed = progressed.changed(), if progress_open => { progress_open = changed.is_ok(); },
                    }
                };
                if !response.status().is_success() {
                    return Err(tokio::time::timeout(
                        idle,
                        json_body::<ApplicationError>(response, 131_072),
                    )
                    .await
                    .unwrap_or_else(|_| Err(crate::unavailable()))
                    .unwrap_or_else(|_| crate::unavailable()));
                }
                let status: MediaRequestStatus =
                    tokio::time::timeout(idle, json_body(response, SNAPSHOT_LIMIT))
                        .await
                        .map_err(|_| crate::unavailable())??;
                validate_status(&status)?;
                if status.declaration != declaration || status.revision != revision {
                    return Err(bad_reply(
                        "Media upload receipt changed its original binding",
                    ));
                }
                Ok(status)
            };
            tokio::select! {
                _ = cancel.cancelled() => Err(crate::cancelled()),
                _ = lifetime.cancelled() => Err(crate::cancelled()),
                _ = client.stop.cancelled() => Err(crate::cancelled()),
                result = work => result,
            }
            .map_err(|e| outcome(e, &request_id, true, true))
        });
        // The owned transfer is admitted with its immutable bearer and source before
        // releasing account locks. Its physical reader keeps the original lease pinned.
        drop(guard);
        transfer
            .await
            .map_err(|_| outcome(crate::unavailable(), request, true, true))?
    }
}

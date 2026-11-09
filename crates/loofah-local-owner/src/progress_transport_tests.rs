use super::*;
use hypr_loofah_protocol::{ApplicationError, ErrorCode, LocalCliProgress};
use std::{
    future::Future,
    sync::{Mutex, atomic::AtomicBool},
};

struct ProgressHandler {
    fail: bool,
    retained: Mutex<Option<ProgressReporter>>,
    release: Option<Arc<Semaphore>>,
    finished: Arc<AtomicBool>,
}

fn event(index: usize, phase: &str) -> LocalCliProgress {
    LocalCliProgress {
        schema_version: "2".into(),
        phase: phase.into(),
        processed: index,
        total: Some(100),
        added: 0,
        updated: 0,
        deleted: 0,
        elapsed_ms: 0,
    }
}

impl OwnerHandler for ProgressHandler {
    fn operations(&self) -> Vec<LocalCliOperationKind> {
        vec![LocalCliOperationKind::Note]
    }

    fn execute(&self, request: OwnerRequest) -> OwnerFuture {
        // Fill before returning an immediately-ready future: delivery cannot
        // accidentally hide overflow by keeping pace with this producer.
        for index in 0..64 {
            request.progress.report(event(index, "index"));
        }
        request
            .progress
            .report(event(100, if self.fail { "failed" } else { "ready" }));
        *self.retained.lock().unwrap() = Some(request.progress.clone());
        let fail = self.fail;
        let release = self.release.clone();
        let finished = self.finished.clone();
        Box::pin(async move {
            if let Some(release) = release {
                release.acquire().await.unwrap().forget();
            }
            finished.store(true, Ordering::SeqCst);
            OwnerResponse {
                response: LocalCliResponse {
                    result: if fail {
                        Err(ApplicationError {
                            code: ErrorCode::Conflict,
                            message: "fixture refused".into(),
                            request_id: None,
                            retryable: false,
                            current_revision: None,
                            mutation_outcome: None,
                        })
                    } else {
                        Ok(LocalCliReply::Note {
                            snapshot: NoteSnapshot {
                                markdown: Some("original".into()),
                                title: "title".into(),
                                revision: "r".into(),
                            },
                        })
                    },
                },
                blob: None,
            }
        })
    }
}

async fn completed_progress(fail: bool) {
    let fixture = Fixture::new();
    let handler = Arc::new(ProgressHandler {
        fail,
        retained: Mutex::new(None),
        release: None,
        finished: Arc::new(AtomicBool::new(false)),
    });
    let server = OwnerServer::bind(
        fixture.lease(),
        &fixture.endpoints,
        &fixture.temporary,
        handler.clone(),
    )
    .await
    .unwrap();
    let mut events = Vec::new();
    let output = fixture
        .client()
        .await
        .execute_with_progress(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::Note {
                session_id: "note".into(),
            },
            None,
            |progress| events.push(progress),
        )
        .await
        .unwrap();
    assert_eq!(output.response.result.is_err(), fail);
    assert_eq!(events.len(), 16);
    assert_eq!(
        events
            .iter()
            .map(|value| value.processed)
            .collect::<Vec<_>>(),
        (0..15).chain([100]).collect::<Vec<_>>()
    );
    assert_eq!(
        events.last().unwrap().phase,
        if fail { "failed" } else { "ready" }
    );
    assert!(handler.finished.load(Ordering::SeqCst));
    handler
        .retained
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .report(event(101, "late"));
    tokio::time::timeout(Duration::from_secs(5), server.shutdown())
        .await
        .unwrap()
        .unwrap();
    drop(fixture.lease());
}

#[tokio::test]
async fn ready_response_follows_full_progress_queue_and_terminal_event_with_retained_clone() {
    completed_progress(false).await;
}

#[tokio::test]
async fn error_response_follows_full_progress_queue_and_exact_final_event() {
    completed_progress(true).await;
}

#[tokio::test]
async fn dropped_progress_delivery_still_drains_actual_operation_without_waiting_for_reporter_clone()
 {
    let fixture = Fixture::new();
    let release = Arc::new(Semaphore::new(0));
    let handler = Arc::new(ProgressHandler {
        fail: false,
        retained: Mutex::new(None),
        release: Some(release.clone()),
        finished: Arc::new(AtomicBool::new(false)),
    });
    let lease = fixture.lease();
    let original = Arc::downgrade(&lease.pin());
    let server = OwnerServer::bind(
        lease,
        &fixture.endpoints,
        &fixture.temporary,
        handler.clone(),
    )
    .await
    .unwrap();
    let client = fixture.client().await;
    let (seen, received) = tokio::sync::oneshot::channel();
    let mut seen = Some(seen);
    let request = tokio::spawn(async move {
        client
            .execute_with_progress(
                uuid::Uuid::new_v4().to_string(),
                LocalCliOperation::Note {
                    session_id: "note".into(),
                },
                None,
                move |_| {
                    if let Some(seen) = seen.take() {
                        let _ = seen.send(());
                    }
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), received)
        .await
        .unwrap()
        .unwrap();
    request.abort();
    let _ = request.await;
    server.request_shutdown();
    let mut shutdown = Box::pin(server.wait_for_shutdown());
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(shutdown.as_mut().poll(cx).is_pending()))
            .await
    );
    assert!(!handler.finished.load(Ordering::SeqCst));
    assert!(original.upgrade().is_some());
    assert!(matches!(
        VaultLease::try_acquire_existing(&fixture.vault).unwrap(),
        LeaseAttempt::Busy
    ));
    release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert!(handler.finished.load(Ordering::SeqCst));
    handler
        .retained
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .report(event(102, "late"));
    assert!(original.upgrade().is_none());
    drop(fixture.lease());
}

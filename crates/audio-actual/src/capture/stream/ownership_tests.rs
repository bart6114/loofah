use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Probe {
    dropped: Arc<AtomicUsize>,
    done: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Drop for Probe {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
        let _ = self.done.take().unwrap().send(());
    }
}
fn owned_health() -> (
    Arc<CaptureHealth>,
    std::sync::Weak<Probe>,
    Arc<AtomicUsize>,
    tokio::sync::oneshot::Receiver<()>,
) {
    let dropped = Arc::new(AtomicUsize::new(0));
    let (done, completion) = tokio::sync::oneshot::channel();
    let owner = Arc::new(Probe {
        dropped: dropped.clone(),
        done: Some(done),
    });
    let weak = Arc::downgrade(&owner);
    let health = CaptureHealth {
        owner: Some(owner),
        ..Default::default()
    };
    (Arc::new(health), weak, dropped, completion)
}
async fn retired(owner: &std::sync::Weak<Probe>, completion: tokio::sync::oneshot::Receiver<()>) {
    // A zero strong count can precede the destructor's completion on another worker.
    tokio::time::timeout(std::time::Duration::from_secs(3), completion)
        .await
        .unwrap()
        .unwrap();
    assert!(owner.upgrade().is_none());
}

#[tokio::test]
async fn original_owner_retires_after_actual_native_drain_even_if_control_survives() {
    let (health, owner, dropped, completion) = owned_health();
    let (input, chunks, _) = queued_chunks(1);
    input.send(Ok(vec![0.25; 160])).await.unwrap();
    let mut stream = open_single(chunks, CaptureSide::Mic, Some(health));
    let control = stream.drain_control().unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    stream.request_stop().unwrap();
    while stream.next().await.is_some() {}
    assert_eq!(control.completion().await.unwrap().frames, 160);
    retired(&owner, completion).await;
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(control.completion().await.is_ok());
}

#[tokio::test]
async fn dropped_observer_cannot_release_original_owner_while_native_factory_is_blocked() {
    let (health, owner, dropped, completion) = owned_health();
    let (_changes, refresh) = tokio::sync::watch::channel(0);
    let (entered, started) = tokio::sync::oneshot::channel();
    let entered = Mutex::new(Some(entered));
    let (release, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    let stream = super::super::recovery::recovering_chunks(
        boxed_chunks(futures_util::stream::empty()),
        Arc::new(move || {
            entered.lock().unwrap().take().unwrap().send(()).unwrap();
            wait.lock().unwrap().recv().unwrap();
            Err(Error::MicOpenFailed)
        }),
        refresh,
        health,
        1,
        16_000,
        160,
    );
    tokio::time::timeout(std::time::Duration::from_secs(3), started)
        .await
        .unwrap()
        .unwrap();
    drop(stream);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert!(owner.upgrade().is_some());
    release.send(()).unwrap();
    retired(&owner, completion).await;
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn native_panic_releases_owner_only_after_the_actual_worker_unwinds() {
    let (health, owner, dropped, completion) = owned_health();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let (done, receipt) = tokio::sync::watch::channel(None);
    let task = spawn_native(
        async move {
            entered.send(()).unwrap();
            wait.await.unwrap();
            panic!("controlled native worker failure")
        },
        done,
        health,
    );
    started.await.unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    task.await.unwrap();
    retired(&owner, completion).await;
    assert!(receipt.borrow().as_ref().unwrap().is_err());
    assert!(owner.upgrade().is_none());
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

use super::*;
use std::{future::Future, pin::Pin, task::Poll, time::Duration};

async fn queued<F: Future>(future: &mut Pin<Box<F>>) {
    assert!(std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await);
}

#[tokio::test]
async fn reserved_live_capacity_is_explicit_and_never_expands_the_total() {
    let legacy = SpeechAdmission::new(1, 0, 2).unwrap();
    let batch = legacy.batch(&CancellationToken::new()).await.unwrap();
    assert!(legacy.is_busy());
    assert!(matches!(legacy.try_live(), Err(AdmissionError::Busy)));
    drop(batch);
    drop(legacy.try_live().unwrap());

    let partitioned = SpeechAdmission::new(2, 1, 2).unwrap();
    let batch = partitioned.batch(&CancellationToken::new()).await.unwrap();
    assert!(!partitioned.is_busy());
    let live = partitioned.try_live().unwrap();
    assert!(partitioned.is_busy());
    assert_eq!(partitioned.slots.available_permits(), 0);
    assert!(matches!(partitioned.try_live(), Err(AdmissionError::Busy)));
    let cancel = CancellationToken::new();
    let mut waiting = Box::pin(partitioned.batch(&cancel));
    queued(&mut waiting).await;
    drop(live);
    assert!(!partitioned.is_busy());
    queued(&mut waiting).await;
    assert_eq!(partitioned.slots.available_permits(), 1);
    drop(batch);
    drop(waiting.await.unwrap());
    assert_eq!(partitioned.slots.available_permits(), 2);
}

#[tokio::test]
async fn global_batch_queue_is_bounded_fifo_and_cancelled_waiters_never_get_capacity() {
    let admission = SpeechAdmission::new(2, 1, 2).unwrap();
    let running = admission.batch(&CancellationToken::new()).await.unwrap();
    let cancel_first = CancellationToken::new();
    let cancel_second = CancellationToken::new();
    let mut first_vault = Box::pin(admission.batch(&cancel_first));
    queued(&mut first_vault).await;
    let mut second_vault = Box::pin(admission.batch(&cancel_second));
    queued(&mut second_vault).await;
    assert!(matches!(
        admission.batch(&CancellationToken::new()).await,
        Err(AdmissionError::Busy)
    ));
    cancel_first.cancel();
    assert!(matches!(first_vault.await, Err(AdmissionError::Cancelled)));
    let cancel_later = CancellationToken::new();
    let mut later_vault = Box::pin(admission.batch(&cancel_later));
    queued(&mut later_vault).await;
    drop(running);
    queued(&mut later_vault).await;
    let second = second_vault.await.unwrap();
    queued(&mut later_vault).await;
    drop(second);
    drop(later_vault.await.unwrap());
    assert_eq!(admission.slots.available_permits(), 2);
    assert_eq!(admission.waiting.available_permits(), 2);
}

#[tokio::test]
async fn abandoned_waiter_keeps_physical_owner_and_shutdown_waits_for_both_lanes() {
    let admission = SpeechAdmission::new(2, 1, 2).unwrap();
    let batch = admission.batch(&CancellationToken::new()).await.unwrap();
    let live = admission.try_live().unwrap();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let owner = tokio::spawn(async move {
        tokio::task::spawn_blocking(move || {
            let _physical = batch;
            entered.send(()).unwrap();
            blocked.recv().unwrap();
        })
        .await
        .unwrap();
    });
    started.await.unwrap();
    owner.abort();
    assert!(owner.await.unwrap_err().is_cancelled());
    let cancel = CancellationToken::new();
    let mut waiting = Box::pin(admission.batch(&cancel));
    queued(&mut waiting).await;
    admission.stop();
    assert!(matches!(waiting.await, Err(AdmissionError::Stopped)));
    assert!(matches!(admission.try_live(), Err(AdmissionError::Stopped)));
    let mut drain = Box::pin(admission.drain());
    queued(&mut drain).await;
    drop(live);
    queued(&mut drain).await;
    release.send(()).unwrap();
    drop(
        tokio::time::timeout(Duration::from_secs(2), drain)
            .await
            .unwrap(),
    );
}

#[tokio::test]
async fn physical_worker_panic_closes_admission_before_releasing_its_slot() {
    let admission = SpeechAdmission::new(2, 1, 2).unwrap();
    let batch = admission.batch(&CancellationToken::new()).await.unwrap();
    let task = tokio::task::spawn_blocking(move || {
        let _physical = batch;
        panic!("injected native worker loss");
    });
    assert!(task.await.unwrap_err().is_panic());
    assert!(admission.is_stopped());
    assert!(matches!(admission.try_live(), Err(AdmissionError::Stopped)));
    assert!(matches!(
        admission.batch(&CancellationToken::new()).await,
        Err(AdmissionError::Stopped)
    ));
    drop(
        tokio::time::timeout(Duration::from_secs(2), admission.drain())
            .await
            .unwrap(),
    );
}

#[tokio::test]
async fn startup_and_shutdown_own_every_configured_group() {
    let admission = SpeechAdmission::new(2, 1, 1).unwrap();
    let loading = admission.reserve_loading();
    assert!(matches!(admission.try_live(), Err(AdmissionError::Busy)));
    let cancel = CancellationToken::new();
    let mut waiting = Box::pin(admission.batch(&cancel));
    queued(&mut waiting).await;
    admission.stop();
    assert!(matches!(waiting.await, Err(AdmissionError::Stopped)));
    let mut drain = Box::pin(admission.drain());
    queued(&mut drain).await;
    drop(loading);
    drop(
        tokio::time::timeout(Duration::from_secs(2), drain)
            .await
            .unwrap(),
    );
}

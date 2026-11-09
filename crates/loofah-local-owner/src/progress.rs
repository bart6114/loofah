use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use hypr_loofah_protocol::LocalCliProgress;
use tokio::sync::Notify;

const CAPACITY: usize = 16;

#[derive(Default)]
struct Pending {
    events: VecDeque<LocalCliProgress>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    pending: Mutex<Pending>,
    changed: Notify,
}

#[derive(Clone)]
pub struct ProgressReporter(Arc<Shared>);

impl ProgressReporter {
    pub fn report(&self, progress: LocalCliProgress) {
        if progress.phase.len() > 256 || progress.schema_version.len() > 64 {
            return;
        }
        {
            let mut pending = self.0.pending.lock().unwrap();
            if pending.closed {
                return;
            }
            // Keep early milestones and the latest state without blocking the producer.
            if pending.events.len() == CAPACITY {
                pending.events.pop_back();
            }
            pending.events.push_back(progress);
        }
        self.0.changed.notify_one();
    }
}

pub(crate) struct Receiver(Arc<Shared>);
pub(crate) struct Completion(Arc<Shared>);

pub(crate) fn channel() -> (ProgressReporter, Receiver, Completion) {
    let shared = Arc::new(Shared::default());
    (
        ProgressReporter(shared.clone()),
        Receiver(shared.clone()),
        Completion(shared),
    )
}

impl Receiver {
    pub(crate) async fn recv(&mut self) -> Option<LocalCliProgress> {
        loop {
            let changed = self.0.changed.notified();
            {
                let mut pending = self.0.pending.lock().unwrap();
                if let Some(event) = pending.events.pop_front() {
                    return Some(event);
                }
                if pending.closed {
                    return None;
                }
            }
            changed.await;
        }
    }

    pub(crate) fn finish(&mut self) -> VecDeque<LocalCliProgress> {
        let mut pending = self.0.pending.lock().unwrap();
        pending.closed = true;
        std::mem::take(&mut pending.events)
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.finish();
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        self.0.pending.lock().unwrap().closed = true;
        self.0.changed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;

    fn event(processed: usize) -> LocalCliProgress {
        LocalCliProgress {
            schema_version: "2".into(),
            phase: format!("phase-{processed}"),
            processed,
            total: Some(100),
            added: 0,
            updated: 0,
            deleted: 0,
            elapsed_ms: 0,
        }
    }

    #[tokio::test]
    async fn completed_operation_drains_full_queue_with_exact_latest_and_rejects_late_clone() {
        let (reporter, mut receiver, completion) = channel();
        let retained = reporter.clone();
        for index in 0..100 {
            reporter.report(event(index));
        }
        let operation = async move {
            drop(completion);
            42
        };
        // Select completion with all progress still pending, deterministically.
        assert_eq!(operation.await, 42);
        retained.report(event(100));
        let events = receiver.finish();
        assert_eq!(events.len(), CAPACITY);
        assert_eq!(
            events
                .iter()
                .map(|value| value.processed)
                .collect::<Vec<_>>(),
            (0..CAPACITY - 1).chain([99]).collect::<Vec<_>>()
        );
        assert!(receiver.recv().await.is_none());
        assert!(receiver.finish().is_empty());
    }

    #[tokio::test]
    async fn progress_wakes_registered_receiver_and_completion_closes_without_reporter_drop() {
        let (reporter, mut receiver, completion) = channel();
        let mut next = Box::pin(receiver.recv());
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(next.as_mut().poll(cx).is_pending()))
                .await
        );
        reporter.report(event(3));
        assert_eq!(next.await.unwrap().processed, 3);
        drop(completion);
        assert!(receiver.recv().await.is_none());
        reporter.report(event(4));
        assert!(receiver.finish().is_empty());
    }

    #[test]
    fn dropped_delivery_discards_bounded_progress_without_blocking_retained_producer() {
        let (reporter, receiver, completion) = channel();
        reporter.report(event(0));
        drop(receiver);
        for index in 1..100 {
            reporter.report(event(index));
        }
        let pending = reporter.0.pending.lock().unwrap();
        assert!(pending.closed && pending.events.is_empty());
        drop(pending);
        drop(completion);
    }

    #[test]
    fn invalid_event_cannot_replace_latest_valid_progress() {
        let (reporter, mut receiver, _completion) = channel();
        reporter.report(event(0));
        let mut invalid = event(1);
        invalid.phase = "x".repeat(257);
        reporter.report(invalid);
        let mut invalid = event(2);
        invalid.schema_version = "x".repeat(65);
        reporter.report(invalid);
        let events = receiver.finish();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].processed, 0);
    }
}

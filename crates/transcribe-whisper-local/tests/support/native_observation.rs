use std::{
    fmt::{self, Write},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio::sync::{Mutex, Notify};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

pub static MODEL_TEST_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Default)]
pub struct NativeEvents {
    starts: AtomicU64,
    completions: AtomicU64,
    changed: Notify,
}

impl NativeEvents {
    // Absence of the completion event is an unfinished-attempt witness, not
    // proof that a thread is currently inside the kernel or has drained.
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.starts.load(Ordering::Acquire),
            self.completions.load(Ordering::Acquire),
        )
    }

    pub async fn started_after(&self, previous: u64) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            // Register before checking the counter: notify_waiters does not retain
            // a permit when a Native event lands between the check and suspension.
            changed.as_mut().enable();
            if self.starts.load(Ordering::Acquire) > previous {
                return;
            }
            changed.await;
        }
    }
}

pub fn install() -> Arc<NativeEvents> {
    static EVENTS: OnceLock<Arc<NativeEvents>> = OnceLock::new();
    EVENTS
        .get_or_init(|| {
            let events = Arc::new(NativeEvents::default());
            tracing_subscriber::registry()
                .with(tracing_subscriber::EnvFilter::new(
                    "transcribe_whisper_local=debug,whisper_local=debug,model_manager=debug",
                ))
                .with(tracing_subscriber::fmt::layer().with_test_writer())
                .with(Observe(events.clone()))
                .try_init()
                .expect("real model tests require the Native event observer subscriber");
            events
        })
        .clone()
}

struct Observe(Arc<NativeEvents>);

impl<S: Subscriber> Layer<S> for Observe {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        if event.metadata().target() != "whisper_local::model::actual" {
            return;
        }
        let mut message = MessageVisitor::default();
        event.record(&mut message);
        let counter = match message.0 {
            Some(Message::Started) => &self.0.starts,
            Some(Message::Completed) => &self.0.completions,
            None => return,
        };
        if counter
            .fetch_update(Ordering::Release, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .is_ok()
            && matches!(message.0, Some(Message::Started))
        {
            self.0.changed.notify_waiters();
        }
    }
}

#[derive(Clone, Copy)]
enum Message {
    Started,
    Completed,
}

#[derive(Default)]
struct MessageVisitor(Option<Message>);

impl MessageVisitor {
    fn classify(&mut self, text: &str) {
        self.0 = match text {
            "transcribe_started" => Some(Message::Started),
            "whisper_inference_completed" => Some(Message::Completed),
            _ => None,
        };
    }
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.classify(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() != "message" {
            return;
        }
        let mut message = ShortMessage {
            bytes: [0; 64],
            length: 0,
        };
        if write!(&mut message, "{value:?}").is_ok()
            && let Ok(text) = std::str::from_utf8(&message.bytes[..message.length])
        {
            self.classify(text);
        }
    }
}

struct ShortMessage {
    bytes: [u8; 64],
    length: usize,
}

impl Write for ShortMessage {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if value.len() > self.bytes.len() - self.length {
            return Err(fmt::Error);
        }
        self.bytes[self.length..self.length + value.len()].copy_from_slice(value.as_bytes());
        self.length += value.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{task::Poll, time::Duration};

    fn emit(events: &Arc<NativeEvents>, callback: impl FnOnce()) {
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(Observe(events.clone())),
            callback,
        );
    }

    #[test]
    fn native_observer_counts_only_exact_native_messages() {
        let events = Arc::new(NativeEvents::default());
        emit(&events, || {
            tracing::info!(target: "whisper_local::model::actual", samples = 42, "transcribe_started");
            tracing::info!(target: "whisper_local::model::actual", elapsed_ms = 7, "whisper_inference_completed");
            tracing::info!(target: "transcribe_whisper_local::service::live_worker", "transcribe_started");
            tracing::info!(target: "whisper_local::model::actual", "prefix transcribe_started");
            tracing::info!(target: "whisper_local::model::actual", detail = "transcribe_started", "different event");
            tracing::info!(target: "whisper_local::model::actual", "{}", "transcribe_started".repeat(100));
            tracing::info!(target: "whisper_local::model::actual", message = "transcribe_started");
        });
        assert_eq!(events.snapshot(), (2, 1));
    }

    #[tokio::test]
    async fn native_observer_waits_for_new_start_and_notifies_all_registered_waiters() {
        let events = Arc::new(NativeEvents::default());
        emit(
            &events,
            || tracing::info!(target: "whisper_local::model::actual", "transcribe_started"),
        );
        tokio::time::timeout(Duration::from_secs(1), events.started_after(0))
            .await
            .unwrap();
        let first = events.started_after(1);
        let second = events.started_after(1);
        tokio::pin!(first, second);
        assert!(matches!(futures_util::poll!(&mut first), Poll::Pending));
        assert!(matches!(futures_util::poll!(&mut second), Poll::Pending));
        emit(
            &events,
            || tracing::info!(target: "whisper_local::model::actual", "whisper_inference_completed"),
        );
        assert!(matches!(futures_util::poll!(&mut first), Poll::Pending));
        assert!(matches!(futures_util::poll!(&mut second), Poll::Pending));
        emit(
            &events,
            || tracing::info!(target: "whisper_local::model::actual", "transcribe_started"),
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(first, second);
        })
        .await
        .unwrap();
        assert_eq!(events.snapshot(), (2, 1));
    }

    #[test]
    fn native_observer_counters_never_wrap() {
        let events = Arc::new(NativeEvents::default());
        events.starts.store(u64::MAX, Ordering::Relaxed);
        events.completions.store(u64::MAX, Ordering::Relaxed);
        emit(&events, || {
            tracing::info!(target: "whisper_local::model::actual", "transcribe_started");
            tracing::info!(target: "whisper_local::model::actual", "whisper_inference_completed");
        });
        assert_eq!(events.snapshot(), (u64::MAX, u64::MAX));
    }
}

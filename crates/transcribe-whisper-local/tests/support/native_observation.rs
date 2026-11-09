use std::{
    fmt::{self, Write},
    sync::{
        Arc, Mutex as StdMutex, OnceLock,
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
    live: StdMutex<LiveState>,
    live_changed: Notify,
}

const MAX_LIVE_SESSIONS: usize = 32;

#[derive(Default)]
struct LiveState {
    epoch: u64,
    sessions: Vec<LiveSession>,
    invalid: bool,
}

struct LiveSession {
    id: String,
    epoch: u64,
    drained: bool,
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

    pub fn live_epoch(&self) -> u64 {
        let state = self.live.lock().expect("live observer lock poisoned");
        let (epoch, invalid) = (state.epoch, state.invalid);
        drop(state);
        assert!(
            !invalid,
            "live observer exceeded its bounded identity inventory"
        );
        epoch
    }

    pub async fn live_started_after(&self, previous: u64) -> String {
        loop {
            let changed = self.live_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let (next, invalid) = {
                let state = self.live.lock().expect("live observer lock poisoned");
                (
                    state
                        .sessions
                        .iter()
                        .find(|session| session.epoch > previous)
                        .map(|session| session.id.clone()),
                    state.invalid,
                )
            };
            assert!(
                !invalid,
                "live observer exceeded its bounded identity inventory"
            );
            if let Some(next) = next {
                return next;
            }
            changed.await;
        }
    }

    pub async fn live_drained(&self, request: &str) {
        assert!(valid_id(request), "invalid live worker identity");
        loop {
            let changed = self.live_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let (drained, invalid) = {
                let state = self.live.lock().expect("live observer lock poisoned");
                (
                    state
                        .sessions
                        .iter()
                        .find(|session| session.id == request)
                        .map(|session| session.drained),
                    state.invalid,
                )
            };
            assert!(
                !invalid,
                "live observer exceeded its bounded identity inventory"
            );
            assert!(drained.is_some(), "unobserved live worker identity");
            if drained == Some(true) {
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
        if event.metadata().target() == "transcribe_whisper_local::service::streaming" {
            self.live_event(event);
            return;
        }
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

impl Observe {
    fn live_event(&self, event: &Event<'_>) {
        let mut fields = LiveVisitor::default();
        event.record(&mut fields);
        let (Some(message), Some(request)) = (fields.message, fields.request) else {
            return;
        };
        if let Ok(mut state) = self.0.live.lock() {
            match message {
                LiveMessage::Started => {
                    if state.sessions.len() >= MAX_LIVE_SESSIONS
                        || state.sessions.iter().any(|session| session.id == request)
                        || state.epoch == u64::MAX
                    {
                        state.invalid = true;
                    } else {
                        state.epoch += 1;
                        let epoch = state.epoch;
                        state.sessions.push(LiveSession {
                            id: request,
                            epoch,
                            drained: false,
                        });
                    }
                }
                LiveMessage::Completion if fields.confirmed == Some(true) => {
                    if let Some(session) = state
                        .sessions
                        .iter_mut()
                        .find(|session| session.id == request)
                    {
                        session.drained = true;
                    }
                }
                LiveMessage::Completion => {}
            }
        }
        self.0.live_changed.notify_waiters();
    }
}

#[derive(Clone, Copy)]
enum LiveMessage {
    Started,
    Completion,
}

#[derive(Default)]
struct LiveVisitor {
    message: Option<LiveMessage>,
    request: Option<String>,
    confirmed: Option<bool>,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

impl Visit for LiveVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => {
                self.message = match value {
                    "whisper_live_worker_started" => Some(LiveMessage::Started),
                    "whisper_live_worker_completion" => Some(LiveMessage::Completion),
                    _ => None,
                }
            }
            "request_id" if valid_id(value) => self.request = Some(value.into()),
            _ => {}
        }
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "confirmed" {
            self.confirmed = Some(value);
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if !matches!(field.name(), "message" | "request_id") {
            return;
        }
        let mut text = ShortMessage {
            bytes: [0; 64],
            length: 0,
        };
        if write!(&mut text, "{value:?}").is_ok()
            && let Ok(value) = std::str::from_utf8(&text.bytes[..text.length])
        {
            self.record_str(field, value);
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

    #[tokio::test]
    async fn live_observer_requires_confirmed_original_worker_completion() {
        let events = Arc::new(NativeEvents::default());
        let started = events.live_started_after(events.live_epoch());
        tokio::pin!(started);
        assert!(matches!(futures_util::poll!(&mut started), Poll::Pending));
        emit(&events, || {
            tracing::debug!(target: "transcribe_whisper_local::service::streaming",
            request_id = %"original", "whisper_live_worker_started")
        });
        let original = tokio::time::timeout(Duration::from_secs(1), started)
            .await
            .unwrap();
        assert_eq!(original, "original");
        let drained = events.live_drained(&original);
        tokio::pin!(drained);
        assert!(matches!(futures_util::poll!(&mut drained), Poll::Pending));
        let epoch = events.live_epoch();
        emit(&events, || {
            tracing::debug!(target: "transcribe_whisper_local::service::streaming",
                request_id = "replacement", "whisper_live_worker_started");
            tracing::debug!(target: "transcribe_whisper_local::service::streaming",
                request_id = "replacement", confirmed = true, "whisper_live_worker_completion");
            tracing::debug!(target: "transcribe_whisper_local::service::streaming",
                request_id = "unknown", confirmed = true, "whisper_live_worker_completion");
            tracing::debug!(target: "transcribe_whisper_local::service::streaming",
                request_id = "original", confirmed = false, "whisper_live_worker_completion");
            tracing::debug!(target: "transcribe_whisper_local::service::streaming",
                request_id = "original", confirmed = "true", "whisper_live_worker_completion");
            tracing::info!(target: "whisper_local::model::actual", "whisper_inference_completed");
        });
        assert_eq!(events.live_started_after(epoch).await, "replacement");
        assert!(matches!(futures_util::poll!(&mut drained), Poll::Pending));
        emit(&events, || {
            tracing::debug!(target: "transcribe_whisper_local::service::streaming",
            request_id = %"original", confirmed = true, "whisper_live_worker_completion")
        });
        tokio::time::timeout(Duration::from_secs(1), drained)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), events.live_drained(&original))
            .await
            .unwrap();
    }

    #[test]
    fn live_observer_ignores_malformed_unknown_and_wrong_target_identities() {
        let events = Arc::new(NativeEvents::default());
        emit(&events, || {
            for request in ["".to_string(), "x".repeat(65), "bad/id".into(), "ü".into()] {
                tracing::debug!(target: "transcribe_whisper_local::service::streaming",
                    request_id = %request, "whisper_live_worker_started");
            }
            tracing::debug!(target: "transcribe_whisper_local::service::streaming", "whisper_live_worker_started");
            tracing::debug!(target: "transcribe_whisper_local::service::live_worker",
                request_id = "wrong-target", "whisper_live_worker_started");
            tracing::debug!(target: "transcribe_whisper_local::service::streaming",
                request_id = "wrong-message", "prefix whisper_live_worker_started");
            tracing::debug!(target: "transcribe_whisper_local::service::streaming",
                request_id = "unknown", confirmed = true, "whisper_live_worker_completion");
        });
        assert_eq!(events.live_epoch(), 0);
        assert!(events.live.lock().unwrap().sessions.is_empty());
        emit(&events, || {
            tracing::debug!(target: "transcribe_whisper_local::service::streaming",
            request_id = %"x".repeat(64), "whisper_live_worker_started")
        });
        assert_eq!(events.live_epoch(), 1);
    }

    #[test]
    fn live_observer_inventory_never_evicts_original_identity_or_accepts_reuse() {
        let events = Arc::new(NativeEvents::default());
        emit(&events, || {
            for index in 0..MAX_LIVE_SESSIONS {
                tracing::debug!(target: "transcribe_whisper_local::service::streaming",
                    request_id = %format!("request-{index}"), "whisper_live_worker_started");
            }
        });
        assert_eq!(events.live_epoch(), MAX_LIVE_SESSIONS as u64);
        emit(&events, || {
            tracing::debug!(target: "transcribe_whisper_local::service::streaming",
            request_id = "overflow", "whisper_live_worker_started")
        });
        {
            let state = events.live.lock().unwrap();
            assert_eq!(state.sessions.len(), MAX_LIVE_SESSIONS);
            assert_eq!(state.sessions[0].id, "request-0");
            assert!(state.invalid);
        }
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| events.live_epoch())).is_err()
        );
        let reused = Arc::new(NativeEvents::default());
        emit(&reused, || {
            for _ in 0..2 {
                tracing::debug!(target: "transcribe_whisper_local::service::streaming",
                    request_id = "same", "whisper_live_worker_started");
            }
        });
        assert_eq!(reused.live.lock().unwrap().sessions.len(), 1);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reused.live_epoch())).is_err()
        );
    }
}

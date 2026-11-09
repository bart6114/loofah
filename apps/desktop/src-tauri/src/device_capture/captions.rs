use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use hypr_loofah_client::{
    CaptureLiveBinding, CaptureLiveEvent, ConnectionState, RemoteClient, RemoteOperation,
    RemoteReply, Subscription,
};
use hypr_loofah_protocol::{
    ApplicationError, CaptureChannel, CaptureIdentity, CaptureLiveCaptionPage, CaptureTarget,
    ErrorCode,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::DeviceProcessingProgress;

const MAX_WORDS: usize = 128;
const MAX_PARTIALS: usize = 64;
const MAX_TEXT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, serde::Serialize, specta::Type)]
pub struct DeviceCaptionWord {
    pub id: Option<String>,
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub channel: u8,
    pub role: CaptureChannel,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, specta::Type)]
pub struct DeviceCaptionPreview {
    pub binding: CaptureLiveBinding,
    pub connection: ConnectionState,
    pub error: Option<ApplicationError>,
    pub next_sequence: u64,
    pub words: Vec<DeviceCaptionWord>,
    pub partials: Vec<DeviceCaptionWord>,
}

impl DeviceCaptionPreview {
    fn new(binding: CaptureLiveBinding) -> Self {
        Self {
            binding,
            connection: ConnectionState::Connecting,
            error: None,
            next_sequence: 0,
            words: vec![],
            partials: vec![],
        }
    }

    fn page(&mut self, page: CaptureLiveCaptionPage) -> bool {
        if page.snapshot.generation_id != self.binding.generation_id
            || page.snapshot.capture_id != self.binding.capture_id
            || page.snapshot.capture_fingerprint != self.binding.capture_fingerprint
            || page.snapshot.session_id != self.binding.session_id
        {
            return false;
        }
        for caption in page.captions {
            if caption.sequence < self.next_sequence {
                continue;
            }
            if caption.generation_id != self.binding.generation_id
                || caption.sequence != self.next_sequence
            {
                return false;
            }
            self.next_sequence += 1;
            let mut words = self
                .words
                .drain(..)
                .filter_map(|word| {
                    let id = word.id.clone()?;
                    (!caption.replaced_ids.contains(&id)).then_some((id, word))
                })
                .collect::<std::collections::HashMap<_, _>>();
            for word in caption.new_words {
                words.insert(
                    word.id.clone(),
                    DeviceCaptionWord {
                        id: Some(word.id),
                        text: word.text,
                        start_ms: word.start_ms,
                        end_ms: word.end_ms,
                        channel: caption.physical_channel,
                        role: caption.role,
                    },
                );
            }
            self.words = words.into_values().collect();
            self.partials
                .retain(|word| word.channel != caption.physical_channel);
            self.partials
                .extend(caption.partials.into_iter().map(|word| DeviceCaptionWord {
                    id: None,
                    text: word.text,
                    start_ms: word.start_ms,
                    end_ms: word.end_ms,
                    channel: caption.physical_channel,
                    role: caption.role,
                }));
            self.bound();
        }
        self.connection = ConnectionState::Connected;
        self.error = None;
        true
    }

    fn bound(&mut self) {
        self.words.sort_by(|left, right| {
            (left.start_ms, left.channel, left.end_ms, &left.id).cmp(&(
                right.start_ms,
                right.channel,
                right.end_ms,
                &right.id,
            ))
        });
        self.partials
            .sort_by_key(|word| (word.start_ms, word.channel, word.end_ms));
        if self.words.len() > MAX_WORDS {
            self.words.drain(..self.words.len() - MAX_WORDS);
        }
        if self.partials.len() > MAX_PARTIALS {
            self.partials.drain(..self.partials.len() - MAX_PARTIALS);
        }
        let mut bytes = self
            .words
            .iter()
            .chain(&self.partials)
            .map(|word| word.text.len())
            .sum::<usize>();
        while bytes > MAX_TEXT_BYTES {
            let word = if !self.words.is_empty() {
                self.words.remove(0)
            } else {
                self.partials.remove(0)
            };
            bytes -= word.text.len();
        }
    }

    fn clear(&mut self) {
        self.words.clear();
        self.partials.clear();
    }
}

pub(super) struct CaptionObserver {
    binding: CaptureLiveBinding,
    subscription: Option<Subscription>,
    recover: Arc<AtomicBool>,
    recovered: bool,
}
impl CaptionObserver {
    pub async fn close(mut self) {
        if let Some(subscription) = self.subscription.take() {
            subscription.close().await;
        }
    }
}

pub(super) async fn observe(
    current: &mut Option<CaptionObserver>,
    client: &Arc<RemoteClient>,
    identity: &CaptureIdentity,
    progress: &watch::Sender<DeviceProcessingProgress>,
    updates: &watch::Sender<u64>,
    stop: &CancellationToken,
) {
    let snapshot = progress.borrow().clone();
    let Some(status) = snapshot.status else {
        return;
    };
    let (Some(generation_id), Some(session_id)) = (status.live_generation_id, status.session_id)
    else {
        return;
    };
    if !matches!(&identity.target, CaptureTarget::ExistingSession { session_id: original, .. } if original == &session_id)
    {
        return;
    }
    let binding = CaptureLiveBinding {
        capture_id: identity.capture_id.clone(),
        generation_id,
        capture_fingerprint: snapshot.declaration.request.capture_fingerprint,
        session_id,
        format: snapshot.format,
    };
    if let Some(observer) = current {
        if observer.binding != binding {
            return;
        }
        if !observer.recover.swap(false, Ordering::AcqRel) || observer.recovered {
            return;
        }
        observer.recovered = true;
        if let Some(subscription) = observer.subscription.take() {
            subscription.close().await;
        }
        let after = progress
            .borrow()
            .captions
            .as_ref()
            .and_then(|value| value.next_sequence.checked_sub(1));
        let page = client
            .execute(
                RemoteOperation::CaptureLiveCaptions {
                    vault_id: identity.scope.vault_id.clone(),
                    binding: binding.clone(),
                    after,
                    limit: 8,
                },
                &uuid::Uuid::new_v4().to_string(),
                stop.clone(),
            )
            .await;
        match page {
            Ok(RemoteReply::CaptureLiveCaptions(page)) => {
                let mut accepted = false;
                progress.send_modify(|value| {
                    if let Some(captions) = &mut value.captions {
                        accepted = captions.page(page);
                    }
                });
                if !accepted {
                    return;
                }
            }
            Err(cause) => {
                progress.send_modify(|value| {
                    if let Some(captions) = &mut value.captions {
                        if matches!(
                            cause.code,
                            ErrorCode::Unauthenticated | ErrorCode::Forbidden | ErrorCode::NotFound
                        ) {
                            captions.clear();
                        }
                        captions.error = Some(cause);
                    }
                });
                return;
            }
            _ => return,
        }
    } else {
        progress
            .send_modify(|value| value.captions = Some(DeviceCaptionPreview::new(binding.clone())));
        *current = Some(CaptionObserver {
            binding: binding.clone(),
            subscription: None,
            recover: Arc::new(AtomicBool::new(false)),
            recovered: false,
        });
    }
    if stop.is_cancelled() {
        return;
    }
    let observer = current.as_mut().unwrap();
    let after = progress
        .borrow()
        .captions
        .as_ref()
        .and_then(|value| value.next_sequence.checked_sub(1));
    let delivery = progress.clone();
    let wake = updates.clone();
    let recover = observer.recover.clone();
    let connection = Arc::downgrade(client);
    let subscribed = client.subscribe_capture_live(
        identity.scope.vault_id.clone(),
        binding.clone(),
        after,
        move |event| {
            let closed = connection.upgrade().is_none_or(|client| client.is_closed());
            let mut accepted = true;
            delivery.send_modify(|value| {
                let Some(captions) = &mut value.captions else {
                    accepted = false;
                    return;
                };
                if captions.binding != binding {
                    accepted = false;
                    return;
                }
                match event {
                    CaptureLiveEvent::Connection { state, error } => {
                        if closed
                            || matches!(
                                state,
                                ConnectionState::AccessDenied | ConnectionState::LoginRequired
                            )
                        {
                            captions.clear();
                        }
                        if state == ConnectionState::Incompatible {
                            recover.store(true, Ordering::Release);
                        }
                        captions.connection = state;
                        captions.error = error;
                    }
                    CaptureLiveEvent::Page { page } => {
                        if closed {
                            captions.clear();
                            accepted = false;
                            return;
                        }
                        if !captions.page(page) {
                            recover.store(true, Ordering::Release);
                            accepted = false;
                        }
                    }
                }
            });
            wake.send_modify(|revision| *revision = revision.wrapping_add(1));
            accepted
        },
    );
    match subscribed {
        Ok(subscription) => observer.subscription = Some(subscription),
        Err(cause) => {
            progress.send_modify(|value| {
                if let Some(captions) = &mut value.captions {
                    captions.connection = ConnectionState::Incompatible;
                    captions.error = Some(cause);
                }
            });
        }
    }
    updates.send_modify(|revision| *revision = revision.wrapping_add(1));
}

pub(super) fn retire(client: &RemoteClient, progress: &watch::Sender<DeviceProcessingProgress>) {
    if client.is_closed() {
        progress.send_modify(|value| {
            if let Some(captions) = &mut value.captions {
                captions.clear();
                captions.connection = ConnectionState::Closed;
            }
        });
    }
}

#[cfg(test)]
#[path = "captions_tests.rs"]
mod tests;

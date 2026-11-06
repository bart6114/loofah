use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use hypr_loofah_protocol::{Change, ChangeEvent};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct EventHub(Arc<Mutex<State>>);

struct State {
    vault_id: String,
    epoch: String,
    sequence: u64,
    capacity: usize,
    replay: VecDeque<ChangeEvent>,
    sender: broadcast::Sender<ChangeEvent>,
}

pub struct Subscription {
    hub: EventHub,
    replay: VecDeque<ChangeEvent>,
    receiver: broadcast::Receiver<ChangeEvent>,
}

impl EventHub {
    pub fn new(vault_id: String, capacity: usize) -> Self {
        assert!(capacity > 0);
        Self(Arc::new(Mutex::new(State {
            vault_id,
            epoch: uuid::Uuid::new_v4().to_string(),
            sequence: 0,
            capacity,
            replay: VecDeque::new(),
            sender: broadcast::channel(capacity).0,
        })))
    }

    pub fn vault_id(&self) -> String {
        self.0.lock().unwrap().vault_id.clone()
    }

    pub fn publish(&self, change: Change) -> ChangeEvent {
        let change = match change {
            Change::IndexChanged {
                entity,
                mut ids,
                mut artifacts,
            } => {
                if ids.len() > 256 || ids.iter().map(String::len).sum::<usize>() > 32 * 1024 {
                    ids.clear();
                    artifacts = None;
                } else if artifacts.as_ref().is_some_and(|details| {
                    serde_json::to_vec(details).map_or(true, |bytes| bytes.len() > 32 * 1024)
                }) {
                    artifacts = None;
                }
                Change::IndexChanged {
                    entity,
                    ids,
                    artifacts,
                }
            }
            change => change,
        };
        let mut state = self.0.lock().unwrap();
        if state.sequence == u64::MAX {
            state.epoch = uuid::Uuid::new_v4().to_string();
            state.sequence = 0;
            state.replay.clear();
        }
        state.sequence += 1;
        let event = state.event(change);
        if state.replay.len() == state.capacity {
            state.replay.pop_front();
        }
        state.replay.push_back(event.clone());
        let _ = state.sender.send(event.clone());
        event
    }

    pub fn subscribe(&self, cursor: Option<&str>) -> Subscription {
        let state = self.0.lock().unwrap();
        let sequence = cursor.and_then(|cursor| {
            let (epoch, sequence) = cursor.rsplit_once(':')?;
            (epoch == state.epoch)
                .then(|| sequence.parse::<u64>().ok())
                .flatten()
        });
        let available = sequence.is_some_and(|sequence| {
            sequence <= state.sequence
                && state.replay.front().is_none_or(|first| {
                    first
                        .cursor
                        .rsplit_once(':')
                        .unwrap()
                        .1
                        .parse::<u64>()
                        .unwrap()
                        <= sequence.saturating_add(1)
                })
        });
        let replay = if available {
            state
                .replay
                .iter()
                .filter(|event| {
                    event
                        .cursor
                        .rsplit_once(':')
                        .unwrap()
                        .1
                        .parse::<u64>()
                        .unwrap()
                        > sequence.unwrap()
                })
                .cloned()
                .collect()
        } else {
            VecDeque::from([state.event(Change::ResyncRequired)])
        };
        // Subscribe while holding the same lock used by publishers, closing the replay/live gap.
        Subscription {
            hub: self.clone(),
            replay,
            receiver: state.sender.subscribe(),
        }
    }
}

impl State {
    fn event(&self, change: Change) -> ChangeEvent {
        ChangeEvent {
            vault_id: self.vault_id.clone(),
            cursor: format!("{}:{}", self.epoch, self.sequence),
            change,
        }
    }
}

impl Subscription {
    pub async fn recv(&mut self) -> Option<ChangeEvent> {
        if let Some(event) = self.replay.pop_front() {
            return Some(event);
        }
        match self.receiver.recv().await {
            Ok(event) => Some(event),
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let state = self.hub.0.lock().unwrap();
                self.receiver = state.sender.subscribe();
                Some(state.event(Change::ResyncRequired))
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detail_budget_falls_back_to_scoped_invalidation_and_replays_small_details() {
        use hypr_loofah_protocol::{ArtifactChange, ArtifactTarget, IndexEntity};
        let hub = EventHub::new("vault".into(), 4);
        let cursor = hub.publish(Change::SearchReady).cursor;
        let small = Change::IndexChanged {
            entity: IndexEntity::Artifacts,
            ids: vec!["first".into()],
            artifacts: Some(vec![ArtifactChange {
                session_id: "first".into(),
                target: ArtifactTarget::Attachment {
                    attachment_id: "image.png".into(),
                },
            }]),
        };
        hub.publish(small.clone());
        let mut replay = hub.subscribe(Some(&cursor));
        assert_eq!(replay.recv().await.unwrap().change, small);
        let overflow = hub.publish(Change::IndexChanged {
            entity: IndexEntity::Artifacts,
            ids: vec!["first".into(), "second".into()],
            artifacts: Some(
                (0..500)
                    .map(|i| ArtifactChange {
                        session_id: "first".into(),
                        target: ArtifactTarget::Attachment {
                            attachment_id: format!("{}-{i}.png", "a".repeat(200)),
                        },
                    })
                    .collect(),
            ),
        });
        assert_eq!(
            overflow.change,
            Change::IndexChanged {
                entity: IndexEntity::Artifacts,
                ids: vec!["first".into(), "second".into()],
                artifacts: None
            }
        );
    }

    #[test]
    fn bulk_invalidations_are_bounded_without_losing_the_changed_entity() {
        let hub = EventHub::new("vault".into(), 4);
        let event = hub.publish(Change::IndexChanged {
            entity: hypr_loofah_protocol::IndexEntity::Sessions,
            ids: (0..1000).map(|id| id.to_string()).collect(),
            artifacts: None,
        });
        assert_eq!(
            event.change,
            Change::IndexChanged {
                entity: hypr_loofah_protocol::IndexEntity::Sessions,
                ids: vec![],
                artifacts: None,
            }
        );
    }

    #[tokio::test]
    async fn reconnect_replays_then_delivers_live_without_a_gap() {
        let hub = EventHub::new("private".into(), 3);
        let first = hub.publish(Change::SearchReady);
        let second = hub.publish(Change::JobChanged {
            job_id: "job".into(),
        });
        let mut subscriber = hub.subscribe(Some(&first.cursor));
        let third = hub.publish(Change::SearchReady);
        assert_eq!(subscriber.recv().await, Some(second));
        assert_eq!(subscriber.recv().await, Some(third));
    }

    #[tokio::test]
    async fn overflow_restart_and_invalid_cursors_require_resync() {
        let hub = EventHub::new("shared".into(), 1);
        let first = hub.publish(Change::SearchReady);
        let mut slow = hub.subscribe(Some(&first.cursor));
        hub.publish(Change::SearchReady);
        let last = hub.publish(Change::SearchReady);
        for cursor in [
            None,
            Some("invalid"),
            Some(first.cursor.as_str()),
            Some("future:999"),
        ] {
            let event = hub.subscribe(cursor).recv().await.unwrap();
            assert_eq!(event.change, Change::ResyncRequired);
            assert_eq!(event.cursor, last.cursor);
            assert_eq!(event.vault_id, "shared");
        }
        assert_eq!(slow.recv().await.unwrap().change, Change::ResyncRequired);
        let fresh = hub.publish(Change::SearchReady);
        assert_eq!(slow.recv().await, Some(fresh));
        let restarted = EventHub::new("shared".into(), 1);
        assert_eq!(
            restarted
                .subscribe(Some(&last.cursor))
                .recv()
                .await
                .unwrap()
                .change,
            Change::ResyncRequired
        );
    }
}

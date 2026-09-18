use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::scope::Entity;

pub const IDLE_CHECKPOINT: Duration = Duration::from_secs(10);
pub const MAX_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(60);
pub const TRANSFER_CONCURRENCY: usize = 2;
pub const MULTIPART_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub struct Checkpoints {
    pending: BTreeMap<Entity, Pending>,
    generation: u64,
}

struct Pending {
    first: Instant,
    last: Instant,
    generation: u64,
    first_generation: u64,
    immediate: bool,
    recording: bool,
}

impl Checkpoints {
    pub fn len(&self) -> usize {
        self.pending.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
    pub fn changed(&mut self, entity: Entity, now: Instant, immediate: bool) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("checkpoint generation overflow");
        let item = self.pending.entry(entity).or_insert(Pending {
            first: now,
            last: now,
            generation: 0,
            first_generation: self.generation,
            immediate: false,
            recording: false,
        });
        item.last = now;
        item.generation = self.generation;
        item.immediate |= immediate;
    }

    pub fn recording(&mut self, entity: Entity, active: bool, now: Instant) {
        self.changed(entity.clone(), now, !active);
        self.pending.get_mut(&entity).unwrap().recording = active;
    }

    pub fn due(&self, now: Instant) -> Vec<(Entity, u64)> {
        self.pending
            .iter()
            .filter(|(_, pending)| {
                !pending.recording
                    && (pending.immediate
                        || now.saturating_duration_since(pending.last) >= IDLE_CHECKPOINT
                        || now.saturating_duration_since(pending.first) >= MAX_CHECKPOINT_INTERVAL)
            })
            .map(|(entity, pending)| (entity.clone(), pending.generation))
            .collect()
    }

    /// Acknowledging an older capture must not erase edits made while it uploaded.
    pub fn committed(&mut self, entity: &Entity, generation: u64, now: Instant) {
        if let Some(pending) = self.pending.get_mut(entity) {
            if pending.generation == generation {
                self.pending.remove(entity);
            } else if generation >= pending.first_generation && generation < pending.generation {
                pending.first = now;
                pending.first_generation = generation + 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_edits_are_bounded_and_inflight_edits_survive() {
        let mut queue = Checkpoints::default();
        let entity = Entity::Session("one".into());
        let now = Instant::now();
        for seconds in 0..=60 {
            queue.changed(entity.clone(), now + Duration::from_secs(seconds), false);
        }
        let due = queue.due(now + Duration::from_secs(60));
        assert_eq!(due.len(), 1);
        queue.changed(entity.clone(), now + Duration::from_secs(61), false);
        queue.committed(&entity, due[0].1, now + Duration::from_secs(62));
        assert_eq!(queue.due(now + Duration::from_secs(71)).len(), 1);
    }

    #[test]
    fn active_recording_defers_even_explicit_checkpoints() {
        let mut queue = Checkpoints::default();
        let entity = Entity::Session("one".into());
        let now = Instant::now();
        queue.recording(entity.clone(), true, now);
        queue.changed(entity.clone(), now, true);
        assert!(queue.due(now + Duration::from_secs(120)).is_empty());
        queue.recording(entity, false, now + Duration::from_secs(121));
        assert_eq!(queue.due(now + Duration::from_secs(121)).len(), 1);
    }

    #[test]
    fn duplicate_old_acknowledgment_cannot_clear_a_new_pending_edit() {
        let mut queue = Checkpoints::default();
        let entity = Entity::Session("one".into());
        let now = Instant::now();
        queue.changed(entity.clone(), now, true);
        let generation = queue.due(now)[0].1;
        queue.committed(&entity, generation, now);
        queue.changed(entity.clone(), now, true);
        queue.committed(&entity, generation, now);
        assert_eq!(queue.due(now).len(), 1);
    }
}

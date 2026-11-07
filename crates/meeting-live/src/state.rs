use crate::*;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const RETENTION: Duration = Duration::from_secs(600);
const STOP_RETENTION: Duration = Duration::from_secs(120);
const MAX_CHANGES: usize = 10_000;
const SNAPSHOT_WORDS: usize = 1_000;
const PAGE_CHANGES: usize = 500;

struct Recording {
    meeting_id: String,
    id: String,
    state: RecordingState,
    live: bool,
    stopped_at: Option<Instant>,
    sequence: u64,
    floor: u64,
    words: HashMap<String, (Instant, Word)>,
    journal: VecDeque<(u64, Instant, Change)>,
    truncated: bool,
}

#[derive(Default)]
pub struct LiveState {
    active: Option<String>,
    recordings: HashMap<String, Recording>,
}

impl LiveState {
    pub fn start(&mut self, meeting_id: &str, live: bool) -> String {
        self.prune();
        if let Some(recording) = self
            .active
            .as_ref()
            .and_then(|id| self.recordings.get_mut(id))
        {
            if recording.meeting_id == meeting_id {
                recording.live = live;
                return recording.id.clone();
            }
            recording.state = RecordingState::Stopped;
            recording.stopped_at = Some(Instant::now());
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.recordings.insert(
            id.clone(),
            Recording {
                meeting_id: meeting_id.into(),
                id: id.clone(),
                state: RecordingState::Recording,
                live,
                stopped_at: None,
                sequence: 0,
                floor: 0,
                words: HashMap::new(),
                journal: VecDeque::new(),
                truncated: false,
            },
        );
        self.active = Some(id.clone());
        id
    }

    pub fn transition(&mut self, meeting_id: &str, state: RecordingState) {
        // A previous session can finish after the next has already started.
        for recording in self
            .recordings
            .values_mut()
            .filter(|r| r.meeting_id == meeting_id && r.state != RecordingState::Stopped)
        {
            recording.state = state;
            if state == RecordingState::Stopped {
                recording.stopped_at = Some(Instant::now());
            }
            if self.active.as_ref() == Some(&recording.id) {
                self.active = None;
            }
        }
        self.prune();
    }

    pub fn active(&self) -> Option<ActiveMeeting> {
        let recording = self.recordings.get(self.active.as_ref()?)?;
        Some(ActiveMeeting {
            meeting_id: recording.meeting_id.clone(),
            recording_id: recording.id.clone(),
            title: String::new(),
            state: recording.state,
            live_transcription_available: recording.live,
            nudges_available: false,
        })
    }

    pub fn check_recording(&self, expected: Option<&str>) -> Result<ActiveMeeting> {
        let active = self
            .active()
            .ok_or_else(|| LiveError::new("no_active_recording", "No recording is active"))?;
        if expected.is_some_and(|id| id != active.recording_id) {
            return Err(LiveError::new(
                "recording_mismatch",
                "The requested recording is no longer active",
            ));
        }
        Ok(active)
    }

    pub fn append(&mut self, meeting_id: &str, words: Vec<Word>, removed: Vec<String>) {
        let target = self
            .active
            .as_ref()
            .filter(|id| {
                self.recordings
                    .get(*id)
                    .is_some_and(|r| r.meeting_id == meeting_id)
            })
            .cloned()
            .or_else(|| {
                self.recordings
                    .values()
                    .find(|r| r.meeting_id == meeting_id && r.state == RecordingState::Finalizing)
                    .map(|r| r.id.clone())
            });
        let Some(recording) = target.and_then(|id| self.recordings.get_mut(&id)) else {
            return;
        };
        let now = Instant::now();
        for id in removed {
            recording.words.remove(&id);
            recording.push(Change::Remove { id }, now);
        }
        for mut word in words {
            word.text = word.text.split_whitespace().collect::<Vec<_>>().join(" ");
            // Bound upstream data too, so one malformed provider response cannot grow the IPC buffer indefinitely.
            if word.text.len() > 512
                || word.id.len() > 128
                || word.speaker_label.len() > 128
                || word.text.chars().any(char::is_control)
                || word.id.chars().any(char::is_control)
                || word.speaker_label.chars().any(char::is_control)
            {
                recording.sequence += 1;
                recording.floor = recording.sequence;
                recording.journal.clear();
                recording.truncated = true;
                continue;
            }
            recording.words.insert(word.id.clone(), (now, word.clone()));
            recording.push(Change::Upsert { word }, now);
        }
        recording.prune(now);
    }

    pub fn transcript(&mut self, input: &TranscriptInput) -> Result<TranscriptPage> {
        self.prune();
        let recording = self.recordings.get_mut(&input.recording_id).ok_or_else(|| LiveError::new("recording_not_found", "Recording context is unavailable or expired; discover the active meeting again"))?;
        let sequence = input
            .cursor
            .as_ref()
            .map(|cursor| {
                let (id, sequence) = cursor.rsplit_once(':').ok_or_else(|| {
                    LiveError::new("invalid_cursor", "Malformed transcript cursor")
                })?;
                let sequence = sequence
                    .parse::<u64>()
                    .map_err(|_| LiveError::new("invalid_cursor", "Malformed transcript cursor"))?;
                if id != recording.id || sequence > recording.sequence {
                    return Err(LiveError::new(
                        "invalid_cursor",
                        "Cursor belongs to another recording or revision",
                    ));
                }
                Ok(sequence)
            })
            .transpose()?;
        let reset = sequence.is_none_or(|seq| seq < recording.floor);
        let (changes, next, has_more, truncated) = if reset {
            let newest = recording
                .words
                .values()
                .map(|(_, w)| w.end_ms)
                .max()
                .unwrap_or(0);
            let mut words: Vec<_> = recording
                .words
                .values()
                .filter(|(at, _)| at.elapsed() <= Duration::from_secs(300))
                .map(|(_, w)| w)
                .filter(|w| w.end_ms >= newest.saturating_sub(300_000))
                .collect();
            words.sort_by(|a, b| {
                (a.start_ms, a.channel, &a.id).cmp(&(b.start_ms, b.channel, &b.id))
            });
            let truncated = recording.truncated
                || words.len() > SNAPSHOT_WORDS
                || words.len() < recording.words.len();
            let offset = words.len().saturating_sub(SNAPSHOT_WORDS);
            (
                words
                    .into_iter()
                    .skip(offset)
                    .cloned()
                    .map(|word| Change::Upsert { word })
                    .collect(),
                recording.sequence,
                false,
                truncated,
            )
        } else {
            let seq = sequence.unwrap();
            let entries: Vec<_> = recording
                .journal
                .iter()
                .filter(|(n, _, _)| *n > seq)
                .take(PAGE_CHANGES)
                .collect();
            let next = entries.last().map(|(n, _, _)| *n).unwrap_or(seq);
            (
                entries
                    .into_iter()
                    .map(|(_, _, change)| change.clone())
                    .collect(),
                next,
                next < recording.sequence,
                false,
            )
        };
        Ok(TranscriptPage {
            meeting_id: recording.meeting_id.clone(),
            recording_id: recording.id.clone(),
            state: recording.state,
            live_transcription_available: recording.live,
            changes,
            cursor: format!("{}:{next}", recording.id),
            has_more,
            reset,
            truncated,
        })
    }

    fn prune(&mut self) {
        let now = Instant::now();
        self.recordings.retain(|_, r| {
            r.stopped_at
                .is_none_or(|at| now.duration_since(at) < STOP_RETENTION)
        });
        for r in self.recordings.values_mut() {
            r.prune(now);
        }
    }
}

impl Recording {
    fn push(&mut self, change: Change, now: Instant) {
        self.sequence += 1;
        self.journal.push_back((self.sequence, now, change));
    }
    fn prune(&mut self, now: Instant) {
        while self.journal.front().is_some_and(|(_, at, _)| {
            self.journal.len() > MAX_CHANGES || now.duration_since(*at) > RETENTION
        }) {
            if let Some((sequence, _, _)) = self.journal.pop_front() {
                self.floor = sequence;
            }
        }
        let before = self.words.len();
        self.words
            .retain(|_, (at, _)| now.duration_since(*at) <= RETENTION);
        if self.words.len() > MAX_CHANGES {
            let mut oldest: Vec<_> = self
                .words
                .iter()
                .map(|(id, (at, _))| (*at, id.clone()))
                .collect();
            oldest.sort();
            for (_, id) in oldest.into_iter().take(self.words.len() - MAX_CHANGES) {
                self.words.remove(&id);
            }
        }
        self.truncated |= self.words.len() < before;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn word(id: &str, text: &str) -> Word {
        Word {
            id: id.into(),
            text: text.into(),
            start_ms: 0,
            end_ms: 1,
            channel: 0,
            speaker_index: None,
            speaker_label: "You".into(),
        }
    }
    fn read(state: &mut LiveState, id: &str, cursor: Option<String>) -> TranscriptPage {
        state
            .transcript(&TranscriptInput {
                recording_id: id.into(),
                cursor,
            })
            .unwrap()
    }
    #[test]
    fn corrections_and_repeated_cursors_are_lossless() {
        let mut state = LiveState::default();
        let id = state.start("meeting", true);
        state.append("meeting", vec![word("a", "Monday")], vec![]);
        let first = read(&mut state, &id, None);
        state.append("meeting", vec![word("b", "Tuesday")], vec!["a".into()]);
        let next = read(&mut state, &id, Some(first.cursor.clone()));
        assert!(matches!(&next.changes[0], Change::Remove { id } if id == "a"));
        assert_eq!(next.changes.len(), 2);
        assert_eq!(read(&mut state, &id, Some(first.cursor)).changes.len(), 2);
        assert!(read(&mut state, &id, Some(next.cursor)).changes.is_empty());
        assert_eq!(read(&mut state, &id, None).changes.len(), 1);
    }
    #[test]
    fn stopped_recordings_drain_independently_and_resume_has_new_identity() {
        let mut state = LiveState::default();
        let old = state.start("one", true);
        state.transition("one", RecordingState::Finalizing);
        assert!(state.check_recording(Some(&old)).is_err());
        let new = state.start("two", false);
        state.append("one", vec![word("final", "Done")], vec![]);
        state.transition("one", RecordingState::Stopped);
        assert_eq!(state.active().unwrap().recording_id, new);
        assert_eq!(read(&mut state, &old, None).changes.len(), 1);
        assert!(!read(&mut state, &new, None).live_transcription_available);
        assert!(state.check_recording(Some(&old)).is_err());
        state.transition("two", RecordingState::Stopped);
        assert_ne!(state.start("one", true), old);
    }
    #[test]
    fn pagination_and_expired_cursors() {
        let mut state = LiveState::default();
        let id = state.start("one", true);
        let initial = read(&mut state, &id, None);
        state.append(
            "one",
            (0..501).map(|i| word(&i.to_string(), "hi")).collect(),
            vec![],
        );
        let page = read(&mut state, &id, Some(initial.cursor.clone()));
        assert_eq!(page.changes.len(), 500);
        assert!(page.has_more);
        assert_eq!(read(&mut state, &id, Some(page.cursor)).changes.len(), 1);
        state.append(
            "one",
            (501..10_002).map(|i| word(&i.to_string(), "hi")).collect(),
            vec![],
        );
        let reset = read(&mut state, &id, Some(initial.cursor));
        assert!(reset.reset && reset.truncated);
        assert_eq!(reset.changes.len(), 1000);
        assert!(
            state
                .transcript(&TranscriptInput {
                    recording_id: id,
                    cursor: Some("other:1".into())
                })
                .is_err()
        );
    }
    #[test]
    fn repeated_active_events_update_availability_without_resetting_context() {
        let mut state = LiveState::default();
        let id = state.start("meeting", true);
        state.append("meeting", vec![word("one", "Hello")], vec![]);
        assert_eq!(state.start("meeting", false), id);
        let page = read(&mut state, &id, None);
        assert!(!page.live_transcription_available);
        assert_eq!(page.changes.len(), 1);
    }

    #[test]
    fn journal_age_resets_and_stopped_context_expires() {
        let mut state = LiveState::default();
        let id = state.start("meeting", true);
        let first = read(&mut state, &id, None);
        state.append("meeting", vec![word("old", "Earlier")], vec![]);
        let recording = state.recordings.get_mut(&id).unwrap();
        recording.journal.front_mut().unwrap().1 =
            Instant::now() - RETENTION - Duration::from_secs(1);
        recording.words.get_mut("old").unwrap().0 =
            Instant::now() - RETENTION - Duration::from_secs(1);
        let page = read(&mut state, &id, Some(first.cursor));
        assert!(page.reset && page.truncated);
        assert!(page.changes.is_empty());
        state.transition("meeting", RecordingState::Stopped);
        state.recordings.get_mut(&id).unwrap().stopped_at = Some(Instant::now() - STOP_RETENTION);
        assert_eq!(
            state
                .transcript(&TranscriptInput {
                    recording_id: id,
                    cursor: None
                })
                .unwrap_err()
                .code,
            "recording_not_found"
        );
    }

    #[test]
    fn oversized_upstream_words_force_explicit_reset() {
        let mut state = LiveState::default();
        let id = state.start("meeting", true);
        let first = read(&mut state, &id, None);
        state.append("meeting", vec![word("huge", &"x".repeat(513))], vec![]);
        let page = read(&mut state, &id, Some(first.cursor));
        assert!(page.reset && page.truncated);
    }

    #[test]
    fn normalization_counts_unicode_characters() {
        assert_eq!(
            normalize_nudge("  Ask\n about  timing ").unwrap(),
            "Ask about timing"
        );
        assert!(normalize_nudge(&"é".repeat(200)).is_ok());
        assert!(normalize_nudge(&"é".repeat(201)).is_err());
        assert!(normalize_nudge(" \n ").is_err());
    }
}

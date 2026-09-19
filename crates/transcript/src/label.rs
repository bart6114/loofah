use std::collections::{HashMap, HashSet};

use crate::{ChannelProfile, Segment, SegmentKey};

#[derive(Debug, Clone, Default)]
pub struct SpeakerLabelContext {
    pub self_human_id: Option<String>,
    pub human_name_by_id: HashMap<String, String>,
    pub diarized_channels: HashSet<ChannelProfile>,
}

#[derive(Debug, Clone, Default)]
pub struct SpeakerLabeler {
    unknown_speaker_map: HashMap<SegmentKey, usize>,
    next_index: usize,
    max_unknown_speaker_number: Option<usize>,
}

impl SpeakerLabeler {
    pub fn new() -> Self {
        Self {
            unknown_speaker_map: HashMap::new(),
            next_index: 1,
            max_unknown_speaker_number: None,
        }
    }

    pub fn with_max_unknown_speaker_number(mut self, max: Option<usize>) -> Self {
        self.max_unknown_speaker_number = max;
        self
    }

    pub fn from_segments(
        segments: &[Segment],
        ctx: Option<&SpeakerLabelContext>,
        max_unknown_speaker_number: Option<usize>,
    ) -> Self {
        let mut labeler = Self::new().with_max_unknown_speaker_number(max_unknown_speaker_number);
        for segment in segments {
            if !segment.key.is_known_speaker(ctx) {
                labeler.unknown_speaker_number(&segment.key);
            }
        }
        labeler
    }

    pub fn label_for(&mut self, key: &SegmentKey, ctx: Option<&SpeakerLabelContext>) -> String {
        render_speaker_label(key, ctx, Some(self))
    }

    pub fn unknown_speaker_number(&mut self, key: &SegmentKey) -> usize {
        if let Some(existing) = self.unknown_speaker_map.get(key) {
            return *existing;
        }

        let next = self
            .max_unknown_speaker_number
            .map(|max| self.next_index.min(max))
            .unwrap_or(self.next_index);
        self.unknown_speaker_map.insert(key.clone(), next);
        self.next_index += 1;
        next
    }
}

impl SegmentKey {
    pub fn is_current_user(&self, ctx: &SpeakerLabelContext) -> bool {
        match self.speaker_human_id.as_ref() {
            Some(id) => ctx.self_human_id.as_ref() == Some(id),
            None => self.is_heuristic_self(ctx),
        }
    }

    pub fn is_known_speaker(&self, ctx: Option<&SpeakerLabelContext>) -> bool {
        if self.speaker_human_id.is_some() {
            return true;
        }

        ctx.is_some_and(|ctx| self.is_heuristic_self(ctx))
    }

    // DirectMic implies "self" only while the channel is undiarized; once
    // diarization separates speakers on it, an indexed key may be someone
    // else picked up by the mic, so it must stay an unknown speaker.
    fn is_heuristic_self(&self, ctx: &SpeakerLabelContext) -> bool {
        self.channel == ChannelProfile::DirectMic
            && ctx.self_human_id.is_some()
            && !(self.speaker_index.is_some() && ctx.diarized_channels.contains(&self.channel))
    }
}

pub fn render_speaker_label(
    key: &SegmentKey,
    ctx: Option<&SpeakerLabelContext>,
    mut labeler: Option<&mut SpeakerLabeler>,
) -> String {
    if let Some(ctx) = ctx {
        if let Some(human_id) = key.speaker_human_id.as_ref() {
            if let Some(name) = ctx.human_name_by_id.get(human_id) {
                return name.clone();
            }
            return human_id.clone();
        }

        if key.is_heuristic_self(ctx)
            && let Some(self_human_id) = ctx.self_human_id.as_ref()
        {
            if let Some(name) = ctx.human_name_by_id.get(self_human_id) {
                return name.clone();
            }
            return "You".to_string();
        }
    } else if let Some(human_id) = key.speaker_human_id.as_ref() {
        return human_id.clone();
    }

    if let Some(labeler) = labeler.as_mut() {
        return format!("Speaker {}", labeler.unknown_speaker_number(key));
    }

    let channel_label = match key.channel {
        ChannelProfile::DirectMic => "A",
        ChannelProfile::RemoteParty => "B",
        ChannelProfile::MixedCapture => "C",
    };

    match key.speaker_index {
        Some(index) => format!("Speaker {}", index + 1),
        None => format!("Speaker {channel_label}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChannelProfile;

    #[test]
    fn current_user_identity_respects_explicit_assignments_and_ambiguity() {
        let mut ctx = SpeakerLabelContext {
            self_human_id: Some("self".into()),
            ..Default::default()
        };
        let mut key = direct_mic_key();
        assert!(key.is_current_user(&ctx));
        key.speaker_human_id = Some("other".into());
        assert!(!key.is_current_user(&ctx));
        key.speaker_human_id = None;
        key.speaker_index = Some(0);
        ctx.diarized_channels.insert(ChannelProfile::DirectMic);
        assert!(!key.is_current_user(&ctx));
        key.channel = ChannelProfile::MixedCapture;
        assert!(!key.is_current_user(&ctx));
        key.speaker_human_id = Some("self".into());
        assert!(key.is_current_user(&ctx));
    }

    fn direct_mic_key() -> SegmentKey {
        SegmentKey {
            channel: ChannelProfile::DirectMic,
            speaker_index: None,
            speaker_human_id: None,
        }
    }

    #[test]
    fn renders_direct_mic_as_you_when_self_exists_without_name() {
        let ctx = SpeakerLabelContext {
            self_human_id: Some("self".to_string()),
            ..Default::default()
        };

        assert_eq!(
            render_speaker_label(&direct_mic_key(), Some(&ctx), None),
            "You"
        );
    }

    #[test]
    fn preserves_unknown_speaker_numbers_across_calls() {
        let mut labeler = SpeakerLabeler::new();
        let a = SegmentKey {
            channel: ChannelProfile::DirectMic,
            speaker_index: Some(7),
            speaker_human_id: None,
        };
        let b = SegmentKey {
            channel: ChannelProfile::RemoteParty,
            speaker_index: Some(9),
            speaker_human_id: None,
        };

        assert_eq!(labeler.label_for(&a, None), "Speaker 1");
        assert_eq!(labeler.label_for(&b, None), "Speaker 2");
        assert_eq!(labeler.label_for(&a, None), "Speaker 1");
    }

    #[test]
    fn caps_unknown_speaker_numbers() {
        let mut labeler = SpeakerLabeler::new().with_max_unknown_speaker_number(Some(2));
        let a = SegmentKey {
            channel: ChannelProfile::DirectMic,
            speaker_index: Some(0),
            speaker_human_id: None,
        };
        let b = SegmentKey {
            channel: ChannelProfile::RemoteParty,
            speaker_index: Some(1),
            speaker_human_id: None,
        };
        let c = SegmentKey {
            channel: ChannelProfile::RemoteParty,
            speaker_index: Some(2),
            speaker_human_id: None,
        };

        assert_eq!(labeler.label_for(&a, None), "Speaker 1");
        assert_eq!(labeler.label_for(&b, None), "Speaker 2");
        assert_eq!(labeler.label_for(&c, None), "Speaker 2");
    }

    #[test]
    fn treats_direct_mic_with_provider_speaker_as_self() {
        let ctx = SpeakerLabelContext {
            self_human_id: Some("self".to_string()),
            ..Default::default()
        };
        let key = SegmentKey {
            channel: ChannelProfile::DirectMic,
            speaker_index: Some(2),
            speaker_human_id: None,
        };

        assert!(key.is_known_speaker(Some(&ctx)));
        assert_eq!(render_speaker_label(&key, Some(&ctx), None), "You");
    }
}

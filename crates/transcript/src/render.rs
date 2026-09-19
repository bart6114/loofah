use std::collections::{HashMap, HashSet};

use crate::{
    ChannelProfile, FinalizedWord, IdentityAssignment, IdentityScope, SegmentBuilderOptions,
    SegmentKey, SegmentWord, SpeakerLabelContext, SpeakerLabeler, WordState, build_segments,
    channel_assignments_for_participants, render_speaker_label, segment_options_for_participants,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct RenderTranscriptWordInput {
    pub id: String,
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub channel: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_index: Option<i32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct RenderTranscriptHuman {
    pub human_id: String,
    pub name: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct RenderTranscriptInput {
    pub started_at: Option<i64>,
    pub words: Vec<RenderTranscriptWordInput>,
    pub assignments: Vec<IdentityAssignment>,
    /// True when word timings are synthetic (e.g. Soniqo batch), meaning they
    /// are too imprecise for word-level cross-channel interleaving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthetic_timing: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct RenderTranscriptRequest {
    pub transcripts: Vec<RenderTranscriptInput>,
    pub participant_human_ids: Vec<String>,
    pub self_human_id: Option<String>,
    pub humans: Vec<RenderTranscriptHuman>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct RenderedTranscriptSegment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_current_user: Option<bool>,
    pub id: String,
    pub key: SegmentKey,
    pub speaker_label: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub words: Vec<SegmentWord>,
}

pub fn render_transcript_segments(
    request: RenderTranscriptRequest,
) -> Vec<RenderedTranscriptSegment> {
    let RenderTranscriptRequest {
        transcripts,
        participant_human_ids,
        self_human_id,
        humans,
    } = request;

    let base_started_at = earliest_started_at(&transcripts);
    let segment_options =
        segment_options_for_participants(&participant_human_ids, self_human_id.as_deref());

    let mut all_segments = Vec::new();
    let mut diarized_channels: HashSet<ChannelProfile> = HashSet::new();

    for transcript in transcripts {
        diarized_channels.extend(diarized_channels_for_words(&transcript.words));

        let offset = transcript
            .started_at
            .map(|started_at| started_at - base_started_at)
            .unwrap_or(0);
        let synthetic_timing = transcript.synthetic_timing.unwrap_or(false);

        let (words, transcript_assignments) =
            offset_transcript_data(transcript.words, transcript.assignments, offset);

        // Channels named by the transcript's own (hint-derived, user-explicit) channel
        // assignments must always render, so they extend the participant-heuristic
        // complete_channels rather than being gated by it.
        let explicit_channels: Vec<ChannelProfile> = transcript_assignments
            .iter()
            .filter_map(|assignment| match assignment.scope {
                IdentityScope::Channel { channel } => Some(channel),
                _ => None,
            })
            .collect();

        // Heuristics first: identity maps are last-insert-wins, and explicit
        // assignments must beat the participant heuristic on conflict.
        let heuristic_assignments =
            channel_assignments_for_participants(&participant_human_ids, self_human_id.as_deref());

        // Channels whose identity is purely heuristic (not confirmed by an
        // explicit assignment) must not absorb diarized speaker indexes.
        let heuristic_channels: Vec<ChannelProfile> = heuristic_assignments
            .iter()
            .filter_map(|assignment| match assignment.scope {
                IdentityScope::Channel { channel } => Some(channel),
                _ => None,
            })
            .filter(|channel| !explicit_channels.contains(channel))
            .collect();

        let mut assignments = heuristic_assignments;
        assignments.extend(transcript_assignments);

        let mut complete_channels = segment_options
            .complete_channels
            .clone()
            .unwrap_or_default();
        for channel in explicit_channels {
            if !complete_channels.contains(&channel) {
                complete_channels.push(channel);
            }
        }

        let options = SegmentBuilderOptions {
            sentence_atomic: Some(synthetic_timing),
            complete_channels: Some(complete_channels),
            heuristic_channels: Some(heuristic_channels),
            ..segment_options.clone()
        };
        let segments = build_segments(&words, &[], &assignments, Some(&options));
        all_segments.extend(segments);
    }

    all_segments.sort_by_key(|seg| seg.words.first().map(|w| w.start_ms).unwrap_or(i64::MAX));

    // Two distinct diarized speakers must never share one "Speaker N" label,
    // so the participant-count cap only applies to undiarized transcripts.
    let max_speaker_number = diarized_channels
        .is_empty()
        .then(|| {
            max_speaker_number_for_participants(&participant_human_ids, self_human_id.as_deref())
        })
        .flatten();
    let ctx = SpeakerLabelContext {
        self_human_id: self_human_id.clone(),
        human_name_by_id: humans
            .into_iter()
            .map(|human| (human.human_id, human.name))
            .collect::<HashMap<_, _>>(),
        diarized_channels,
    };
    let mut labeler = SpeakerLabeler::from_segments(&all_segments, Some(&ctx), max_speaker_number);

    all_segments
        .into_iter()
        .filter_map(|segment| {
            let words = normalize_rendered_segment_words(segment.words);
            let first = words.first()?;
            let last = words.last()?;
            let text = words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<String>()
                .trim()
                .to_string();
            if text.is_empty() {
                return None;
            }

            Some(RenderedTranscriptSegment {
                is_current_user: Some(segment.key.is_current_user(&ctx)),
                id: stable_segment_id(&segment.key, &words),
                speaker_label: render_speaker_label(&segment.key, Some(&ctx), Some(&mut labeler)),
                start_ms: first.start_ms,
                end_ms: last.end_ms,
                text,
                words,
                key: segment.key,
            })
        })
        .collect()
}

// A channel is diarized when at least two distinct speaker indexes appear in
// its final words (all words in this path are final).
fn diarized_channels_for_words(words: &[RenderTranscriptWordInput]) -> HashSet<ChannelProfile> {
    let mut indexes_by_channel: HashMap<ChannelProfile, HashSet<i32>> = HashMap::new();
    for word in words {
        if let Some(speaker_index) = word.speaker_index {
            indexes_by_channel
                .entry(ChannelProfile::from(word.channel))
                .or_default()
                .insert(speaker_index);
        }
    }

    indexes_by_channel
        .into_iter()
        .filter_map(|(channel, indexes)| (indexes.len() > 1).then_some(channel))
        .collect()
}

fn max_speaker_number_for_participants(
    participant_human_ids: &[String],
    self_human_id: Option<&str>,
) -> Option<usize> {
    let mut participants = participant_human_ids.to_vec();

    if let Some(self_human_id) = self_human_id
        && !participants.iter().any(|id| id == self_human_id)
    {
        participants.push(self_human_id.to_string());
    }

    participants.sort();
    participants.dedup();

    (participants.len() > 1).then_some(participants.len())
}

fn offset_transcript_data(
    raw_words: Vec<RenderTranscriptWordInput>,
    assignments: Vec<IdentityAssignment>,
    time_offset: i64,
) -> (Vec<FinalizedWord>, Vec<IdentityAssignment>) {
    let words: Vec<FinalizedWord> = raw_words
        .into_iter()
        .map(|w| FinalizedWord {
            id: w.id,
            text: w.text,
            start_ms: w.start_ms + time_offset,
            end_ms: w.end_ms + time_offset,
            channel: w.channel,
            state: WordState::Final,
            speaker_index: w.speaker_index,
        })
        .collect();

    (words, assignments)
}

fn earliest_started_at(transcripts: &[RenderTranscriptInput]) -> i64 {
    transcripts
        .iter()
        .filter_map(|transcript| transcript.started_at)
        .min()
        .unwrap_or(0)
}

pub fn normalize_rendered_segment_words(words: Vec<SegmentWord>) -> Vec<SegmentWord> {
    words
        .into_iter()
        .enumerate()
        .map(|(index, mut word)| {
            word.text = normalized_rendered_word_text(&word.text, index == 0);
            word
        })
        .collect()
}

pub fn stable_segment_id(key: &SegmentKey, words: &[SegmentWord]) -> String {
    let first_anchor = words
        .first()
        .map(|word| {
            word.id
                .clone()
                .unwrap_or_else(|| format!("start:{}", word.start_ms))
        })
        .unwrap_or_else(|| "none".to_string());
    let last_anchor = words
        .last()
        .map(|word| {
            word.id
                .clone()
                .unwrap_or_else(|| format!("end:{}", word.end_ms))
        })
        .unwrap_or_else(|| "none".to_string());

    // Deliberately excludes speaker_human_id: the id doubles as the React key and
    // identity-cache key, and a speaker rename must not remount the segment.
    // The word anchors already make the id unique.
    format!(
        "{}:{}:{}:{}",
        key.channel as i32,
        key.speaker_index
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        first_anchor,
        last_anchor
    )
}

fn normalized_rendered_word_text(text: &str, is_first_word: bool) -> String {
    let trimmed_start = text.trim_start();
    if trimmed_start.is_empty() {
        return text.to_string();
    }

    if is_first_word {
        return trimmed_start.to_string();
    }

    if text.starts_with(' ') {
        return text.to_string();
    }

    if trimmed_start.starts_with(|c: char| ",.;:!?)}]'".contains(c)) {
        return trimmed_start.to_string();
    }

    format!(" {trimmed_start}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChannelProfile, IdentityScope};

    fn word(
        id: &str,
        text: &str,
        start_ms: i64,
        end_ms: i64,
        channel: i32,
    ) -> RenderTranscriptWordInput {
        RenderTranscriptWordInput {
            id: id.to_string(),
            text: text.to_string(),
            start_ms,
            end_ms,
            channel,
            speaker_index: None,
        }
    }

    fn word_si(
        id: &str,
        text: &str,
        start_ms: i64,
        end_ms: i64,
        channel: i32,
        speaker_index: i32,
    ) -> RenderTranscriptWordInput {
        RenderTranscriptWordInput {
            id: id.to_string(),
            text: text.to_string(),
            start_ms,
            end_ms,
            channel,
            speaker_index: Some(speaker_index),
        }
    }

    fn channel_assignment(human_id: &str, channel: ChannelProfile) -> IdentityAssignment {
        IdentityAssignment {
            human_id: human_id.to_string(),
            scope: IdentityScope::Channel { channel },
        }
    }

    fn speaker_assignment(
        human_id: &str,
        channel: ChannelProfile,
        speaker_index: i32,
    ) -> IdentityAssignment {
        IdentityAssignment {
            human_id: human_id.to_string(),
            scope: IdentityScope::ChannelSpeaker {
                channel,
                speaker_index,
            },
        }
    }

    #[test]
    fn speaker_assignment_keeps_segment_ids_stable() {
        let render = |assignments: Vec<IdentityAssignment>| {
            render_transcript_segments(RenderTranscriptRequest {
                transcripts: vec![RenderTranscriptInput {
                    started_at: Some(0),
                    words: vec![
                        word("w1", " remote", 0, 100, 1),
                        word("w2", " reply", 120, 220, 1),
                    ],
                    assignments,
                    synthetic_timing: None,
                }],
                participant_human_ids: vec![],
                self_human_id: None,
                humans: vec![],
            })
        };

        let before = render(vec![]);
        let after = render(vec![channel_assignment(
            "christel",
            ChannelProfile::RemoteParty,
        )]);

        assert_eq!(before.len(), 1);
        assert_eq!(after.len(), 1);
        assert_ne!(before[0].speaker_label, after[0].speaker_label);
        assert_eq!(before[0].id, after[0].id);
    }

    #[test]
    fn renders_segments_with_labels() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![
                    word("w1", " hello", 0, 100, 0),
                    word("w2", " world", 120, 240, 1),
                ],
                assignments: vec![],
                synthetic_timing: None,
            }],
            participant_human_ids: vec!["human-1".to_string(), "human-2".to_string()],
            self_human_id: Some("human-1".to_string()),
            humans: vec![
                RenderTranscriptHuman {
                    human_id: "human-1".to_string(),
                    name: "Alice".to_string(),
                },
                RenderTranscriptHuman {
                    human_id: "human-2".to_string(),
                    name: "Bob".to_string(),
                },
            ],
        });

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].speaker_label, "Alice");
        assert_eq!(segments[0].text, "hello");
        assert_eq!(segments[1].speaker_label, "Bob");
        assert_eq!(segments[1].text, "world");
    }

    #[test]
    fn caps_unknown_speaker_labels_to_participant_count() {
        // The cap only holds while no channel is diarized (one index per channel).
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![
                    word("w1", " one", 0, 100, 0),
                    word_si("w2", " two", 200, 300, 1, 5),
                    word_si("w3", " three", 400, 500, 2, 7),
                ],
                assignments: vec![],
                synthetic_timing: None,
            }],
            participant_human_ids: vec!["a".to_string(), "b".to_string()],
            self_human_id: None,
            humans: vec![],
        });

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].speaker_label, "Speaker 1");
        assert_eq!(segments[1].speaker_label, "Speaker 2");
        assert_eq!(segments[2].speaker_label, "Speaker 2");
    }

    #[test]
    fn cap_lifts_when_indexes_exceed_participants() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![
                    word_si("w1", " one", 0, 100, 2, 0),
                    word_si("w2", " two", 200, 300, 2, 1),
                    word_si("w3", " three", 400, 500, 2, 2),
                ],
                assignments: vec![],
                synthetic_timing: None,
            }],
            participant_human_ids: vec!["self".to_string(), "remote".to_string()],
            self_human_id: Some("self".to_string()),
            humans: vec![],
        });

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].speaker_label, "Speaker 1");
        assert_eq!(segments[1].speaker_label, "Speaker 2");
        assert_eq!(segments[2].speaker_label, "Speaker 3");
    }

    #[test]
    fn multi_index_mic_channel_not_labeled_you() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![
                    word_si("w1", " first", 0, 400, 0, 0),
                    word_si("w2", " voice", 500, 900, 0, 0),
                    word_si("w3", " second", 5_000, 5_400, 0, 1),
                    word_si("w4", " voice", 5_500, 5_900, 0, 1),
                ],
                assignments: vec![],
                synthetic_timing: None,
            }],
            participant_human_ids: vec![],
            self_human_id: Some("self".to_string()),
            humans: vec![RenderTranscriptHuman {
                human_id: "self".to_string(),
                name: "Me".to_string(),
            }],
        });

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].speaker_label, "Speaker 1");
        assert_eq!(segments[1].speaker_label, "Speaker 2");
        assert!(
            segments
                .iter()
                .all(|segment| segment.key.speaker_human_id.is_none())
        );
    }

    #[test]
    fn indexed_assignment_keeps_channel_fallback() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![
                    word_si("w1", " zero", 0, 400, 1, 0),
                    word_si("w2", " speaks", 500, 900, 1, 0),
                    word_si("w3", " one", 4_000, 4_400, 1, 1),
                    word_si("w4", " speaks", 4_500, 4_900, 1, 1),
                    word("w5", " gap", 8_500, 8_900, 1),
                    word("w6", " words", 9_000, 9_400, 1),
                ],
                assignments: vec![
                    channel_assignment("alice", ChannelProfile::RemoteParty),
                    speaker_assignment("bob", ChannelProfile::RemoteParty, 1),
                ],
                synthetic_timing: None,
            }],
            participant_human_ids: vec![],
            self_human_id: None,
            humans: vec![
                RenderTranscriptHuman {
                    human_id: "alice".to_string(),
                    name: "Alice".to_string(),
                },
                RenderTranscriptHuman {
                    human_id: "bob".to_string(),
                    name: "Bob".to_string(),
                },
            ],
        });

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].speaker_label, "Alice");
        assert_eq!(segments[0].key.speaker_index, Some(0));
        assert_eq!(segments[1].speaker_label, "Bob");
        assert_eq!(segments[1].key.speaker_index, Some(1));
        assert_eq!(segments[2].speaker_label, "Alice");
        assert_eq!(segments[2].key.speaker_index, None);
    }

    #[test]
    fn multiple_indexed_assignments_on_one_channel_all_apply() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![
                    word_si("w1", " zero", 0, 400, 1, 0),
                    word_si("w2", " one", 4_000, 4_400, 1, 1),
                    word_si("w3", " two", 8_000, 8_400, 1, 2),
                ],
                assignments: vec![
                    speaker_assignment("py", ChannelProfile::RemoteParty, 0),
                    speaker_assignment("py", ChannelProfile::RemoteParty, 1),
                    speaker_assignment("py", ChannelProfile::RemoteParty, 2),
                ],
                synthetic_timing: None,
            }],
            participant_human_ids: vec![],
            self_human_id: None,
            humans: vec![RenderTranscriptHuman {
                human_id: "py".to_string(),
                name: "PY".to_string(),
            }],
        });

        assert!(
            segments.iter().all(|segment| segment.speaker_label == "PY"),
            "every diarized cluster must take its scoped assignment, got: {:?}",
            segments
                .iter()
                .map(|s| s.speaker_label.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn undiarized_channel_behavior_unchanged() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![
                    word_si("w1", " hello", 0, 400, 0, 2),
                    word("w2", " reply", 1_000, 1_400, 1),
                    word("w3", " here", 1_500, 1_900, 1),
                ],
                assignments: vec![],
                synthetic_timing: None,
            }],
            participant_human_ids: vec!["self".to_string(), "remote".to_string()],
            self_human_id: Some("self".to_string()),
            humans: vec![
                RenderTranscriptHuman {
                    human_id: "self".to_string(),
                    name: "Me".to_string(),
                },
                RenderTranscriptHuman {
                    human_id: "remote".to_string(),
                    name: "Bob".to_string(),
                },
            ],
        });

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].speaker_label, "Me");
        assert_eq!(segments[0].key.speaker_human_id.as_deref(), Some("self"));
        assert_eq!(segments[1].speaker_label, "Bob");
        assert_eq!(segments[1].key.speaker_human_id.as_deref(), Some("remote"));
    }

    #[test]
    fn labels_diarized_direct_mic_as_self_without_remote_participant() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![word_si("w1", " hello", 0, 100, 0, 2)],
                assignments: vec![],
                synthetic_timing: None,
            }],
            participant_human_ids: vec![],
            self_human_id: Some("self".to_string()),
            humans: vec![RenderTranscriptHuman {
                human_id: "self".to_string(),
                name: "Me".to_string(),
            }],
        });

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].speaker_label, "Me");
        assert_eq!(segments[0].key.speaker_index, Some(2));
        assert_eq!(segments[0].key.speaker_human_id.as_deref(), Some("self"));
    }

    #[test]
    fn normalizes_word_spacing_for_rendered_segments() {
        let words = normalize_rendered_segment_words(vec![
            SegmentWord {
                text: "What".to_string(),
                start_ms: 0,
                end_ms: 100,
                channel: crate::ChannelProfile::DirectMic,
                is_final: true,
                id: Some("w1".to_string()),
            },
            SegmentWord {
                text: "do".to_string(),
                start_ms: 100,
                end_ms: 200,
                channel: crate::ChannelProfile::DirectMic,
                is_final: true,
                id: Some("w2".to_string()),
            },
            SegmentWord {
                text: "'s".to_string(),
                start_ms: 200,
                end_ms: 250,
                channel: crate::ChannelProfile::DirectMic,
                is_final: true,
                id: Some("w3".to_string()),
            },
        ]);

        assert_eq!(words[0].text, "What");
        assert_eq!(words[1].text, " do");
        assert_eq!(words[2].text, "'s");
    }

    #[test]
    fn propagates_remote_labels_when_complete_channel_is_requested() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![
                    word("w1", " remote", 0, 100, 1),
                    word("w2", " reply", 120, 220, 1),
                ],
                assignments: vec![channel_assignment("remote", ChannelProfile::RemoteParty)],
                synthetic_timing: None,
            }],
            participant_human_ids: vec!["self".to_string(), "remote".to_string()],
            self_human_id: None,
            humans: vec![RenderTranscriptHuman {
                human_id: "remote".to_string(),
                name: "Remote".to_string(),
            }],
        });

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].speaker_label, "Remote");
        assert_eq!(segments[0].text, "remote reply");
    }

    #[test]
    fn keeps_same_provider_speaker_index_isolated_per_channel() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![
                    word_si("w1", " john", 0, 100, 0, 0),
                    word_si("w1b", " says", 120, 220, 0, 0),
                    word_si("w1c", " hi", 240, 340, 0, 0),
                    word_si("w2", " janet", 500, 600, 1, 0),
                    word_si("w2b", " replies", 620, 720, 1, 0),
                    word_si("w2c", " back", 740, 840, 1, 0),
                    word_si("w3", " again", 1_000, 1_100, 0, 0),
                ],
                assignments: vec![
                    speaker_assignment("john", ChannelProfile::DirectMic, 0),
                    speaker_assignment("janet", ChannelProfile::RemoteParty, 0),
                ],
                synthetic_timing: None,
            }],
            participant_human_ids: vec![],
            self_human_id: None,
            humans: vec![
                RenderTranscriptHuman {
                    human_id: "john".to_string(),
                    name: "John".to_string(),
                },
                RenderTranscriptHuman {
                    human_id: "janet".to_string(),
                    name: "Janet".to_string(),
                },
            ],
        });

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].speaker_label, "John");
        assert_eq!(segments[1].speaker_label, "Janet");
        assert_eq!(segments[2].speaker_label, "John");
    }

    #[test]
    fn normalizes_multi_row_offsets_from_earliest_transcript() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![
                RenderTranscriptInput {
                    started_at: Some(5_000),
                    words: vec![word("late", " later", 100, 200, 1)],
                    assignments: vec![],
                    synthetic_timing: None,
                },
                RenderTranscriptInput {
                    started_at: Some(1_000),
                    words: vec![word("early", " hello", 0, 100, 0)],
                    assignments: vec![],
                    synthetic_timing: None,
                },
            ],
            participant_human_ids: vec!["self".to_string(), "remote".to_string()],
            self_human_id: Some("self".to_string()),
            humans: vec![RenderTranscriptHuman {
                human_id: "self".to_string(),
                name: "Me".to_string(),
            }],
        });

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "hello");
        assert_eq!(segments[0].start_ms, 0);
        assert_eq!(segments[1].text, "later");
        assert_eq!(segments[1].start_ms, 4_100);
    }

    #[test]
    fn applies_explicit_channel_assignment_without_participants() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![
                    word("w1", " remote", 0, 100, 1),
                    word("w2", " reply", 120, 220, 1),
                ],
                assignments: vec![channel_assignment("remote", ChannelProfile::RemoteParty)],
                synthetic_timing: None,
            }],
            participant_human_ids: vec![],
            self_human_id: None,
            humans: vec![RenderTranscriptHuman {
                human_id: "remote".to_string(),
                name: "Remote".to_string(),
            }],
        });

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].speaker_label, "Remote");
        assert_eq!(segments[0].key.speaker_human_id.as_deref(), Some("remote"));
    }

    #[test]
    fn explicit_channel_assignment_beats_participant_heuristic() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![word("w1", " hello", 0, 100, 0)],
                assignments: vec![channel_assignment("bob_peters", ChannelProfile::DirectMic)],
                synthetic_timing: None,
            }],
            participant_human_ids: vec![],
            self_human_id: Some("self".to_string()),
            humans: vec![RenderTranscriptHuman {
                human_id: "bob_peters".to_string(),
                name: "Bob Peters".to_string(),
            }],
        });

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].speaker_label, "Bob Peters");
        assert_eq!(
            segments[0].key.speaker_human_id.as_deref(),
            Some("bob_peters")
        );
    }

    #[test]
    fn applies_explicit_assignment_on_mixed_capture_channel() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![word("w1", " imported", 0, 100, 2)],
                assignments: vec![channel_assignment("kim", ChannelProfile::MixedCapture)],
                synthetic_timing: None,
            }],
            participant_human_ids: vec![],
            self_human_id: None,
            humans: vec![],
        });

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].speaker_label, "kim");
        assert_eq!(segments[0].key.speaker_human_id.as_deref(), Some("kim"));
    }

    #[test]
    fn renders_raw_human_id_when_no_matching_human() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![word("w1", " remote", 0, 100, 1)],
                assignments: vec![channel_assignment(
                    "bob_peters",
                    ChannelProfile::RemoteParty,
                )],
                synthetic_timing: None,
            }],
            participant_human_ids: vec![],
            self_human_id: None,
            humans: vec![],
        });

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].speaker_label, "bob_peters");
    }

    #[test]
    fn propagates_remote_labels_when_self_not_in_participant_list() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![
                    word("w1", " hello", 0, 100, 0),
                    word("w2", " remote", 120, 220, 1),
                    word("w3", " more", 240, 340, 1),
                ],
                assignments: vec![channel_assignment("remote", ChannelProfile::RemoteParty)],
                synthetic_timing: None,
            }],
            participant_human_ids: vec!["remote".to_string()],
            self_human_id: Some("self".to_string()),
            humans: vec![
                RenderTranscriptHuman {
                    human_id: "self".to_string(),
                    name: "Me".to_string(),
                },
                RenderTranscriptHuman {
                    human_id: "remote".to_string(),
                    name: "Remote".to_string(),
                },
            ],
        });

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].speaker_label, "Me");
        assert_eq!(segments[1].speaker_label, "Remote");
        assert_eq!(segments[1].text, "remote more");
    }

    #[test]
    fn keeps_missing_started_at_rows_anchored_at_zero() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![
                RenderTranscriptInput {
                    started_at: None,
                    words: vec![word("missing-start", " hello", 0, 100, 0)],
                    assignments: vec![],
                    synthetic_timing: None,
                },
                RenderTranscriptInput {
                    started_at: Some(1_000),
                    words: vec![word("known-start", " later", 100, 200, 1)],
                    assignments: vec![],
                    synthetic_timing: None,
                },
            ],
            participant_human_ids: vec!["self".to_string(), "remote".to_string()],
            self_human_id: Some("self".to_string()),
            humans: vec![RenderTranscriptHuman {
                human_id: "self".to_string(),
                name: "Me".to_string(),
            }],
        });

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "hello");
        assert_eq!(segments[0].start_ms, 0);
        assert_eq!(segments[1].text, "later");
        assert_eq!(segments[1].start_ms, 100);
    }

    fn cross_talk_words() -> Vec<RenderTranscriptWordInput> {
        vec![
            word("r1", " Heb", 1_000, 1_500, 1),
            word("r2", " ik", 1_600, 1_900, 1),
            word("r3", " gekeken,", 2_000, 2_500, 1),
            word("r4", " ook", 3_000, 3_500, 1),
            word("m1", " hebben.", 4_000, 4_400, 0),
            word("r5", " buitenland,", 4_500, 5_000, 1),
            word("r6", " in", 5_500, 6_000, 1),
            word("r7", " Engeland.", 6_500, 7_000, 1),
        ]
    }

    #[test]
    fn word_level_interleave_splits_sentences_without_synthetic_timing() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: cross_talk_words(),
                assignments: vec![],
                synthetic_timing: None,
            }],
            participant_human_ids: vec![],
            self_human_id: None,
            humans: vec![],
        });

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[1].text, "hebben.");
    }

    #[test]
    fn keeps_sentences_whole_for_synthetic_timing() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: cross_talk_words(),
                assignments: vec![],
                synthetic_timing: Some(true),
            }],
            participant_human_ids: vec![],
            self_human_id: None,
            humans: vec![],
        });

        assert_eq!(segments.len(), 2);
        assert_eq!(
            segments[0].text,
            "Heb ik gekeken, ook buitenland, in Engeland."
        );
        assert_eq!(segments[1].text, "hebben.");
    }

    #[test]
    fn synthetic_timing_closes_sentence_units_on_long_silence() {
        let segments = render_transcript_segments(RenderTranscriptRequest {
            transcripts: vec![RenderTranscriptInput {
                started_at: Some(0),
                words: vec![
                    word("r1", " we", 0, 400, 1),
                    word("r2", " were", 500, 900, 1),
                    word("r3", " talking", 1_000, 1_600, 1),
                    word("m1", " that's", 10_000, 10_300, 0),
                    word("m2", " a", 10_400, 10_500, 0),
                    word("m3", " reply.", 10_600, 11_000, 0),
                    word("r4", " and", 20_000, 20_300, 1),
                    word("r5", " then", 20_400, 20_700, 1),
                    word("r6", " continued", 20_800, 21_200, 1),
                ],
                assignments: vec![],
                synthetic_timing: Some(true),
            }],
            participant_human_ids: vec![],
            self_human_id: None,
            humans: vec![],
        });

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].text, "we were talking");
        assert_eq!(segments[1].text, "that's a reply.");
        assert_eq!(segments[2].text, "and then continued");
    }
}

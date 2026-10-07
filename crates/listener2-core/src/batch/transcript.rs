//! Port of the frontend's `batch::Response` -> transcript mapping
//! (`apps/desktop/src/stt/useRunBatch.ts` + `store/zustand/listener/{batch,utils}.ts`),
//! so a headless caller can persist a transcript that the desktop app renders
//! identically. Field-for-field fidelity with the TypeScript pipeline is the
//! contract here — timing metadata shape, word-spacing scheme, speaker-hint
//! value encoding, and id-assignment order all mirror the frontend.

use hypr_fs_format::{TranscriptSpeakerHint, TranscriptWithData, TranscriptWord};
use owhisper_interface::batch;
use serde_json::{Map, Value};

const SYNTHETIC_TEXT_WORD_SECONDS: f64 = 0.4;
const MIN_SYNTHETIC_TEXT_WORD_SECONDS: f64 = 0.05;

const TIMING_PROVIDER_WORD: &str = "provider_word";
const TIMING_PROVIDER_SEGMENT_INTERPOLATED: &str = "provider_segment_interpolated";
const TIMING_SYNTHETIC_SPEECH: &str = "synthetic_speech";
const TIMING_SYNTHETIC_TEXT: &str = "synthetic_text";

pub const PROVIDER_SPEAKER_INDEX_HINT: &str = "provider_speaker_index";

/// A speaker hint still tied to a word by position; ids are assigned later
/// (mirrors the TS split between `transformBatch` and the persist callback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchWordHint {
    pub word_index: usize,
    pub speaker_index: usize,
}

/// The fields the persist callback added around the mapped words in TS
/// (`createTranscript` in `apps/desktop/src/stt/queries.ts`).
pub struct BatchTranscriptMeta<'a> {
    pub session_id: &'a str,
    pub user_id: &'a str,
    /// Batch provider identifier embedded in speaker-hint values ("soniqo", ...).
    pub provider: &'a str,
    /// RFC3339 timestamp, the desktop writes `new Date().toISOString()`.
    pub created_at: String,
    /// Epoch milliseconds, the desktop writes `Date.now()`.
    pub started_at_ms: f64,
    pub memo_md: String,
}

/// Port of `transformBatch`: flatten every channel's first alternative into
/// words (ids unassigned) plus positional speaker hints.
pub fn words_and_hints_from_batch_response(
    response: &batch::Response,
) -> (Vec<TranscriptWord>, Vec<BatchWordHint>) {
    let audio = response
        .metadata
        .get("session_audio")
        .cloned()
        .and_then(|value| serde_json::from_value::<hypr_fs_format::SessionAudio>(value).ok());
    let mut all_words = Vec::new();
    let mut all_hints = Vec::new();

    for (channel_index, channel) in response.results.channels.iter().enumerate() {
        let Some(alternative) = channel.alternatives.first() else {
            continue;
        };

        let timing_source = word_timing_source(
            &response.metadata,
            !alternative.words.is_empty(),
            TIMING_SYNTHETIC_TEXT,
        );
        let entries = word_entries_from_transcript(
            &alternative.words,
            &alternative.transcript,
            if audio.is_some_and(|a| a.layout == hypr_fs_format::AudioLayout::Mixed) {
                2
            } else {
                channel_index as i32
            },
            batch_duration_seconds(&response.metadata),
            timing_source,
        );

        let word_offset = all_words.len();
        let (mut words, hints) =
            transform_word_entries(&entries, &alternative.transcript, timing_source);

        if let Some(audio) = audio {
            for word in &mut words {
                if audio.layout == hypr_fs_format::AudioLayout::Mixed {
                    word.channel = 2.0;
                }
                word.metadata.get_or_insert_with(Map::new).insert(
                    "capture_source".into(),
                    serde_json::to_value(audio.source).unwrap(),
                );
            }
        }

        all_hints.extend(hints.into_iter().map(|hint| BatchWordHint {
            word_index: hint.word_index + word_offset,
            speaker_index: hint.speaker_index,
        }));
        all_words.extend(words);
    }

    (all_words, all_hints)
}

/// Port of the persist path (`useRunBatch`'s default persist callback plus
/// `createTranscript`): assign ids, encode speaker hints, and wrap everything
/// into the `TranscriptWithData` the store persists. Returns `None` when the
/// response holds no words — the desktop persists nothing in that case.
///
/// `new_id` mirrors the frontend's `crypto.randomUUID()`; injectable so tests
/// can assert exact output. Assignment order matches TS: word ids first, then
/// hint ids, then the transcript id.
pub fn transcript_from_batch_response(
    response: &batch::Response,
    meta: BatchTranscriptMeta<'_>,
    new_id: &mut dyn FnMut() -> String,
) -> Option<TranscriptWithData> {
    let (mut words, hints) = words_and_hints_from_batch_response(response);
    if words.is_empty() {
        return None;
    }

    let mut word_ids = Vec::with_capacity(words.len());
    for word in &mut words {
        let id = new_id();
        word_ids.push(id.clone());
        word.id = Some(id);
    }

    let speaker_hints = hints
        .iter()
        .map(|hint| TranscriptSpeakerHint {
            id: Some(new_id()),
            word_id: word_ids[hint.word_index].clone(),
            hint_type: PROVIDER_SPEAKER_INDEX_HINT.to_string(),
            // The desktop stores the JSON.stringify'd object as a *string*
            // value (see `toTranscriptSpeakerHint`), so the on-disk hint value
            // is a JSON-encoded string, not a nested object.
            value: Value::String(speaker_hint_value(
                meta.provider,
                words[hint.word_index].channel as i64,
                hint.speaker_index,
            )),
        })
        .collect();

    Some(TranscriptWithData {
        id: new_id(),
        user_id: meta.user_id.to_string(),
        created_at: meta.created_at,
        session_id: meta.session_id.to_string(),
        started_at: meta.started_at_ms,
        ended_at: None,
        memo_md: meta.memo_md,
        words,
        speaker_hints,
    })
}

#[derive(Debug, Clone)]
struct WordEntry {
    word: String,
    punctuated_word: Option<String>,
    start: f64,
    end: f64,
    channel: i32,
    speaker: Option<usize>,
}

struct LocalHint {
    word_index: usize,
    speaker_index: usize,
}

/// Port of `wordEntriesFromTranscript` for the direct-response path
/// (`startSeconds` is always 0 there): keep provider words as-is, otherwise
/// synthesize evenly spaced words from the transcript text.
fn word_entries_from_transcript(
    words: &[batch::Word],
    transcript: &str,
    channel: i32,
    duration_seconds: Option<f64>,
    timing_source: &'static str,
) -> Vec<WordEntry> {
    if !words.is_empty() {
        return words
            .iter()
            .map(|word| WordEntry {
                word: word.word.clone(),
                punctuated_word: word.punctuated_word.clone(),
                start: word.start,
                end: word.end,
                channel: word.channel,
                speaker: word.speaker,
            })
            .collect();
    }

    if transcript.trim().is_empty() {
        return Vec::new();
    }

    let tokens: Vec<&str> = transcript.split_whitespace().collect();
    if tokens.is_empty() {
        return Vec::new();
    }

    let count = tokens.len() as f64;
    let duration = if timing_source == TIMING_SYNTHETIC_TEXT {
        count * SYNTHETIC_TEXT_WORD_SECONDS
    } else {
        duration_seconds
            .filter(|duration| duration.is_finite())
            .unwrap_or(count * SYNTHETIC_TEXT_WORD_SECONDS)
            .max(count * MIN_SYNTHETIC_TEXT_WORD_SECONDS)
    };

    tokens
        .iter()
        .enumerate()
        .map(|(index, token)| WordEntry {
            word: (*token).to_string(),
            punctuated_word: Some((*token).to_string()),
            start: (index as f64 / count) * duration,
            end: ((index + 1) as f64 / count) * duration,
            channel,
            speaker: None,
        })
        .collect()
}

/// Port of `transformWordEntries`: spacing-fixed text, millisecond rounding,
/// timing metadata, and positional speaker hints.
fn transform_word_entries(
    entries: &[WordEntry],
    transcript: &str,
    timing_source: &'static str,
) -> (Vec<TranscriptWord>, Vec<LocalHint>) {
    let raw_texts: Vec<&str> = entries
        .iter()
        .map(|entry| entry.punctuated_word.as_deref().unwrap_or(&entry.word))
        .collect();
    let texts = fix_spacing_for_words(&raw_texts, transcript);

    let mut words = Vec::with_capacity(entries.len());
    let mut hints = Vec::new();

    for (index, entry) in entries.iter().enumerate() {
        words.push(TranscriptWord {
            id: None,
            text: texts[index].clone(),
            start_ms: (entry.start * 1000.0).round(),
            end_ms: (entry.end * 1000.0).round(),
            channel: entry.channel as f64,
            speaker: None,
            metadata: Some(timing_metadata(timing_source)),
        });

        if let Some(speaker_index) = entry.speaker {
            hints.push(LocalHint {
                word_index: index,
                speaker_index,
            });
        }
    }

    (words, hints)
}

/// Port of `fixSpacingForWords`: every word carries its leading whitespace from
/// the transcript (the first word gets a single leading space).
fn fix_spacing_for_words(words: &[&str], transcript: &str) -> Vec<String> {
    let mut result = Vec::with_capacity(words.len());
    let mut pos = 0usize;

    for (index, word) in words.iter().enumerate() {
        let trimmed = word.trim();

        if trimmed.is_empty() {
            result.push((*word).to_string());
            continue;
        }

        match transcript[pos..].find(trimmed) {
            None => result.push((*word).to_string()),
            Some(relative) => {
                let found_at = pos + relative;
                let prefix = if index == 0 {
                    " "
                } else {
                    &transcript[pos..found_at]
                };
                result.push(format!("{prefix}{trimmed}"));
                pos = found_at + trimmed.len();
            }
        }
    }

    result
}

/// Port of `createTranscriptTimingMetadata` for the batch path, where the
/// incoming word metadata is always absent (`batch::Word` has no metadata
/// field): the result is always `{"timing":{"source":<source>}}`.
fn timing_metadata(source: &'static str) -> Map<String, Value> {
    let mut timing = Map::new();
    timing.insert("source".to_string(), Value::String(source.to_string()));
    let mut metadata = Map::new();
    metadata.insert("timing".to_string(), Value::Object(timing));
    metadata
}

/// Port of `getWordTimingSourceForBatchResponse`.
fn word_timing_source(
    metadata: &Value,
    has_provider_words: bool,
    fallback_without_words: &'static str,
) -> &'static str {
    if !has_provider_words {
        return fallback_without_words;
    }

    explicit_timing_source(metadata).unwrap_or(TIMING_PROVIDER_WORD)
}

/// Port of `getBatchResponseTimingSource`: when `metadata.timing` is an object,
/// only `timing.source` is considered; `timing_source` is the fallback key.
fn explicit_timing_source(metadata: &Value) -> Option<&'static str> {
    let record = metadata.as_object()?;
    if let Some(timing) = record.get("timing").and_then(Value::as_object) {
        return valid_timing_source(timing.get("source"));
    }
    valid_timing_source(record.get("timing_source"))
}

fn valid_timing_source(value: Option<&Value>) -> Option<&'static str> {
    match value?.as_str()? {
        "provider_word" => Some(TIMING_PROVIDER_WORD),
        "provider_segment_interpolated" => Some(TIMING_PROVIDER_SEGMENT_INTERPOLATED),
        "synthetic_speech" => Some(TIMING_SYNTHETIC_SPEECH),
        "synthetic_text" => Some(TIMING_SYNTHETIC_TEXT),
        _ => None,
    }
}

/// Port of `getBatchDurationSeconds`.
fn batch_duration_seconds(metadata: &Value) -> Option<f64> {
    metadata
        .as_object()?
        .get("duration")?
        .as_f64()
        .filter(|duration| duration.is_finite() && *duration > 0.0)
}

/// Matches `JSON.stringify({ provider, channel, speaker_index })` byte for byte
/// (compact separators, same key order).
fn speaker_hint_value(provider: &str, channel: i64, speaker_index: usize) -> String {
    #[derive(serde::Serialize)]
    struct SpeakerHintValue<'a> {
        provider: &'a str,
        channel: i64,
        speaker_index: usize,
    }

    serde_json::to_string(&SpeakerHintValue {
        provider,
        channel,
        speaker_index,
    })
    .expect("speaker hint value is always serializable")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_word(
        word: &str,
        punctuated: &str,
        start: f64,
        end: f64,
        channel: i32,
        speaker: Option<usize>,
    ) -> batch::Word {
        batch::Word {
            word: word.to_string(),
            start,
            end,
            confidence: 1.0,
            channel,
            speaker,
            punctuated_word: Some(punctuated.to_string()),
        }
    }

    fn response_with_channels(metadata: Value, channels: Vec<batch::Channel>) -> batch::Response {
        batch::Response {
            metadata,
            results: batch::Results { channels },
        }
    }

    fn channel(transcript: &str, words: Vec<batch::Word>) -> batch::Channel {
        batch::Channel {
            alternatives: vec![batch::Alternatives {
                transcript: transcript.to_string(),
                confidence: 1.0,
                words,
            }],
        }
    }

    fn timing(source: &str) -> Map<String, Value> {
        serde_json::from_value(serde_json::json!({ "timing": { "source": source } })).unwrap()
    }

    fn counter_ids() -> impl FnMut() -> String {
        let mut next = 0usize;
        move || {
            let id = format!("id-{next}");
            next += 1;
            id
        }
    }

    /// Mirrors the desktop's `fixSpacingForWords` test table
    /// (`store/zustand/listener/utils.test.ts`) verbatim.
    #[test]
    fn fix_spacing_matches_the_frontend_test_table() {
        let cases: [(&str, &[&str], &[&str]); 4] = [
            ("Hello", &["Hello"], &[" Hello"]),
            (
                "Yes. Because we",
                &["Yes.", "Because", "we"],
                &[" Yes.", " Because", " we"],
            ),
            ("shouldn't", &["shouldn", "'t"], &[" shouldn", "'t"]),
            (
                "Yes. Because we shouldn't be false.",
                &["Yes.", "Because", "we", "shouldn", "'t", "be", "false."],
                &[
                    " Yes.", " Because", " we", " shouldn", "'t", " be", " false.",
                ],
            ),
        ];

        for (transcript, input, output) in cases {
            let actual = fix_spacing_for_words(input, transcript);
            let expected: Vec<String> = output.iter().map(|s| (*s).to_string()).collect();
            assert_eq!(actual, expected, "transcript: {transcript}");
        }
    }

    #[test]
    fn provider_words_map_with_spacing_timing_and_hints() {
        let response = response_with_channels(
            serde_json::json!({}),
            vec![
                channel(
                    "Hello, world.",
                    vec![
                        provider_word("hello", "Hello,", 0.0, 0.48, 0, Some(0)),
                        provider_word("world", "world.", 0.5, 1.0, 0, Some(1)),
                    ],
                ),
                channel("Hi", vec![provider_word("hi", "Hi", 0.2, 0.4, 1, None)]),
            ],
        );

        let (words, hints) = words_and_hints_from_batch_response(&response);

        assert_eq!(
            words,
            vec![
                TranscriptWord {
                    id: None,
                    text: " Hello,".to_string(),
                    start_ms: 0.0,
                    end_ms: 480.0,
                    channel: 0.0,
                    speaker: None,
                    metadata: Some(timing("provider_word")),
                },
                TranscriptWord {
                    id: None,
                    text: " world.".to_string(),
                    start_ms: 500.0,
                    end_ms: 1000.0,
                    channel: 0.0,
                    speaker: None,
                    metadata: Some(timing("provider_word")),
                },
                TranscriptWord {
                    id: None,
                    text: " Hi".to_string(),
                    start_ms: 200.0,
                    end_ms: 400.0,
                    channel: 1.0,
                    speaker: None,
                    metadata: Some(timing("provider_word")),
                },
            ]
        );
        assert_eq!(
            hints,
            vec![
                BatchWordHint {
                    word_index: 0,
                    speaker_index: 0,
                },
                BatchWordHint {
                    word_index: 1,
                    speaker_index: 1,
                },
            ]
        );
    }

    #[test]
    fn transcript_from_batch_response_matches_the_frontend_persist_shape() {
        let response = response_with_channels(
            serde_json::json!({}),
            vec![channel(
                "Hello, world.",
                vec![
                    provider_word("hello", "Hello,", 0.0, 0.48, 0, Some(0)),
                    provider_word("world", "world.", 0.5, 1.0, 0, Some(1)),
                ],
            )],
        );

        let mut ids = counter_ids();
        let transcript = transcript_from_batch_response(
            &response,
            BatchTranscriptMeta {
                session_id: "session-1",
                user_id: "00000000-0000-0000-0000-000000000000",
                provider: "soniqo",
                created_at: "2026-08-09T00:00:00.000Z".to_string(),
                started_at_ms: 1_700_000_000_000.0,
                memo_md: String::new(),
            },
            &mut ids,
        )
        .unwrap();

        assert_eq!(
            transcript,
            TranscriptWithData {
                id: "id-4".to_string(),
                user_id: "00000000-0000-0000-0000-000000000000".to_string(),
                created_at: "2026-08-09T00:00:00.000Z".to_string(),
                session_id: "session-1".to_string(),
                started_at: 1_700_000_000_000.0,
                ended_at: None,
                memo_md: String::new(),
                words: vec![
                    TranscriptWord {
                        id: Some("id-0".to_string()),
                        text: " Hello,".to_string(),
                        start_ms: 0.0,
                        end_ms: 480.0,
                        channel: 0.0,
                        speaker: None,
                        metadata: Some(timing("provider_word")),
                    },
                    TranscriptWord {
                        id: Some("id-1".to_string()),
                        text: " world.".to_string(),
                        start_ms: 500.0,
                        end_ms: 1000.0,
                        channel: 0.0,
                        speaker: None,
                        metadata: Some(timing("provider_word")),
                    },
                ],
                speaker_hints: vec![
                    TranscriptSpeakerHint {
                        id: Some("id-2".to_string()),
                        word_id: "id-0".to_string(),
                        hint_type: "provider_speaker_index".to_string(),
                        value: Value::String(
                            "{\"provider\":\"soniqo\",\"channel\":0,\"speaker_index\":0}"
                                .to_string()
                        ),
                    },
                    TranscriptSpeakerHint {
                        id: Some("id-3".to_string()),
                        word_id: "id-1".to_string(),
                        hint_type: "provider_speaker_index".to_string(),
                        value: Value::String(
                            "{\"provider\":\"soniqo\",\"channel\":0,\"speaker_index\":1}"
                                .to_string()
                        ),
                    },
                ],
            }
        );
    }

    #[test]
    fn transcript_only_channels_synthesize_text_timed_words() {
        // No provider words: timing falls back to synthetic_text and duration
        // is word_count * 0.4s regardless of metadata.duration.
        let response = response_with_channels(
            serde_json::json!({ "duration": 120.0 }),
            vec![channel("eins zwei", vec![])],
        );

        let (words, hints) = words_and_hints_from_batch_response(&response);

        assert!(hints.is_empty());
        assert_eq!(
            words,
            vec![
                TranscriptWord {
                    id: None,
                    text: " eins".to_string(),
                    start_ms: 0.0,
                    end_ms: 400.0,
                    channel: 0.0,
                    speaker: None,
                    metadata: Some(timing("synthetic_text")),
                },
                TranscriptWord {
                    id: None,
                    text: " zwei".to_string(),
                    start_ms: 400.0,
                    end_ms: 800.0,
                    channel: 0.0,
                    speaker: None,
                    metadata: Some(timing("synthetic_text")),
                },
            ]
        );
    }

    #[test]
    fn explicit_metadata_timing_source_overrides_provider_word() {
        let response = response_with_channels(
            serde_json::json!({ "timing_source": "synthetic_speech" }),
            vec![channel(
                "hello",
                vec![provider_word("hello", "hello", 1.0, 2.0, 0, None)],
            )],
        );

        let (words, _) = words_and_hints_from_batch_response(&response);
        assert_eq!(words[0].metadata, Some(timing("synthetic_speech")));

        // `metadata.timing.source` wins over `timing_source`, and an invalid
        // nested source does NOT fall back to the flat key.
        let nested = response_with_channels(
            serde_json::json!({
                "timing": { "source": "synthetic_text" },
                "timing_source": "synthetic_speech",
            }),
            vec![channel(
                "hello",
                vec![provider_word("hello", "hello", 1.0, 2.0, 0, None)],
            )],
        );
        let (words, _) = words_and_hints_from_batch_response(&nested);
        assert_eq!(words[0].metadata, Some(timing("synthetic_text")));

        let invalid_nested = response_with_channels(
            serde_json::json!({
                "timing": { "source": "bogus" },
                "timing_source": "synthetic_speech",
            }),
            vec![channel(
                "hello",
                vec![provider_word("hello", "hello", 1.0, 2.0, 0, None)],
            )],
        );
        let (words, _) = words_and_hints_from_batch_response(&invalid_nested);
        assert_eq!(words[0].metadata, Some(timing("provider_word")));
    }

    #[test]
    fn empty_response_produces_no_transcript() {
        let empty = response_with_channels(serde_json::json!({}), vec![channel("", vec![])]);
        let mut ids = counter_ids();
        assert!(
            transcript_from_batch_response(
                &empty,
                BatchTranscriptMeta {
                    session_id: "session-1",
                    user_id: "",
                    provider: "soniqo",
                    created_at: String::new(),
                    started_at_ms: 0.0,
                    memo_md: String::new(),
                },
                &mut ids,
            )
            .is_none()
        );
    }

    /// End-to-end against the real soniqo response builder: the words the
    /// desktop would persist for a soniqo batch result.
    #[test]
    fn soniqo_batch_response_maps_like_the_desktop() {
        let response = hypr_transcribe_soniqo::batch_response_from_text(
            hypr_transcribe_soniqo::SoniqoModel::ParakeetBatch,
            "hello world".to_string(),
            2.0,
        );

        let (words, hints) = words_and_hints_from_batch_response(&response);

        assert!(hints.is_empty());
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, " hello");
        assert_eq!(words[1].text, " world");
        assert_eq!(words[0].start_ms, 0.0);
        assert_eq!(words[0].end_ms, 400.0);
        assert_eq!(words[1].end_ms, 800.0);
        // soniqo stamps metadata.timing_source, which flows into every word.
        assert_eq!(words[0].metadata, Some(timing("synthetic_text")));
    }
}

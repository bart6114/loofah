use std::collections::HashMap;
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta::Type;

use hypr_vault_read::{SessionMeta, TranscriptWithData};

use crate::render::object_hint_value;
use crate::{Error, Pagination, Result, occurred_at, pagination, run_blocking};

pub const DEFAULT_SEARCH_LIMIT: u32 = 20;
pub const MAX_SEARCH_LIMIT: u32 = 50;
const SNIPPET_RADIUS_CHARS: usize = 100;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Type)]
#[serde(rename_all = "snake_case")]
pub struct SearchMeetingsInput {
    #[schemars(
        description = "Search words and quoted phrases, with a trailing word prefix; results are ranked by relevance, once per session. Required unless speaker is set"
    )]
    pub query: Option<String>,
    #[schemars(
        description = "Case-insensitive person id or name substring; limits results to meetings where that person spoke. The query matches anywhere in those transcripts; without query, lists meetings where they spoke"
    )]
    pub speaker: Option<String>,
    #[schemars(
        description = "Sources to search (title, note, summary, transcript); defaults to all. Ignored when speaker is set: speaker search is transcript-only"
    )]
    pub kinds: Option<Vec<SearchKind>>,
    #[schemars(description = "Maximum hits; defaults to 20 and is capped at 50")]
    #[schemars(range(min = 1, max = 50))]
    pub limit: Option<u32>,
    #[schemars(description = "Number of hits to skip; defaults to 0")]
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Type)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    Title,
    Note,
    Summary,
    Transcript,
}

const ALL_KINDS: [SearchKind; 4] = [
    SearchKind::Title,
    SearchKind::Note,
    SearchKind::Summary,
    SearchKind::Transcript,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct SearchHit {
    pub meeting_id: String,
    pub meeting_title: String,
    pub occurred_at: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_title: Option<String>,
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub pagination: Pagination,
}

pub async fn search_meetings(vault: &Path, input: SearchMeetingsInput) -> Result<SearchPage> {
    run_blocking("search meetings", vault, move |vault| {
        search_meetings_sync(vault, input)
    })
    .await
}

struct SearchContext {
    query: String,
    speaker: Option<String>,
    kinds: Vec<SearchKind>,
    people_names: HashMap<String, String>,
}

impl SearchContext {
    fn wants(&self, kind: SearchKind) -> bool {
        self.kinds.contains(&kind)
    }

    fn speaker_matches(&self, person_id: &str) -> bool {
        let Some(filter) = &self.speaker else {
            return true;
        };
        person_id.to_lowercase().contains(filter)
            || self
                .people_names
                .get(person_id)
                .is_some_and(|name| name.to_lowercase().contains(filter))
    }

    fn display_name(&self, person_id: &str) -> String {
        match self.people_names.get(person_id) {
            Some(name) if !name.trim().is_empty() => name.clone(),
            _ => person_id.to_string(),
        }
    }
}

fn search_meetings_sync(vault: &Path, input: SearchMeetingsInput) -> Result<SearchPage> {
    let terms = input
        .query
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    let speaker = input
        .speaker
        .as_deref()
        .map(str::trim)
        .filter(|speaker| !speaker.is_empty())
        .map(str::to_lowercase);
    if terms.is_empty() && speaker.is_none() {
        return Err(Error::InvalidInput(
            "search requires query and/or speaker".to_string(),
        ));
    }

    let limit = input
        .limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT);
    let offset = input.offset.unwrap_or(0);
    let needed = offset as usize + limit as usize + 1;

    let cache = crate::open_search_cache(vault)?;
    let ranked = cache
        .search(hypr_search_cache::SearchRequest {
            query: input.query.clone().unwrap_or_default(),
            collection: None,
            filters: Default::default(),
            limit: cache.headers().len().max(1),
            options: hypr_search_cache::SearchOptions {
                snippets: Some(true),
                ..Default::default()
            },
        })
        .map_err(|error| Error::Vault {
            action: "search meetings",
            reason: error.to_string(),
        })?;
    let filtered = speaker.is_some() || input.kinds.as_ref().is_some_and(|k| !k.is_empty());

    // A speaker filter only makes sense against transcript words, so it narrows the
    // search to transcripts no matter which kinds were requested.
    let kinds = if speaker.is_some() {
        vec![SearchKind::Transcript]
    } else {
        input
            .kinds
            .filter(|kinds| !kinds.is_empty())
            .unwrap_or_else(|| ALL_KINDS.to_vec())
    };
    let ctx = SearchContext {
        query: input.query.clone().unwrap_or_default(),
        speaker,
        kinds,
        people_names: hypr_vault_read::read_people(vault)
            .into_iter()
            .map(|person| (person.id, person.name))
            .collect(),
    };

    let mut hits = Vec::new();
    for ranked in ranked.hits {
        let Some(session) = cache
            .session(&ranked.document.id)
            .map_err(|error| Error::Vault {
                action: "search meetings",
                reason: error.to_string(),
            })?
        else {
            continue;
        };
        let mut sources = Vec::new();
        collect_session_hits(&session, &ctx, &mut sources);
        if let Some(hit) = sources.into_iter().next() {
            hits.push(hit);
        } else if !filtered {
            let mut hit = base_hit(&session.meta, "session");
            hit.snippet = ranked
                .content_snippet
                .map(|s| s.fragment)
                .filter(|s| !s.is_empty())
                .unwrap_or(ranked.document.title);
            hits.push(hit);
        }
        if hits.len() >= needed {
            break;
        }
    }

    let has_more = hits.len() > offset as usize + limit as usize;
    let hits = hits
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect::<Vec<_>>();
    Ok(SearchPage {
        pagination: pagination(offset, limit, hits.len(), None, has_more),
        hits,
    })
}

fn collect_session_hits(
    session: &hypr_search_cache::CachedSession,
    ctx: &SearchContext,
    hits: &mut Vec<SearchHit>,
) {
    let meta = &session.meta;
    if ctx.wants(SearchKind::Title)
        && let Some((start, end)) = hypr_search_cache::matching_range(&meta.title, &ctx.query)
    {
        let mut hit = base_hit(meta, "title");
        hit.snippet = make_snippet(&meta.title, start, end);
        hits.push(hit);
    }

    if ctx.wants(SearchKind::Note)
        && let Some(markdown) = session.note.as_ref()
        && let Some((start, end)) = hypr_search_cache::matching_range(markdown, &ctx.query)
    {
        let mut hit = base_hit(meta, "note");
        hit.snippet = make_snippet(markdown, start, end);
        hits.push(hit);
    }

    if ctx.wants(SearchKind::Summary) {
        for doc in &session.docs {
            let haystack = if doc.title.trim().is_empty() {
                doc.markdown.clone()
            } else {
                format!("{}\n{}", doc.title, doc.markdown)
            };
            let Some((start, end)) = hypr_search_cache::matching_range(&haystack, &ctx.query)
            else {
                continue;
            };
            let mut hit = base_hit(meta, "summary");
            hit.snippet = make_snippet(&haystack, start, end);
            hit.document_id = Some(doc.id.clone());
            if !doc.title.trim().is_empty() {
                hit.document_title = Some(doc.title.clone());
            }
            hits.push(hit);
        }
    }

    if ctx.wants(SearchKind::Transcript) {
        collect_transcript_hits(session, ctx, hits);
    }
}

struct WordRecord {
    start_ms: i64,
    speaker_id: Option<String>,
    text: String,
}

fn collect_transcript_hits(
    session: &hypr_search_cache::CachedSession,
    ctx: &SearchContext,
    hits: &mut Vec<SearchHit>,
) {
    let meta = &session.meta;
    let mut transcripts = session.transcripts.clone();
    // Same ordering as get_meeting_transcript's flattening, so global_offset is
    // directly usable as its `offset` input.
    transcripts.sort_by(|a, b| {
        (a.started_at.round() as i64, &a.id).cmp(&(b.started_at.round() as i64, &b.id))
    });

    let mut records = Vec::new();
    for transcript in &transcripts {
        let speaker_ids = attribute_speakers(transcript);
        for (index, word) in transcript.words.iter().enumerate() {
            records.push(WordRecord {
                start_ms: word.start_ms.round() as i64,
                speaker_id: speaker_ids.get(index).cloned().flatten(),
                text: word.text.trim().to_string(),
            });
        }
    }

    // The speaker filter is a presence gate: the meeting qualifies when any word is
    // attributed to a matching person (words no hint attributes to anyone never match),
    // and the query then searches the whole transcript, whoever said the matched word.
    let speaker_words = records
        .iter()
        .filter(|record| !record.text.is_empty())
        .filter(|record| {
            record
                .speaker_id
                .as_deref()
                .is_some_and(|person_id| ctx.speaker_matches(person_id))
        })
        .collect::<Vec<_>>();
    if ctx.speaker.is_some() && speaker_words.is_empty() {
        return;
    }

    if ctx.query.trim().is_empty() {
        let mut snippet = String::new();
        let mut chars = 0usize;
        let mut truncated = false;
        for record in &speaker_words {
            if !snippet.is_empty() {
                snippet.push(' ');
                chars += 1;
            }
            snippet.push_str(&record.text);
            chars += record.text.chars().count();
            if chars >= 2 * SNIPPET_RADIUS_CHARS {
                truncated = true;
                break;
            }
        }
        if truncated {
            snippet.push('…');
        }
        push_transcript_hit(hits, meta, ctx, speaker_words[0], snippet);
        return;
    }

    let included = records
        .iter()
        .filter(|record| !record.text.is_empty())
        .collect::<Vec<_>>();
    if included.is_empty() {
        return;
    }

    let mut haystack = String::new();
    let mut word_starts = Vec::new();
    for (index, record) in included.iter().enumerate() {
        if !haystack.is_empty() {
            haystack.push(' ');
        }
        word_starts.push((haystack.len(), index));
        haystack.push_str(&record.text);
    }

    let Some((start, end)) = hypr_search_cache::matching_range(&haystack, &ctx.query) else {
        return;
    };
    let word_index = match word_starts.binary_search_by(|(word_start, _)| word_start.cmp(&start)) {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    };
    let snippet = make_snippet(&haystack, start, end);
    push_transcript_hit(
        hits,
        meta,
        ctx,
        included[word_starts[word_index].1],
        snippet,
    );
}

fn push_transcript_hit(
    hits: &mut Vec<SearchHit>,
    meta: &SessionMeta,
    ctx: &SearchContext,
    record: &WordRecord,
    snippet: String,
) {
    let mut hit = base_hit(meta, "transcript");
    hit.snippet = snippet;
    hit.start_ms = Some(record.start_ms);
    if let Some(person_id) = &record.speaker_id {
        hit.speaker = Some(ctx.display_name(person_id));
        hit.speaker_id = Some(person_id.clone());
    }
    hits.push(hit);
}

fn base_hit(meta: &SessionMeta, kind: &str) -> SearchHit {
    SearchHit {
        meeting_id: meta.id.clone(),
        meeting_title: meta.title.clone(),
        occurred_at: occurred_at(meta).to_string(),
        kind: kind.to_string(),
        document_id: None,
        document_title: None,
        snippet: String::new(),
        speaker: None,
        speaker_id: None,
        start_ms: None,
    }
}

/// Mirrors the desktop's two-pass hint normalization (`render-transcript.ts`):
/// provider hints stamp per-word speaker indexes first, then label hints bind a person
/// id to the anchored word's (channel, speaker_index) — or to the whole channel when
/// the anchored word carries no index.
fn attribute_speakers(transcript: &TranscriptWithData) -> Vec<Option<String>> {
    let word_count = transcript.words.len();
    let mut index_by_word_id = HashMap::new();
    for (index, word) in transcript.words.iter().enumerate() {
        if let Some(word_id) = word.id.as_deref() {
            index_by_word_id.insert(word_id, index);
        }
    }

    let mut channels = transcript
        .words
        .iter()
        .map(|word| word.channel.round() as i64)
        .collect::<Vec<_>>();
    let mut speaker_indexes = vec![None::<i64>; word_count];
    for hint in &transcript.speaker_hints {
        if hint.hint_type != "provider_speaker_index" {
            continue;
        }
        let Some(&index) = index_by_word_id.get(hint.word_id.as_str()) else {
            continue;
        };
        let Some(value) = object_hint_value(&hint.value) else {
            continue;
        };
        let Some(speaker_index) = value.get("speaker_index").and_then(Value::as_f64) else {
            continue;
        };
        speaker_indexes[index] = Some(speaker_index.round() as i64);
        if let Some(channel) = value.get("channel").and_then(Value::as_f64) {
            channels[index] = channel.round() as i64;
        }
    }

    let mut by_scoped_speaker = HashMap::new();
    let mut by_channel = HashMap::new();
    for hint in &transcript.speaker_hints {
        if hint.hint_type != "speaker_label" {
            continue;
        }
        let Some(&index) = index_by_word_id.get(hint.word_id.as_str()) else {
            continue;
        };
        let Some(label) = hint.value.as_str().filter(|label| !label.is_empty()) else {
            continue;
        };
        match speaker_indexes[index] {
            Some(speaker_index) => {
                by_scoped_speaker.insert((channels[index], speaker_index), label.to_string());
            }
            None => {
                by_channel.insert(channels[index], label.to_string());
            }
        }
    }

    (0..word_count)
        .map(|index| {
            if let Some(speaker_index) = speaker_indexes[index]
                && let Some(person_id) = by_scoped_speaker.get(&(channels[index], speaker_index))
            {
                return Some(person_id.clone());
            }
            by_channel.get(&channels[index]).cloned()
        })
        .collect()
}

/// ~100 characters of context on each side of the match, cut on character boundaries,
/// whitespace collapsed, with `…` marking truncation.
fn make_snippet(text: &str, start: usize, end: usize) -> String {
    let snippet_start = text[..start]
        .char_indices()
        .rev()
        .map(|(index, _)| index)
        .nth(SNIPPET_RADIUS_CHARS - 1)
        .unwrap_or(0);
    let snippet_end = text[end..]
        .char_indices()
        .map(|(index, _)| index)
        .nth(SNIPPET_RADIUS_CHARS)
        .map(|index| end + index)
        .unwrap_or(text.len());
    let mut fragment = text[snippet_start..snippet_end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if snippet_start > 0 {
        fragment.insert(0, '…');
    }
    if snippet_end < text.len() {
        fragment.push('…');
    }
    fragment
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GetMeetingTranscriptInput;

    fn seed_session(vault: &Path, id: &str, title: &str, started_at: &str) {
        let dir = vault.join(format!("sessions/{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("_meta.json"),
            serde_json::json!({
                "id": id,
                "title": title,
                "started_at": started_at,
                "ended_at": null,
                "created_at": "2026-07-01T00:00:00Z",
                "tags": [],
            })
            .to_string(),
        )
        .unwrap();
    }

    fn write_word(id: &str, text: &str, index: u32, channel: f64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "text": text,
            "start_ms": f64::from(index) * 1000.0,
            "end_ms": f64::from(index) * 1000.0 + 500.0,
            "channel": channel,
        })
    }

    fn write_transcript(
        vault: &Path,
        id: &str,
        words: Vec<serde_json::Value>,
        hints: Vec<serde_json::Value>,
    ) {
        std::fs::write(
            vault.join(format!("sessions/{id}/transcript.json")),
            serde_json::json!({
                "transcripts": [{
                    "id": format!("{id}-t1"),
                    "session_id": id,
                    "started_at": 0.0,
                    "words": words,
                    "speaker_hints": hints,
                }],
            })
            .to_string(),
        )
        .unwrap();
    }

    async fn search(vault: &Path, input: SearchMeetingsInput) -> SearchPage {
        search_meetings(vault, input).await.unwrap()
    }

    fn query(text: &str) -> SearchMeetingsInput {
        SearchMeetingsInput {
            query: Some(text.to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn source_filters_use_shared_prefix_accent_and_phrase_matching() {
        let vault = tempfile::tempdir().unwrap();
        seed_session(vault.path(), "m1", "Planning", "2026-07-13");
        std::fs::write(
            vault.path().join("sessions/m1/notes.md"),
            "Café roadmap review",
        )
        .unwrap();
        for query in ["cafe road", "\"café roadmap\""] {
            let page = search(
                vault.path(),
                SearchMeetingsInput {
                    query: Some(query.into()),
                    kinds: Some(vec![SearchKind::Note]),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(page.hits.len(), 1);
            assert_eq!(page.hits[0].kind, "note");
        }
    }

    #[tokio::test]
    async fn requires_query_or_speaker() {
        let vault = tempfile::tempdir().unwrap();
        let error = search_meetings(vault.path(), SearchMeetingsInput::default())
            .await
            .unwrap_err();
        assert!(matches!(error, Error::InvalidInput(_)));

        let error = search_meetings(
            vault.path(),
            SearchMeetingsInput {
                query: Some("   ".to_string()),
                speaker: Some(" ".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, Error::InvalidInput(_)));
    }

    #[tokio::test]
    async fn matches_title_note_summary_and_transcript() {
        let vault = tempfile::tempdir().unwrap();
        seed_session(vault.path(), "m1", "Budget planning", "2026-07-13");
        let dir = vault.path().join("sessions/m1");
        // Legacy note name on purpose: search must still read `_memo.md` vaults.
        std::fs::write(dir.join("_memo.md"), "We reviewed the budget baseline.").unwrap();
        std::fs::create_dir_all(dir.join("enhanced")).unwrap();
        std::fs::write(
            dir.join("enhanced/doc-1.md"),
            "---\nkind: summary\ntitle: Recap\nsort_order: 1\n---\n\nDecided the budget ships Tuesday.",
        )
        .unwrap();
        write_transcript(
            vault.path(),
            "m1",
            vec![
                write_word("w0", "the", 0, 0.0),
                write_word("w1", "budget", 1, 0.0),
                write_word("w2", "discussion", 2, 0.0),
            ],
            vec![],
        );

        let page = search(vault.path(), query("Budget")).await;
        assert_eq!(page.hits.len(), 1, "one result per session");
        assert_eq!(page.hits[0].kind, "title");
        assert_eq!(page.hits[0].snippet, "Budget planning");
        assert!(page.pagination.next_offset.is_none());

        let page = search(
            vault.path(),
            SearchMeetingsInput {
                query: Some("budget".to_string()),
                kinds: Some(vec![SearchKind::Note]),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(
            page.hits
                .iter()
                .map(|hit| hit.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["note"],
        );
    }

    #[tokio::test]
    async fn canonical_summary_is_searchable_without_indexing_attachments() {
        let vault = tempfile::tempdir().unwrap();
        seed_session(vault.path(), "m1", "Sync", "2026-07-13");
        let dir = vault.path().join("sessions/m1");
        std::fs::create_dir_all(dir.join("attachments")).unwrap();
        std::fs::write(dir.join("summary.md"), "Budget approved.").unwrap();
        std::fs::write(dir.join("attachments/summary.md"), "Budget attachment.").unwrap();
        std::fs::write(dir.join("minutes.md"), "Budget attachment.").unwrap();

        let page = search(vault.path(), query("budget")).await;
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].kind, "summary");
        assert_eq!(page.hits[0].document_id.as_deref(), Some("m1"));
        assert_eq!(page.hits[0].snippet, "Summary Budget approved.");
    }

    #[tokio::test]
    async fn multi_term_query_requires_every_term() {
        let vault = tempfile::tempdir().unwrap();
        seed_session(vault.path(), "m1", "Sync", "2026-07-13");
        std::fs::write(
            vault.path().join("sessions/m1/notes.md"),
            "Budget grows while the forecast shrinks.",
        )
        .unwrap();

        let page = search(vault.path(), query("FORECAST budget")).await;
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].kind, "note");

        let page = search(vault.path(), query("budget missing")).await;
        assert!(page.hits.is_empty());
    }

    #[tokio::test]
    async fn snippet_survives_non_ascii_prefixes() {
        let vault = tempfile::tempdir().unwrap();
        seed_session(vault.path(), "m1", "Sync", "2026-07-13");
        let note = format!("{} target reached", "É".repeat(150));
        std::fs::write(vault.path().join("sessions/m1/notes.md"), &note).unwrap();

        let page = search(vault.path(), query("TARGET")).await;
        assert_eq!(page.hits.len(), 1);
        let snippet = &page.hits[0].snippet;
        assert!(snippet.starts_with('…'), "snippet: {snippet}");
        assert!(snippet.contains("target reached"), "snippet: {snippet}");
    }

    #[tokio::test]
    async fn transcript_hit_start_ms_locates_the_match_in_the_rendered_transcript() {
        let vault = tempfile::tempdir().unwrap();
        seed_session(vault.path(), "m1", "Sync", "2026-07-13");
        write_transcript(
            vault.path(),
            "m1",
            vec![
                write_word("w0", "alpha", 0, 0.0),
                write_word("w1", "bravo", 1, 0.0),
                write_word("w2", "charlie", 2, 0.0),
            ],
            vec![],
        );

        let page = search(vault.path(), query("charlie")).await;
        assert_eq!(page.hits[0].start_ms, Some(2000));

        let transcript = crate::get_meeting_transcript(
            vault.path(),
            GetMeetingTranscriptInput {
                meeting_id: "m1".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(transcript.text.contains("charlie"));
    }

    #[tokio::test]
    async fn speaker_filter_matches_id_and_name_and_excludes_unattributed_words() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::write(
            vault.path().join("people.json"),
            serde_json::json!({"people": [{"id": "bob_peters", "name": "Bob Peters"}]}).to_string(),
        )
        .unwrap();
        seed_session(vault.path(), "m1", "Sync", "2026-07-13");
        write_transcript(
            vault.path(),
            "m1",
            vec![
                write_word("w0", "unattributed", 0, 0.0),
                write_word("w1", "remote", 1, 1.0),
                write_word("w2", "greetings", 2, 1.0),
            ],
            vec![serde_json::json!({
                "word_id": "w1",
                "type": "speaker_label",
                "value": "bob_peters",
            })],
        );

        for filter in ["bob", "PETERS"] {
            let page = search(
                vault.path(),
                SearchMeetingsInput {
                    speaker: Some(filter.to_string()),
                    ..Default::default()
                },
            )
            .await;
            assert_eq!(page.hits.len(), 1, "filter {filter}");
            let hit = &page.hits[0];
            assert_eq!(hit.kind, "transcript");
            assert_eq!(hit.speaker.as_deref(), Some("Bob Peters"));
            assert_eq!(hit.speaker_id.as_deref(), Some("bob_peters"));
            assert_eq!(hit.snippet, "remote greetings");
            assert_eq!(hit.start_ms, Some(1000));
        }

        let page = search(
            vault.path(),
            SearchMeetingsInput {
                speaker: Some("nobody".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert!(page.hits.is_empty());

        // Query + speaker: the speaker gates the meeting, but the query matches the
        // whole transcript — here the matched word belongs to nobody in particular.
        let page = search(
            vault.path(),
            SearchMeetingsInput {
                query: Some("unattributed".to_string()),
                speaker: Some("bob".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].start_ms, Some(0));
        assert_eq!(page.hits[0].speaker, None);

        // A meeting containing the term but not the speaker stays gated out.
        seed_session(vault.path(), "m2", "Sync", "2026-07-14");
        write_transcript(
            vault.path(),
            "m2",
            vec![write_word("w0", "unattributed", 0, 0.0)],
            vec![],
        );
        let page = search(
            vault.path(),
            SearchMeetingsInput {
                query: Some("unattributed".to_string()),
                speaker: Some("bob".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(
            page.hits
                .iter()
                .map(|hit| hit.meeting_id.as_str())
                .collect::<Vec<_>>(),
            vec!["m1"],
        );
    }

    #[tokio::test]
    async fn provider_speaker_index_hints_scope_labels_and_fall_back_to_raw_ids() {
        let vault = tempfile::tempdir().unwrap();
        seed_session(vault.path(), "m1", "Sync", "2026-07-13");
        write_transcript(
            vault.path(),
            "m1",
            vec![
                write_word("w0", "hello", 0, 0.0),
                write_word("w1", "world", 1, 0.0),
                write_word("w2", "again", 2, 0.0),
            ],
            vec![
                serde_json::json!({
                    "word_id": "w0",
                    "type": "provider_speaker_index",
                    "value": {"speaker_index": 0},
                }),
                serde_json::json!({
                    "word_id": "w1",
                    "type": "provider_speaker_index",
                    "value": "{\"speaker_index\": 1}",
                }),
                serde_json::json!({
                    "word_id": "w2",
                    "type": "provider_speaker_index",
                    "value": {"speaker_index": 0},
                }),
                serde_json::json!({
                    "word_id": "w0",
                    "type": "speaker_label",
                    "value": "alice",
                }),
            ],
        );

        let page = search(
            vault.path(),
            SearchMeetingsInput {
                speaker: Some("alice".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(page.hits.len(), 1);
        let hit = &page.hits[0];
        // No people.json entry: the display name falls back to the raw id, and only
        // the (channel 0, speaker 0) words belong to alice.
        assert_eq!(hit.speaker.as_deref(), Some("alice"));
        assert_eq!(hit.snippet, "hello again");
    }

    #[tokio::test]
    async fn multi_index_channel_not_absorbed() {
        let vault = tempfile::tempdir().unwrap();
        seed_session(vault.path(), "m1", "Sync", "2026-07-13");
        write_transcript(
            vault.path(),
            "m1",
            vec![
                write_word("w0", "alpha", 0, 0.0),
                write_word("w1", "bravo", 1, 0.0),
                write_word("w2", "gap", 2, 0.0),
            ],
            vec![
                serde_json::json!({
                    "word_id": "w0",
                    "type": "provider_speaker_index",
                    "value": {"speaker_index": 0},
                }),
                serde_json::json!({
                    "word_id": "w1",
                    "type": "provider_speaker_index",
                    "value": {"speaker_index": 1},
                }),
                serde_json::json!({
                    "word_id": "w0",
                    "type": "speaker_label",
                    "value": "alice",
                }),
                serde_json::json!({
                    "word_id": "w2",
                    "type": "speaker_label",
                    "value": "bob",
                }),
            ],
        );

        // The channel-scope label (bob) must not claim the index alice is
        // explicitly assigned to, and alice must not spill onto other indexes.
        let page = search(
            vault.path(),
            SearchMeetingsInput {
                speaker: Some("alice".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].snippet, "alpha");
        assert_eq!(page.hits[0].start_ms, Some(0));
    }

    #[tokio::test]
    async fn channel_scope_is_fallback_for_unassigned_indexes() {
        let vault = tempfile::tempdir().unwrap();
        seed_session(vault.path(), "m1", "Sync", "2026-07-13");
        write_transcript(
            vault.path(),
            "m1",
            vec![
                write_word("w0", "alpha", 0, 0.0),
                write_word("w1", "bravo", 1, 0.0),
                write_word("w2", "gap", 2, 0.0),
            ],
            vec![
                serde_json::json!({
                    "word_id": "w0",
                    "type": "provider_speaker_index",
                    "value": {"speaker_index": 0},
                }),
                serde_json::json!({
                    "word_id": "w1",
                    "type": "provider_speaker_index",
                    "value": {"speaker_index": 1},
                }),
                serde_json::json!({
                    "word_id": "w0",
                    "type": "speaker_label",
                    "value": "alice",
                }),
                serde_json::json!({
                    "word_id": "w2",
                    "type": "speaker_label",
                    "value": "bob",
                }),
            ],
        );

        // bob's channel-scope label covers the index with no assignment of its
        // own and the null-index gap word.
        let page = search(
            vault.path(),
            SearchMeetingsInput {
                speaker: Some("bob".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].snippet, "bravo gap");
        assert_eq!(page.hits[0].start_ms, Some(1000));
    }

    #[tokio::test]
    async fn undiarized_single_index_channel_keeps_channel_wide_labels() {
        let vault = tempfile::tempdir().unwrap();
        seed_session(vault.path(), "m1", "Sync", "2026-07-13");
        write_transcript(
            vault.path(),
            "m1",
            vec![
                write_word("w0", "solo", 0, 0.0),
                write_word("w1", "tail", 1, 0.0),
            ],
            vec![
                serde_json::json!({
                    "word_id": "w0",
                    "type": "provider_speaker_index",
                    "value": {"speaker_index": 0},
                }),
                serde_json::json!({
                    "word_id": "w1",
                    "type": "speaker_label",
                    "value": "carol",
                }),
            ],
        );

        // One distinct index is not diarization: the channel-scope label keeps
        // covering the whole channel, indexed words included.
        let page = search(
            vault.path(),
            SearchMeetingsInput {
                speaker: Some("carol".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].snippet, "solo tail");
        assert_eq!(page.hits[0].start_ms, Some(0));
    }

    #[tokio::test]
    async fn relevance_pagination_preserves_cached_entries_after_corruption() {
        let vault = tempfile::tempdir().unwrap();
        for id in ["old", "mid", "new"] {
            seed_session(
                vault.path(),
                id,
                if id == "new" { "Keyword" } else { "Sync" },
                "2026-07-01",
            );
            std::fs::write(
                vault.path().join(format!("sessions/{id}/notes.md")),
                "shared keyword",
            )
            .unwrap();
        }
        let first = search(
            vault.path(),
            SearchMeetingsInput {
                query: Some("keyword".into()),
                limit: Some(2),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(first.hits[0].meeting_id, "new", "title match ranks first");
        assert_eq!(first.pagination.next_offset, Some(2));
        std::fs::write(
            vault.path().join("sessions/new/transcript.json"),
            "not json",
        )
        .unwrap();
        crate::open_search_cache(vault.path())
            .unwrap()
            .refresh(true)
            .unwrap();
        let second = search(
            vault.path(),
            SearchMeetingsInput {
                query: Some("keyword".into()),
                limit: Some(2),
                offset: Some(2),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(second.hits.len(), 1);
        assert!(
            !first
                .hits
                .iter()
                .any(|hit| hit.meeting_id == second.hits[0].meeting_id)
        );
        assert!(second.pagination.next_offset.is_none());
        assert_eq!(search(vault.path(), query("keyword")).await.hits.len(), 3);
    }

    #[tokio::test]
    async fn transcript_search_returns_one_useful_hit_per_session() {
        let vault = tempfile::tempdir().unwrap();
        seed_session(vault.path(), "m1", "Sync", "2026-07-13");
        let mut words = Vec::new();
        for index in 0..260u32 {
            let text = if [0, 5, 120, 130, 250].contains(&index) {
                "needle".to_string()
            } else {
                format!("filler{index}")
            };
            words.push(write_word(&format!("w{index}"), &text, index, 0.0));
        }
        write_transcript(vault.path(), "m1", words, vec![]);

        let page = search(vault.path(), query("needle")).await;
        assert_eq!(
            page.hits
                .iter()
                .map(|hit| hit.start_ms.unwrap())
                .collect::<Vec<_>>(),
            vec![0],
            "one result per session retains the first matching timestamp",
        );
    }
}

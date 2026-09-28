use std::collections::HashMap;

use hypr_template_app::{
    EditableTemplate, EnhanceSystem, EnhanceUser, Segment, Session, Template, Transcript,
    template_source,
};
use hypr_transcript::{
    ChannelProfile, IdentityAssignment, IdentityScope, RenderTranscriptHuman,
    RenderTranscriptInput, RenderTranscriptRequest, RenderTranscriptWordInput,
    render_transcript_segments,
};
use hypr_vault_read::{SessionMeta, TranscriptWithData};
use hypr_vault_write::SessionStore;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct Prepared {
    pub instructions: String,
    pub input: String,
    pub source_hash: String,
}

pub async fn prepare(store: &SessionStore, id: &str, language: &str) -> Result<Prepared, String> {
    let meta = store
        .read_meta(id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("Session was deleted")?;
    let (note, transcripts) = tokio::try_join!(store.read_note(id), store.session_transcripts(id))
        .map_err(|error| error.to_string())?;
    let vault = store.vault_base().to_path_buf();
    let language = language.to_string();
    tokio::task::spawn_blocking(move || {
        let config =
            hypr_storage::config::read_config(&vault).map_err(|error| error.to_string())?;
        let prompt_override = config.auto_summary_prompt;
        let humans = hypr_vault_read::read_people(&vault)
            .into_iter()
            .filter(|person| !person.name.trim().is_empty())
            .map(|person| RenderTranscriptHuman {
                human_id: person.id,
                name: person.name,
            })
            .collect();
        let mut prepared = assemble(
            meta,
            note.unwrap_or_default(),
            transcripts,
            humans,
            &language,
            prompt_override,
        )?;
        prepared.source_hash =
            Sha256::digest(format!("{}:{}", prepared.source_hash, config.ai_language))
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
        Ok(prepared)
    })
    .await
    .map_err(|error| error.to_string())?
}

pub async fn prepare_title(
    store: &SessionStore,
    id: &str,
    language: &str,
) -> Result<Option<(Prepared, String, String)>, String> {
    let meta = store
        .read_meta(id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("Session was deleted")?;
    if !title_eligible(&meta.title) {
        return Ok(None);
    }
    let summary = store
        .read_summary(id)
        .await
        .map_err(|error| error.to_string())?
        .filter(|summary| !summary.trim().is_empty())
        .ok_or("Create a summary before generating its title")?;
    let instructions =
        hypr_template_app::render(Template::TitleSystem(hypr_template_app::TitleSystem {
            language: Some(language.to_owned()),
        }))
        .map_err(|error| error.to_string())?;
    let input = hypr_template_app::render(Template::TitleUser(hypr_template_app::TitleUser {
        enhanced_note: summary.clone(),
    }))
    .map_err(|error| error.to_string())?;
    let source = serde_json::to_vec(&(
        &meta.title,
        &summary,
        language,
        template_source(EditableTemplate::TitleUser),
    ))
    .map_err(|error| error.to_string())?;
    let source_hash = Sha256::digest(source)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(Some((
        Prepared {
            instructions,
            input,
            source_hash,
        },
        meta.title,
        summary,
    )))
}

pub fn title_eligible(title: &str) -> bool {
    title.trim().is_empty() || title == "Untitled note"
}

fn assemble(
    meta: SessionMeta,
    note: String,
    transcripts: Vec<TranscriptWithData>,
    humans: Vec<RenderTranscriptHuman>,
    language: &str,
    prompt_override: String,
) -> Result<Prepared, String> {
    let pre_meeting_memo = transcripts
        .first()
        .map(|t| t.memo_md.clone())
        .unwrap_or_default();
    let segments = render_transcript_segments(RenderTranscriptRequest {
        transcripts: transcripts.iter().map(render_input).collect(),
        participant_human_ids: Vec::new(),
        self_human_id: None,
        humans,
    });
    if note.trim().is_empty() && pre_meeting_memo.trim().is_empty() && segments.is_empty() {
        return Err("Add notes or a transcript before creating a summary".into());
    }
    let user = EnhanceUser {
        session: Session {
            title: Some(meta.title.clone()),
            started_at: meta.started_at.clone(),
            ended_at: meta.ended_at.clone(),
            event: None,
        },
        participants: Vec::new(),
        transcripts: if segments.is_empty() {
            Vec::new()
        } else {
            vec![Transcript {
                started_at: transcripts.first().map(|t| t.started_at.max(0.0) as u64),
                ended_at: transcripts
                    .iter()
                    .filter_map(|t| t.ended_at)
                    .reduce(f64::max)
                    .map(|end| end.max(0.0) as u64),
                segments: segments
                    .into_iter()
                    .map(|segment| Segment {
                        speaker: segment.speaker_label,
                        text: segment.text,
                        is_current_user: Some(false),
                    })
                    .collect(),
            }]
        },
        pre_meeting_memo,
        post_meeting_memo: note.clone(),
    };
    let input = hypr_template_app::render(Template::EnhanceUser(Box::new(user.clone())))
        .map_err(|error| error.to_string())?;
    // Hash source material and prompt configuration, not the system prompt's changing current date.
    let source = serde_json::to_vec(&(
        &meta,
        &note,
        &transcripts,
        &user,
        &input,
        &prompt_override,
        language,
        template_source(EditableTemplate::EnhanceSystem),
        template_source(EditableTemplate::EnhanceUser),
    ))
    .map_err(|error| error.to_string())?;
    let instructions = hypr_template_app::render(Template::EnhanceSystem(EnhanceSystem {
        language: Some(language.to_string()),
        prompt_override,
    }))
    .map_err(|error| error.to_string())?;
    Ok(Prepared {
        instructions,
        input,
        source_hash: Sha256::digest(source)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}

pub(crate) fn render_input(transcript: &TranscriptWithData) -> RenderTranscriptInput {
    let mut by_id = HashMap::new();
    let mut words: Vec<_> = transcript
        .words
        .iter()
        .enumerate()
        .map(|(index, word)| {
            let id = word
                .id
                .clone()
                .unwrap_or_else(|| format!("{}:word:{index}", transcript.id));
            by_id.insert(id.clone(), index);
            RenderTranscriptWordInput {
                id,
                text: word.text.clone(),
                start_ms: word.start_ms.round() as i64,
                end_ms: word.end_ms.round() as i64,
                channel: word.channel.round() as i32,
                speaker_index: None,
            }
        })
        .collect();
    for hint in &transcript.speaker_hints {
        if hint.hint_type != "provider_speaker_index" {
            continue;
        }
        let Some(&index) = by_id.get(&hint.word_id) else {
            continue;
        };
        let parsed;
        let value = if let Value::String(raw) = &hint.value {
            let Ok(value) = serde_json::from_str::<Value>(raw) else {
                continue;
            };
            parsed = value;
            &parsed
        } else {
            &hint.value
        };
        words[index].speaker_index = value
            .get("speaker_index")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
            .map(|value| value.round() as i32);
        if let Some(channel) = value
            .get("channel")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
        {
            words[index].channel = channel.round() as i32;
        }
    }
    let mut imported_speakers = HashMap::new();
    for (word, stored) in words.iter_mut().zip(&transcript.words) {
        let source = stored
            .metadata
            .as_ref()
            .and_then(|m| m.get("capture_source"))
            .and_then(Value::as_str);
        if word.channel != 2 && matches!(source, Some("import" | "unknown")) {
            let next = imported_speakers.len() as i32;
            let index = *imported_speakers
                .entry((word.channel, word.speaker_index))
                .or_insert(next);
            word.channel = 2;
            word.speaker_index = Some(index);
        }
    }
    let mut assignments = Vec::new();
    for hint in &transcript.speaker_hints {
        if hint.hint_type != "speaker_label" {
            continue;
        }
        let Some(&index) = by_id.get(&hint.word_id) else {
            continue;
        };
        let Some(label) = hint.value.as_str().filter(|label| !label.is_empty()) else {
            continue;
        };
        let word = &words[index];
        let channel = ChannelProfile::from(word.channel);
        assignments.push(IdentityAssignment {
            human_id: label.to_string(),
            scope: match word.speaker_index {
                Some(speaker_index) => IdentityScope::ChannelSpeaker {
                    channel,
                    speaker_index,
                },
                None => IdentityScope::Channel { channel },
            },
        });
    }
    if assignments.is_empty() {
        for (word, stored) in words.iter().zip(&transcript.words) {
            if let Some(speaker) = stored.speaker.as_ref().filter(|s| !s.trim().is_empty()) {
                assignments.push(IdentityAssignment {
                    human_id: speaker.clone(),
                    scope: IdentityScope::Words {
                        word_ids: vec![word.id.clone()],
                    },
                });
            }
        }
    }
    let synthetic_timing = transcript.words.iter().any(|word| {
        let Some(metadata) = &word.metadata else {
            return false;
        };
        let source = metadata
            .get("timing")
            .and_then(|v| v.get("source"))
            .or_else(|| metadata.get("timing_source"))
            .and_then(Value::as_str);
        matches!(source, Some("synthetic_speech" | "synthetic_text"))
    });
    RenderTranscriptInput {
        started_at: Some(transcript.started_at.round() as i64),
        words,
        assignments,
        synthetic_timing: synthetic_timing.then_some(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(note: &str, transcripts: Value) -> (tempfile::TempDir, SessionStore) {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("sessions/s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("_meta.json"),
            json!({
                "id": "s1", "title": "Release plan", "started_at": null, "ended_at": null,
                "created_at": "2026-09-27T10:00:00Z", "tags": []
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(dir.join("notes.md"), note).unwrap();
        std::fs::write(
            dir.join("transcript.json"),
            json!({"transcripts": transcripts}).to_string(),
        )
        .unwrap();
        let store = SessionStore::new(root.path().to_path_buf());
        (root, store)
    }

    fn transcript() -> Value {
        json!([{
            "id": "t1", "session_id": "s1", "user_id": "owner-is-not-a-speaker",
            "started_at": 1000, "memo_md": "Ask about launch timing.",
            "words": [
                {"text": "Ship", "start_ms": 0, "end_ms": 100, "channel": 0},
                {"id": "w2", "text": " on Friday.", "start_ms": 100, "end_ms": 200, "channel": 0}
            ]
        }])
    }

    #[tokio::test]
    async fn note_only_uses_desktop_prompt_and_complete_notes() {
        let (_root, store) = fixture("### Deadline\nShip by Friday.", json!([]));
        let prepared = prepare(&store, "s1", "en").await.unwrap();
        assert!(
            prepared
                .input
                .contains("# Notes\n\n### Deadline\nShip by Friday.")
        );
        assert!(!prepared.input.contains("# Transcript"));
        assert!(prepared.input.contains("Session: Release plan"));
        assert_eq!(
            prepared.instructions,
            hypr_template_app::render(Template::EnhanceSystem(EnhanceSystem {
                language: Some("en".into()),
                prompt_override: String::new()
            }))
            .unwrap()
        );
    }

    #[tokio::test]
    async fn combines_full_notes_snapshot_and_words_without_ids() {
        let (_root, store) = fixture("Decision: release Friday.", transcript());
        let prepared = prepare(&store, "s1", "en").await.unwrap();
        assert!(
            prepared
                .input
                .contains("# Pre-Meeting Notes\n\nAsk about launch timing.")
        );
        assert!(
            prepared
                .input
                .contains("# Meeting Notes\n\nDecision: release Friday.")
        );
        assert!(prepared.input.contains("Speaker 1: Ship on Friday."));
        assert!(!prepared.input.contains("[current user]:"));
    }

    #[tokio::test]
    async fn title_uses_shared_prompts_and_preserves_custom_titles() {
        let (_root, store) = fixture("Release Friday.", json!([]));
        assert!(prepare_title(&store, "s1", "nl").await.unwrap().is_none());
        store
            .update_meta(
                "s1",
                hypr_vault_write::SessionMetaPatch {
                    title: Some("Untitled note".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        store
            .create_generated_summary("s1", "Release on Friday.")
            .await
            .unwrap();
        let (prepared, title, summary) = prepare_title(&store, "s1", "nl").await.unwrap().unwrap();
        assert_eq!(title, "Untitled note");
        assert_eq!(summary, "Release on Friday.");
        assert!(prepared.instructions.contains("Dutch"));
        assert_eq!(
            prepared.input,
            hypr_template_app::render(Template::TitleUser(hypr_template_app::TitleUser {
                enhanced_note: summary
            }))
            .unwrap()
        );
        assert!(!title_eligible("Custom title"));
        assert!(title_eligible("  "));
        assert_ne!(
            prepared.source_hash,
            prepare_title(&store, "s1", "en")
                .await
                .unwrap()
                .unwrap()
                .0
                .source_hash
        );
    }

    #[tokio::test]
    async fn no_content_rejected_despite_session_title() {
        let (_root, store) = fixture("  \n", json!([]));
        assert!(
            prepare(&store, "s1", "en")
                .await
                .unwrap_err()
                .contains("Add notes or a transcript")
        );
    }

    #[tokio::test]
    async fn desktop_prompt_override_and_validation_are_shared() {
        let (root, store) = fixture("Release Friday.", json!([]));
        std::fs::write(
            root.path().join("config.json"),
            json!({
                "auto_summary_prompt": "Summarize in {{ language|upper }}."
            })
            .to_string(),
        )
        .unwrap();
        let prepared = prepare(&store, "s1", "nl").await.unwrap();
        assert_eq!(prepared.instructions, "Summarize in DUTCH.");
        std::fs::write(
            root.path().join("config.json"),
            json!({
                "auto_summary_prompt": "Summarize {{ transcript }}."
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            prepare(&store, "s1", "nl")
                .await
                .unwrap_err()
                .contains("unknown variables: transcript")
        );
    }

    #[tokio::test]
    async fn hash_tracks_notes_transcript_metadata_prompt_and_language() {
        let (root, store) = fixture("Release Friday.", transcript());
        let baseline = prepare(&store, "s1", "en").await.unwrap().source_hash;
        assert_eq!(
            baseline,
            prepare(&store, "s1", "en").await.unwrap().source_hash
        );
        assert_ne!(
            baseline,
            prepare(&store, "s1", "nl").await.unwrap().source_hash
        );
        store.write_note("s1", "Release Monday.").await.unwrap();
        assert_ne!(
            baseline,
            prepare(&store, "s1", "en").await.unwrap().source_hash
        );
        store.write_note("s1", "Release Friday.").await.unwrap();
        let path = root.path().join("sessions/s1/_meta.json");
        let original = std::fs::read(&path).unwrap();
        let mut meta: Value = serde_json::from_slice(&original).unwrap();
        meta["title"] = json!("Changed title");
        std::fs::write(&path, meta.to_string()).unwrap();
        assert_ne!(
            baseline,
            prepare(&store, "s1", "en").await.unwrap().source_hash
        );
        std::fs::write(&path, original).unwrap();
        let mut changed = transcript();
        changed[0]["words"][0]["text"] = json!("Delay");
        std::fs::write(
            root.path().join("sessions/s1/transcript.json"),
            json!({"transcripts": changed}).to_string(),
        )
        .unwrap();
        assert_ne!(
            baseline,
            prepare(&store, "s1", "en").await.unwrap().source_hash
        );
        std::fs::write(
            root.path().join("sessions/s1/transcript.json"),
            json!({"transcripts": transcript()}).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.path().join("config.json"),
            json!({"auto_summary_prompt": "Summarize in {{ language }}."}).to_string(),
        )
        .unwrap();
        assert_ne!(
            baseline,
            prepare(&store, "s1", "en").await.unwrap().source_hash
        );
    }

    #[tokio::test]
    async fn prompt_override_changes_hash_without_changing_source_input() {
        let (root, store) = fixture("Release Friday.", json!([]));
        let before = prepare(&store, "s1", "en").await.unwrap();
        std::fs::write(
            root.path().join("config.json"),
            json!({
                "auto_summary_prompt": "Summarize in {{ language }}."
            })
            .to_string(),
        )
        .unwrap();
        let after = prepare(&store, "s1", "en").await.unwrap();
        assert_eq!(before.input, after.input);
        assert_ne!(before.source_hash, after.source_hash);
    }

    #[tokio::test]
    async fn shared_language_change_invalidates_an_inflight_summary() {
        let (root, store) = fixture("Release Friday.", json!([]));
        let before = prepare(&store, "s1", "en").await.unwrap();
        std::fs::write(root.path().join("config.json"), r#"{"ai_language":"fr"}"#).unwrap();
        let after = prepare(&store, "s1", "en").await.unwrap();
        assert_ne!(before.source_hash, after.source_hash);
    }

    #[tokio::test]
    async fn preserves_imported_speaker_indexes_labels_and_registry_names() {
        let mut data = transcript();
        data[0]["words"][0]["id"] = json!("w1");
        data[0]["speaker_hints"] = json!([
            {"word_id": "w1", "type": "provider_speaker_index", "value": "{\"speaker_index\":0}"},
            {"word_id": "w2", "type": "provider_speaker_index", "value": {"speaker_index": 1}},
            {"word_id": "w1", "type": "speaker_label", "value": "ada"},
            {"word_id": "w2", "type": "speaker_label", "value": "bob"}
        ]);
        let (root, store) = fixture("", data);
        std::fs::write(
            root.path().join("people.json"),
            json!({"people": [
                {"id": "ada", "name": "Ada"}, {"id": "bob", "name": "Bob"}
            ]})
            .to_string(),
        )
        .unwrap();
        let prepared = prepare(&store, "s1", "en").await.unwrap();
        assert!(prepared.input.contains("Ada: Ship"));
        assert!(prepared.input.contains("Bob: on Friday."));
        assert!(!prepared.input.contains("[current user]:"));
    }
}

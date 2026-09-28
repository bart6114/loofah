use hypr_transcript::{RenderTranscriptHuman, RenderTranscriptRequest, render_transcript_segments};
use hypr_vault_write::SessionStore;
use tauri::State;

use super::{MobileState, summary};

#[derive(Debug, serde::Serialize)]
pub struct TranscriptExportSegment {
    speaker: String,
    text: String,
}

#[tauri::command]
pub async fn mobile_transcript_export(
    state: State<'_, MobileState>,
    session_id: String,
) -> Result<Vec<TranscriptExportSegment>, String> {
    export(&state.store, &session_id).await
}

async fn export(store: &SessionStore, id: &str) -> Result<Vec<TranscriptExportSegment>, String> {
    store
        .read_meta(id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("Session not found")?;
    let transcripts = store
        .session_transcripts(id)
        .await
        .map_err(|error| error.to_string())?;
    let vault = store.vault_base().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let humans = hypr_vault_read::read_people(&vault)
            .into_iter()
            .filter(|person| !person.name.trim().is_empty())
            .map(|person| RenderTranscriptHuman {
                human_id: person.id,
                name: person.name,
            })
            .collect();
        render_transcript_segments(RenderTranscriptRequest {
            transcripts: transcripts.iter().map(summary::render_input).collect(),
            participant_human_ids: Vec::new(),
            self_human_id: None,
            humans,
        })
        .into_iter()
        .map(|segment| TranscriptExportSegment {
            speaker: segment.speaker_label,
            text: segment.text,
        })
        .collect()
    })
    .await
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn exports_every_paragraph_and_preserves_people_names() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sessions/s1");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("_meta.json"),
            json!({"id":"s1", "title":"Long conversation", "created_at":"2026-09-27T10:00:00Z", "tags":[]}).to_string(),
        ).unwrap();
        std::fs::write(
            root.path().join("people.json"),
            json!({"people":[{"id":"alex", "name":"Alex Rivera"}]}).to_string(),
        )
        .unwrap();
        let transcripts: Vec<_> = (0..65)
            .map(|index| {
                json!({
                    "id":format!("t{index}"), "session_id":"s1", "started_at":index * 3000,
                    "words":[{
                        "id":format!("w{index}"), "text":format!("Paragraph {index}."),
                        "start_ms":0, "end_ms":1000, "channel":2
                    }],
                    "speaker_hints":[{
                        "word_id":format!("w{index}"), "type":"speaker_label",
                        "value":if index % 2 == 0 { "alex" } else { "Guest" }
                    }]
                })
            })
            .collect();
        std::fs::write(
            directory.join("transcript.json"),
            json!({"transcripts":transcripts}).to_string(),
        )
        .unwrap();
        let store = SessionStore::new(root.path().to_path_buf());
        let segments = export(&store, "s1").await.unwrap();
        assert_eq!(segments.len(), 65);
        assert_eq!(segments[0].speaker, "Alex Rivera");
        assert_eq!(segments[1].speaker, "Guest");
        assert_eq!(segments.last().unwrap().text, "Paragraph 64.");
    }
}

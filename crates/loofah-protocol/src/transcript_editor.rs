use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TranscriptEditorSnapshot {
    pub session_id: String,
    pub session_created_at: String,
    pub revision: String,
    pub transcripts: Vec<crate::TranscriptWithData>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TranscriptSpeakerAssignment {
    pub transcript_id: String,
    pub channel: i32,
    pub speaker_index: Option<i32>,
    pub speaker_label: String,
    pub anchor_word_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct AssignTranscriptSpeakersRequest {
    pub expected_revision: String,
    pub assignments: Vec<TranscriptSpeakerAssignment>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "snapshot", rename_all = "snake_case")]
pub enum TranscriptEditorSaveOutcome {
    Saved(TranscriptEditorSnapshot),
    Conflict(TranscriptEditorSnapshot),
}

pub fn validate_transcript_speakers(
    input: &AssignTranscriptSpeakersRequest,
) -> Result<(), &'static str> {
    if input.expected_revision.len() != 64
        || !input
            .expected_revision
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err("Original transcript revision is required");
    }
    if input.assignments.is_empty() || input.assignments.len() > 64 {
        return Err("One to 64 speaker assignments are required");
    }
    for assignment in &input.assignments {
        if !(0..=255).contains(&assignment.channel)
            || assignment
                .speaker_index
                .is_some_and(|index| !(0..=4095).contains(&index))
        {
            return Err("Invalid speaker channel or index");
        }
        for value in [
            &assignment.transcript_id,
            &assignment.speaker_label,
            &assignment.anchor_word_id,
        ] {
            if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control)
            {
                return Err("Invalid transcript speaker assignment");
            }
        }
    }
    Ok(())
}

pub const TRANSCRIPT_EDITOR_RESPONSE_LIMIT: usize = 72 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct EnsurePersonRequest {
    pub name: String,
}
pub fn validate_person_name(name: &str) -> Result<(), &'static str> {
    if name.trim().is_empty() || name.len() > 1024 || name.chars().any(char::is_control) {
        Err("Person name must contain one to 1024 bytes without control characters")
    } else {
        Ok(())
    }
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct LocalMediaSourceSnapshot {
    pub session_id: String,
    pub session_created_at: String,
    pub recording_selection: String,
    pub recording_sha256: String,
    pub transcript_revision: String,
    pub transcript_present: bool,
    pub audio: hypr_vault_read::SessionAudio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LocalMediaKind {
    ImportAudio,
    Transcribe,
    ImportSubtitles,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LocalMediaState {
    Running,
    Succeeded,
    Failed,
    Review,
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct LocalMediaStatus {
    pub request_id: String,
    pub session_id: String,
    pub kind: LocalMediaKind,
    pub state: LocalMediaState,
    pub transcript_id: Option<String>,
    pub word_count: Option<usize>,
    pub error: Option<crate::ApplicationError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported: Option<hypr_vault_read::SessionMeta>,
}

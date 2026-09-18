use std::collections::HashMap;

pub use hypr_fs_format::{
    TranscriptJson, TranscriptSpeakerHint, TranscriptWithData, TranscriptWord,
};
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ScanResult {
    pub files: HashMap<String, String>,
    pub dirs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentSaveResult {
    pub path: String,
    pub attachment_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    pub attachment_id: String,
    pub path: String,
    pub extension: String,
    pub size: u64,
    pub modified_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioImportSourceInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetaParticipant {
    pub id: String,
    pub user_id: String,
    pub session_id: String,
    pub human_id: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetaData {
    pub id: String,
    pub user_id: String,
    pub created_at: Option<String>,
    pub title: Option<String>,
    pub participants: Vec<SessionMetaParticipant>,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionMetaDataSerde {
    id: String,
    #[serde(default)]
    user_id: String,
    created_at: Option<String>,
    title: Option<String>,
    participants: Option<Vec<SessionMetaParticipant>>,
    tags: Option<Vec<String>>,
}

impl<'de> Deserialize<'de> for SessionMetaData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = SessionMetaDataSerde::deserialize(deserializer)?;

        Ok(Self {
            id: value.id,
            user_id: value.user_id,
            created_at: value.created_at,
            title: value.title,
            participants: value.participants.unwrap_or_default(),
            tags: value.tags.unwrap_or_default(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionNoteData {
    pub id: String,
    pub session_id: String,
    pub template_id: Option<String>,
    pub position: Option<i64>,
    pub title: Option<String>,
    pub tiptap_json: serde_json::Value,
    pub markdown: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionContentData {
    pub session_id: String,
    pub meta: Option<SessionMetaData>,
    pub raw_memo_tiptap_json: Option<serde_json::Value>,
    pub raw_memo_markdown: Option<String>,
    pub transcript: Option<TranscriptJson>,
    pub notes: Vec<SessionNoteData>,
}

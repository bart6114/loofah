use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptItem {
    pub speaker: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub items: Vec<TranscriptItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportMetadata {
    pub title: String,
    pub created_at: String,
    pub participants: Vec<String>,
    pub duration: Option<String>,
}

// `src` is the portable attachment URL exactly as it appears in the markdown
// (`attachments/<encoded-filename>`); `path` is where the file lives on disk.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportAttachment {
    pub src: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportInput {
    pub enhanced_md: String,
    pub note_md: Option<String>,
    pub transcript: Option<Transcript>,
    pub metadata: Option<ExportMetadata>,
    #[serde(default)]
    pub attachments: Vec<ExportAttachment>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum TextFormat {
    Md,
    Txt,
    Org,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportLabels {
    pub untitled: String,
    pub created: String,
    pub participants: String,
    pub duration: String,
    pub metadata: String,
    pub note: String,
    pub summary: String,
    pub transcript: String,
}

impl Default for ExportLabels {
    fn default() -> Self {
        Self {
            untitled: "Untitled".into(),
            created: "Created".into(),
            participants: "Participants".into(),
            duration: "Duration".into(),
            metadata: "Metadata".into(),
            note: "Note".into(),
            summary: "Summary".into(),
            transcript: "Transcript".into(),
        }
    }
}

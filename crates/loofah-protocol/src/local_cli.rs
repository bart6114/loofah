use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    AgentExport, AgentMeeting, AgentPage, AgentSearchPage, AgentTranscript, ApplicationError,
    ListMeetingsInput, MetadataSaveOutcome, MetadataSnapshot, NoteSaveOutcome, NoteSnapshot,
    RenderedTranscriptSegment, SaveMetadataRequest, SaveNoteRequest, SearchMeetingsInput, TagItem,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalInitialSessionMetadata {
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    pub skill: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalCliOperationKind {
    AgentList,
    AgentSearch,
    AgentMeeting,
    AgentTranscript,
    AgentExport,
    AgentSegments,
    Metadata,
    SaveMetadata,
    Note,
    SaveNote,
    AppendNote,
    TagDelta,
    CreateSession,
    SessionLifecycle,
    DeleteSession,
    SessionPath,
    Tags,
    Attach,
    Attachment,
    AttachmentPage,
    Import,
    Transcribe,
    RecoverMedia,
    ImportSubtitles,
    RecoverSubtitles,
    Doctor,
    Initialize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalCliOperation {
    AgentList {
        input: ListMeetingsInput,
    },
    AgentSearch {
        input: SearchMeetingsInput,
    },
    AgentMeeting {
        session_id: String,
    },
    AgentTranscript {
        session_id: String,
    },
    AgentExport {
        session_id: String,
    },
    AgentSegments {
        session_id: String,
    },
    Metadata {
        session_id: String,
    },
    SaveMetadata {
        session_id: String,
        input: SaveMetadataRequest,
    },
    Note {
        session_id: String,
    },
    SaveNote {
        session_id: String,
        input: SaveNoteRequest,
    },
    AppendNote {
        session_id: String,
        markdown: String,
        expected_revision: String,
    },
    TagDelta {
        session_id: String,
        tags: Vec<String>,
        remove: bool,
        expected_revision: String,
    },
    CreateSession {
        title: String,
        markdown: Option<String>,
        metadata: LocalInitialSessionMetadata,
    },
    SessionLifecycle {
        session_id: String,
    },
    DeleteSession {
        session_id: String,
        expected_revision: String,
    },
    SessionPath {
        session_id: String,
    },
    Tags {},
    Attach {
        session_id: String,
        filename: String,
    },
    Attachment {
        session_id: String,
        attachment_id: String,
    },
    AttachmentPage {
        session_id: String,
        limit: usize,
        after: Option<String>,
    },
    Import {
        filename: String,
        title: Option<String>,
        into: Option<String>,
        transcribe: bool,
        metadata: LocalInitialSessionMetadata,
    },
    Transcribe {
        session_id: String,
    },
    TranscribeChecked {
        session_id: String,
        expected: crate::LocalMediaSourceSnapshot,
    },
    RecoverMedia {
        request_id: String,
    },
    ImportSubtitles {
        session_id: String,
        filename: String,
    },
    RecoverSubtitles {
        request_id: String,
    },
    Doctor {},
    Initialize {},
}

impl LocalCliOperation {
    pub fn kind(&self) -> LocalCliOperationKind {
        match self {
            Self::AgentList { .. } => LocalCliOperationKind::AgentList,
            Self::AgentSearch { .. } => LocalCliOperationKind::AgentSearch,
            Self::AgentMeeting { .. } => LocalCliOperationKind::AgentMeeting,
            Self::AgentTranscript { .. } => LocalCliOperationKind::AgentTranscript,
            Self::AgentExport { .. } => LocalCliOperationKind::AgentExport,
            Self::AgentSegments { .. } => LocalCliOperationKind::AgentSegments,
            Self::Metadata { .. } => LocalCliOperationKind::Metadata,
            Self::SaveMetadata { .. } => LocalCliOperationKind::SaveMetadata,
            Self::Note { .. } => LocalCliOperationKind::Note,
            Self::SaveNote { .. } => LocalCliOperationKind::SaveNote,
            Self::AppendNote { .. } => LocalCliOperationKind::AppendNote,
            Self::TagDelta { .. } => LocalCliOperationKind::TagDelta,
            Self::CreateSession { .. } => LocalCliOperationKind::CreateSession,
            Self::SessionLifecycle { .. } => LocalCliOperationKind::SessionLifecycle,
            Self::DeleteSession { .. } => LocalCliOperationKind::DeleteSession,
            Self::SessionPath { .. } => LocalCliOperationKind::SessionPath,
            Self::Tags {} => LocalCliOperationKind::Tags,
            Self::Attach { .. } => LocalCliOperationKind::Attach,
            Self::Attachment { .. } => LocalCliOperationKind::Attachment,
            Self::AttachmentPage { .. } => LocalCliOperationKind::AttachmentPage,
            Self::Import { .. } => LocalCliOperationKind::Import,
            Self::Transcribe { .. } | Self::TranscribeChecked { .. } => {
                LocalCliOperationKind::Transcribe
            }
            Self::RecoverMedia { .. } => LocalCliOperationKind::RecoverMedia,
            Self::ImportSubtitles { .. } => LocalCliOperationKind::ImportSubtitles,
            Self::RecoverSubtitles { .. } => LocalCliOperationKind::RecoverSubtitles,
            Self::Doctor {} => LocalCliOperationKind::Doctor,
            Self::Initialize {} => LocalCliOperationKind::Initialize,
        }
    }

    pub fn has_blob(&self) -> bool {
        matches!(
            self,
            Self::Attach { .. } | Self::Import { .. } | Self::ImportSubtitles { .. }
        )
    }

    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::SaveMetadata { .. }
                | Self::SaveNote { .. }
                | Self::AppendNote { .. }
                | Self::TagDelta { .. }
                | Self::CreateSession { .. }
                | Self::DeleteSession { .. }
                | Self::Attach { .. }
                | Self::Import { .. }
                | Self::Transcribe { .. }
                | Self::TranscribeChecked { .. }
                | Self::RecoverMedia { .. }
                | Self::ImportSubtitles { .. }
                | Self::RecoverSubtitles { .. }
                | Self::Doctor {}
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalCliReply {
    WritePersisted {
        request_id: String,
        session_ids: Vec<String>,
        reason: String,
    },
    AgentList {
        page: AgentPage,
    },
    AgentSearch {
        page: AgentSearchPage,
    },
    AgentMeeting {
        meeting: AgentMeeting,
    },
    AgentTranscript {
        transcript: AgentTranscript,
    },
    AgentExport {
        export: AgentExport,
    },
    AgentSegments {
        segments: Vec<RenderedTranscriptSegment>,
    },
    Metadata {
        snapshot: MetadataSnapshot,
    },
    MetadataSaved {
        outcome: MetadataSaveOutcome,
    },
    Note {
        snapshot: NoteSnapshot,
    },
    NoteSaved {
        outcome: NoteSaveOutcome,
    },
    TagDelta {
        outcome: MetadataSaveOutcome,
        changed: Vec<String>,
        tags: Vec<String>,
    },
    SessionCreated {
        meta: hypr_vault_read::SessionMeta,
    },
    SessionLifecycle {
        session_id: String,
        revision: String,
    },
    SessionDeleted {
        session_id: String,
        path: PathBuf,
        trash_path: PathBuf,
        deleted_at: String,
    },
    SessionPath {
        path: PathBuf,
    },
    Tags {
        tags: Vec<TagItem>,
    },
    Attached {
        attachment_id: String,
        relative_path: PathBuf,
    },
    Attachment {
        filename: String,
    },
    AttachmentPage {
        page: crate::AttachmentPage,
    },
    Imported {
        meta: hypr_vault_read::SessionMeta,
        audio_path: PathBuf,
        transcription: Option<Result<LocalTranscriptionOutcome, ApplicationError>>,
    },
    SubtitlesImported {
        session_id: String,
        transcript_id: Option<String>,
        word_count: usize,
        revision: Option<String>,
    },
    Transcribed {
        session_id: String,
        transcript_id: String,
        word_count: usize,
    },
    Doctor {
        session_count: Option<usize>,
        agents_updated: Option<bool>,
        agents_error: Option<String>,
        error: Option<String>,
        cache_path: Option<PathBuf>,
        cache_ready: bool,
        cache_error: Option<String>,
    },
    Initialized {
        cache_path: PathBuf,
        added: usize,
        updated: usize,
        deleted: usize,
        sessions: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalTranscriptionOutcome {
    pub transcript_id: String,
    pub words: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCliProgress {
    pub schema_version: String,
    pub phase: String,
    pub processed: usize,
    pub total: Option<usize>,
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCliResponse {
    pub result: Result<LocalCliReply, ApplicationError>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_controls_reject_unknown_operations_fields_and_empty_variant_extensions() {
        for json in [
            r#"{"operation":"arbitrary_tauri","path":"/tmp"}"#,
            r#"{"operation":"tags","future":true}"#,
            r#"{"operation":"agent_meeting","session_id":"id","path":"/tmp"}"#,
        ] {
            assert!(serde_json::from_str::<LocalCliOperation>(json).is_err());
        }
    }
}

impl LocalCliOperationKind {
    pub fn accepts_reply(&self, reply: &LocalCliReply, has_blob: bool) -> bool {
        if has_blob != matches!(reply, LocalCliReply::Attachment { .. }) {
            return false;
        }
        if matches!(reply, LocalCliReply::WritePersisted { .. }) {
            return matches!(
                self,
                Self::SaveMetadata
                    | Self::SaveNote
                    | Self::AppendNote
                    | Self::TagDelta
                    | Self::CreateSession
                    | Self::DeleteSession
                    | Self::Import
                    | Self::Transcribe
                    | Self::RecoverMedia
                    | Self::ImportSubtitles
                    | Self::RecoverSubtitles
            );
        }
        matches!(
            (self, reply),
            (
                Self::ImportSubtitles | Self::RecoverSubtitles,
                LocalCliReply::SubtitlesImported { .. }
            ) | (Self::AgentList, LocalCliReply::AgentList { .. })
                | (Self::AgentSearch, LocalCliReply::AgentSearch { .. })
                | (Self::AgentMeeting, LocalCliReply::AgentMeeting { .. })
                | (Self::AgentTranscript, LocalCliReply::AgentTranscript { .. })
                | (Self::AgentExport, LocalCliReply::AgentExport { .. })
                | (Self::AgentSegments, LocalCliReply::AgentSegments { .. })
                | (Self::Metadata, LocalCliReply::Metadata { .. })
                | (Self::SaveMetadata, LocalCliReply::MetadataSaved { .. })
                | (Self::Note, LocalCliReply::Note { .. })
                | (
                    Self::SaveNote | Self::AppendNote,
                    LocalCliReply::NoteSaved { .. }
                )
                | (Self::TagDelta, LocalCliReply::TagDelta { .. })
                | (Self::CreateSession, LocalCliReply::SessionCreated { .. })
                | (
                    Self::SessionLifecycle,
                    LocalCliReply::SessionLifecycle { .. }
                )
                | (Self::DeleteSession, LocalCliReply::SessionDeleted { .. })
                | (Self::SessionPath, LocalCliReply::SessionPath { .. })
                | (Self::Tags, LocalCliReply::Tags { .. })
                | (Self::Attach, LocalCliReply::Attached { .. })
                | (Self::Attachment, LocalCliReply::Attachment { .. })
                | (Self::AttachmentPage, LocalCliReply::AttachmentPage { .. })
                | (Self::Import, LocalCliReply::Imported { .. })
                | (Self::Transcribe, LocalCliReply::Transcribed { .. })
                | (
                    Self::RecoverMedia,
                    LocalCliReply::Transcribed { .. } | LocalCliReply::Imported { .. }
                )
                | (Self::Doctor, LocalCliReply::Doctor { .. })
                | (Self::Initialize, LocalCliReply::Initialized { .. })
        )
    }
}

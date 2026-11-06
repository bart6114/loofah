#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub use hypr_agent_access::{
    GetMeetingInput, GetMeetingTranscriptInput, ListMeetingsInput, Meeting as AgentMeeting,
    MeetingExport as AgentExport, MeetingPage as AgentPage, MeetingTranscript as AgentTranscript,
    SearchMeetingsInput, SearchPage as AgentSearchPage,
};
pub use hypr_transcript::RenderedTranscriptSegment;
pub use hypr_vault_read::media::{AudioPeaks, FileInfo, FileMetadata};
pub use hypr_vault_read::paths::validate_attachment_basename;
pub use hypr_vault_read::{EnhancedDoc, PersonItem, TagItem, TaskItem, TranscriptWithData};

pub const API_MAJOR: u32 = 1;
pub const RECORDING_CHUNK_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct AttachmentPage {
    pub files: Vec<FileInfo>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct RecordingLease {
    pub id: String,
    pub vault_id: String,
    pub session_id: String,
    pub metadata: FileMetadata,
    pub idle_timeout_secs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct AttachmentLease {
    pub id: String,
    pub vault_id: String,
    pub session_id: String,
    pub attachment_id: String,
    pub metadata: FileMetadata,
    pub idle_timeout_secs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct RecordingWaveform {
    pub recording_sha256: String,
    pub peaks: AudioPeaks,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct DeleteRecordingRequest {
    pub expected_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct RemoveAttachmentRequest {
    pub expected_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum VaultKind {
    Personal,
    Shared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum VaultRole {
    Reader,
    Editor,
    Owner,
}

impl VaultRole {
    pub fn can_edit(self) -> bool {
        matches!(self, Self::Editor | Self::Owner)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct VaultMutation {
    pub vault_id: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct VaultInfo {
    pub id: String,
    pub name: String,
    pub kind: VaultKind,
    pub role: VaultRole,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct MemberInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct Membership {
    pub member: MemberInfo,
    pub role: VaultRole,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct Me {
    pub member: MemberInfo,
    pub personal_vault_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct CreateVaultRequest {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct UpdateVaultRequest {
    pub name: String,
    pub expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SetMembershipRequest {
    pub role: VaultRole,
    pub expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct CreateSessionRequest {
    pub title: String,
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SessionDocument {
    pub meta: hypr_vault_read::SessionMeta,
    pub note: NoteSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SessionPage {
    pub sessions: Vec<hypr_vault_read::SessionMeta>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct RemoteAuthConfig {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub client_id: String,
    pub audience: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct Capabilities {
    pub api_major: u32,
    pub application_version: String,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Validation,
    NotFound,
    Unauthenticated,
    Forbidden,
    Conflict,
    Unavailable,
    IncompatibleVersion,
    ResourceLimit,
    Cancelled,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MutationOutcome {
    NotCommitted,
    Persisted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ApplicationError {
    pub code: ErrorCode,
    pub message: String,
    pub request_id: Option<String>,
    pub retryable: bool,
    pub current_revision: Option<String>,
    pub mutation_outcome: Option<MutationOutcome>,
}

impl std::fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ApplicationError {}

/// What the frontend sends on a write: source coordinates come from the command arguments,
/// timestamps and `assignee` are managed store-side (preserved from the existing entry when
/// the task already exists).
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
pub struct TaskInput {
    pub id: String,
    pub source_order: i32,
    pub status: String,
    pub text: String,
    pub body: serde_json::Value,
    #[serde(default)]
    pub due_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskSource {
    RawNote,
    Summary,
    TemplateOutput { document_id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct TasksSnapshot {
    pub tasks: Vec<TaskItem>,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ReplaceTasksRequest {
    pub source: TaskSource,
    pub tasks: Vec<TaskInput>,
    pub expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "snapshot", rename_all = "snake_case")]
pub enum TasksSaveOutcome {
    Saved(TasksSnapshot),
    Conflict(TasksSnapshot),
}

/// Partial update for `_meta.json`: `None` means "leave as-is", so callers can patch a single
/// field without knowing the rest. There is deliberately no way to clear a field back to
/// absent -- no mutation site needs that today.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SessionMetaPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct MetadataSnapshot {
    pub meta: hypr_vault_read::SessionMeta,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SaveMetadataRequest {
    pub patch: SessionMetaPatch,
    pub expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "snapshot", rename_all = "snake_case")]
pub enum MetadataSaveOutcome {
    Saved(MetadataSnapshot),
    Conflict(MetadataSnapshot),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SaveNoteRequest {
    pub markdown: String,
    pub title: Option<String>,
    pub expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct NoteEditorSnapshot {
    pub note: NoteSnapshot,
    /// Raw-note tasks only; the revision guards the canonical session task file.
    pub tasks: TasksSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SaveNoteEditorRequest {
    pub markdown: String,
    pub title: Option<String>,
    pub expected_revision: String,
    pub expected_tasks_revision: String,
    pub tasks: Vec<TaskInput>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "snapshot", rename_all = "snake_case")]
pub enum NoteEditorSaveOutcome {
    Saved(NoteEditorSnapshot),
    Conflict(NoteEditorSnapshot),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DocumentTarget {
    Summary,
    TemplateOutput { document_id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct DocumentSnapshot {
    pub document: Option<EnhancedDoc>,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SaveDocumentRequest {
    pub markdown: String,
    pub expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "snapshot", rename_all = "snake_case")]
pub enum DocumentSaveOutcome {
    Saved(DocumentSnapshot),
    Conflict(DocumentSnapshot),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    IndexChanged {
        entity: IndexEntity,
        ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifacts: Option<Vec<ArtifactChange>>,
    },
    SearchReady,
    CatalogChanged,
    ResyncRequired,
    JobChanged {
        job_id: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
pub struct ArtifactChange {
    pub session_id: String,
    pub target: ArtifactTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactTarget {
    Recording,
    Attachments,
    Attachment {
        attachment_id: String,
    },
    #[serde(other)]
    All,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ChangeEvent {
    pub vault_id: String,
    pub cursor: String,
    pub change: Change,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StartupPhase {
    OpeningVault,
    Scanning { sessions_found: usize },
    Indexing { completed: usize, total: usize },
    ArchivingLegacyTemplates,
    Ready,
    Stopping,
    Stopped,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct StartupStatus {
    pub revision: u64,
    pub vault_path: String,
    pub is_cloud_storage: bool,
    pub phase: StartupPhase,
    pub migration_issues: Vec<String>,
}

/// Which index map changed. Serialized as the lowercase strings the frontend matches
/// on in the `index-changed` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum IndexEntity {
    Sessions,
    #[serde(rename = "session_headers")]
    SessionHeaders,
    Docs,
    Transcripts,
    Tasks,
    People,
    /// The vault-root `tags.json` registry changed (not a session's `_meta.json`
    /// tags -- those ride `Sessions`).
    Tags,
    /// Audio or embedded attachment bytes changed independently of indexed text.
    Artifacts,
}

#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
pub struct NoteSnapshot {
    pub markdown: Option<String>,
    pub title: String,
    pub revision: String,
}

#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
#[serde(tag = "kind", content = "note", rename_all = "snake_case")]
pub enum NoteSaveOutcome {
    Saved(NoteSnapshot),
    Conflict(NoteSnapshot),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn artifact_details_are_additive_and_unknown_targets_refresh_safely() {
        let legacy = json!({"kind":"index_changed","entity":"artifacts","ids":["note"]});
        let event: Change = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(serde_json::to_value(event).unwrap(), legacy);
        let current = json!({"kind":"index_changed","entity":"artifacts","ids":["note"],
            "artifacts":[{"session_id":"note","target":{"kind":"attachment","attachment_id":"chart ü.png"}}]});
        let event: Change = serde_json::from_value(current.clone()).unwrap();
        assert_eq!(serde_json::to_value(event).unwrap(), current);
        let target: ArtifactTarget =
            serde_json::from_value(json!({"kind":"future_asset","future_id":"opaque"})).unwrap();
        assert_eq!(target, ArtifactTarget::All);
    }

    #[test]
    fn additive_request_fields_are_accepted_but_expected_revision_is_required() {
        let request: SaveNoteRequest = serde_json::from_value(json!({
            "markdown": "hello", "title": null, "expected_revision": "opaque", "future_option": true,
        })).unwrap();
        assert_eq!(request.expected_revision, "opaque");
        assert_eq!(
            serde_json::from_value::<Change>(json!({"kind":"future_change","extra":true})).unwrap(),
            Change::Unknown
        );
        assert!(serde_json::from_value::<SaveNoteRequest>(json!({"markdown": "hello"})).is_err());
    }

    #[test]
    fn conflicts_have_a_typed_current_document_and_events_keep_a_vault_scope() {
        let conflict = NoteSaveOutcome::Conflict(NoteSnapshot {
            markdown: Some("current".into()),
            title: "Title".into(),
            revision: "revision".into(),
        });
        let value = serde_json::to_value(&conflict).unwrap();
        assert_eq!(value["kind"], "conflict");
        assert_eq!(value["note"]["revision"], "revision");
        assert_eq!(
            serde_json::from_value::<NoteSaveOutcome>(value).unwrap(),
            conflict
        );
        let event = ChangeEvent {
            vault_id: "private".into(),
            cursor: "epoch:1".into(),
            change: Change::ResyncRequired,
        };
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({"vault_id": "private", "cursor": "epoch:1", "change": {"kind": "resync_required"}})
        );
    }
}

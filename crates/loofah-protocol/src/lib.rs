#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub const API_MAJOR: u32 = 1;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SaveNoteRequest {
    pub markdown: String,
    pub title: Option<String>,
    pub expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    IndexChanged {
        entity: IndexEntity,
        ids: Vec<String>,
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

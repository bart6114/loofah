use std::{sync::Arc, time::Duration};

use hypr_loofah_protocol::*;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{RemoteClient, Result, cancelled, failure, invalid, json_body, unavailable};

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteOperation {
    Provision,
    Me,
    Vaults,
    Directory,
    CreateVault {
        input: CreateVaultRequest,
    },
    Vault {
        vault_id: String,
    },
    RenameVault {
        vault_id: String,
        input: UpdateVaultRequest,
    },
    DeleteVault {
        vault_id: String,
        expected_revision: String,
    },
    Memberships {
        vault_id: String,
    },
    SetMembership {
        vault_id: String,
        member_id: String,
        input: SetMembershipRequest,
    },
    RemoveMembership {
        vault_id: String,
        member_id: String,
        expected_revision: String,
    },
    Sessions {
        vault_id: String,
        limit: u32,
        after: Option<String>,
    },
    Session {
        vault_id: String,
        session_id: String,
    },
    Documents {
        vault_id: String,
        session_id: String,
    },
    Document {
        vault_id: String,
        session_id: String,
        target: DocumentTarget,
    },
    SaveDocument {
        vault_id: String,
        session_id: String,
        target: DocumentTarget,
        input: SaveDocumentRequest,
    },
    Transcripts {
        vault_id: String,
        session_id: String,
    },
    AudioMetadata {
        vault_id: String,
        session_id: String,
    },
    DeleteRecording {
        vault_id: String,
        session_id: String,
        input: DeleteRecordingRequest,
    },
    Tasks {
        vault_id: String,
        session_id: String,
    },
    People {
        vault_id: String,
    },
    Tags {
        vault_id: String,
    },
    CreateSession {
        vault_id: String,
        input: CreateSessionRequest,
    },
    Note {
        vault_id: String,
        session_id: String,
    },
    NoteEditor {
        vault_id: String,
        session_id: String,
    },
    SaveNoteEditor {
        vault_id: String,
        session_id: String,
        input: SaveNoteEditorRequest,
    },
    TasksSnapshot {
        vault_id: String,
        session_id: String,
    },
    ReplaceTasks {
        vault_id: String,
        session_id: String,
        input: ReplaceTasksRequest,
    },
    Metadata {
        vault_id: String,
        session_id: String,
    },
    SaveMetadata {
        vault_id: String,
        session_id: String,
        input: SaveMetadataRequest,
    },
    SaveNote {
        vault_id: String,
        session_id: String,
        input: SaveNoteRequest,
    },
    Search {
        vault_id: String,
        input: hypr_search_cache::SearchRequest,
    },
    AgentList {
        vault_id: String,
        input: ListMeetingsInput,
    },
    AgentSearch {
        vault_id: String,
        input: SearchMeetingsInput,
    },
    AgentMeeting {
        vault_id: String,
        session_id: String,
    },
    AgentTranscript {
        vault_id: String,
        session_id: String,
    },
    AgentExport {
        vault_id: String,
        session_id: String,
    },
    AgentSegments {
        vault_id: String,
        session_id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RemoteReply {
    Me(Me),
    Vaults(Vec<VaultInfo>),
    Directory(Vec<MemberInfo>),
    Vault(VaultInfo),
    VaultMutation(VaultMutation),
    Memberships(Vec<Membership>),
    Sessions(SessionPage),
    Session(SessionDocument),
    Documents(Vec<EnhancedDoc>),
    Document(DocumentSnapshot),
    DocumentSave(DocumentSaveOutcome),
    Transcripts(Vec<TranscriptWithData>),
    AudioMetadata(Option<AudioFileMetadata>),
    RecordingDeleted(bool),
    Tasks(Vec<TaskItem>),
    TasksSnapshot(TasksSnapshot),
    TasksSave(TasksSaveOutcome),
    People(Vec<PersonItem>),
    Tags(Vec<TagItem>),
    Note(NoteSnapshot),
    NoteEditor(NoteEditorSnapshot),
    NoteEditorSave(NoteEditorSaveOutcome),
    Metadata(MetadataSnapshot),
    MetadataSave(MetadataSaveOutcome),
    NoteSave(NoteSaveOutcome),
    Search(hypr_search_cache::SearchResult),
    AgentList(AgentPage),
    AgentSearch(AgentSearchPage),
    AgentMeeting(AgentMeeting),
    AgentTranscript(AgentTranscript),
    AgentExport(AgentExport),
    AgentSegments(Vec<RenderedTranscriptSegment>),
}

impl RemoteClient {
    pub async fn execute(
        self: &Arc<Self>,
        operation: RemoteOperation,
        request_id: &str,
        cancel: CancellationToken,
    ) -> Result<RemoteReply> {
        if uuid::Uuid::parse_str(request_id).is_err() {
            return Err(invalid("A UUID request ID is required"));
        }
        if self.stop.is_cancelled() || cancel.is_cancelled() {
            return Err(cancelled());
        }
        let required_feature = match &operation {
            RemoteOperation::NoteEditor { .. } => Some("note_editor_read"),
            RemoteOperation::SaveNoteEditor { .. } => Some("note_editor_write"),
            RemoteOperation::AudioMetadata { .. } => Some("audio_metadata_read"),
            RemoteOperation::DeleteRecording { .. } => Some("recording_delete"),
            _ => None,
        };
        if required_feature.is_some_and(|required| {
            !self
                .capabilities
                .features
                .iter()
                .any(|feature| feature == required)
        }) {
            let description = match operation {
                RemoteOperation::AudioMetadata { .. } => "recording metadata",
                RemoteOperation::DeleteRecording { .. } => "recording deletion",
                _ => "the note editor",
            };
            let mut error = failure(
                ErrorCode::IncompatibleVersion,
                &format!(
                    "This server does not support {description}. Upgrade the server, then reconnect."
                ),
            );
            error.request_id = Some(request_id.to_owned());
            if matches!(
                operation,
                RemoteOperation::SaveNoteEditor { .. } | RemoteOperation::DeleteRecording { .. }
            ) {
                error.mutation_outcome = Some(MutationOutcome::NotCommitted);
            }
            return Err(error);
        }
        let mut query: Vec<(&str, String)> = Vec::new();
        let mut body = None;
        let (method, segments) = match &operation {
            RemoteOperation::Provision => (Method::POST, vec!["me"]),
            RemoteOperation::Me => (Method::GET, vec!["me"]),
            RemoteOperation::Vaults => (Method::GET, vec!["vaults"]),
            RemoteOperation::Directory => (Method::GET, vec!["members"]),
            RemoteOperation::CreateVault { input } => {
                body = Some(value(input)?);
                (Method::POST, vec!["vaults"])
            }
            RemoteOperation::Vault { vault_id } => (Method::GET, vec!["vaults", vault_id]),
            RemoteOperation::RenameVault { vault_id, input } => {
                body = Some(value(input)?);
                (Method::PATCH, vec!["vaults", vault_id])
            }
            RemoteOperation::DeleteVault {
                vault_id,
                expected_revision,
            } => {
                query.push(("expected_revision", expected_revision.clone()));
                (Method::DELETE, vec!["vaults", vault_id])
            }
            RemoteOperation::Memberships { vault_id } => {
                (Method::GET, vec!["vaults", vault_id, "members"])
            }
            RemoteOperation::SetMembership {
                vault_id,
                member_id,
                input,
            } => {
                body = Some(value(input)?);
                (Method::PUT, vec!["vaults", vault_id, "members", member_id])
            }
            RemoteOperation::RemoveMembership {
                vault_id,
                member_id,
                expected_revision,
            } => {
                query.push(("expected_revision", expected_revision.clone()));
                (
                    Method::DELETE,
                    vec!["vaults", vault_id, "members", member_id],
                )
            }
            RemoteOperation::Sessions {
                vault_id,
                limit,
                after,
            } => {
                if !(1..=200).contains(limit) {
                    return Err(invalid("Page size must be between 1 and 200"));
                }
                query.push(("limit", limit.to_string()));
                if let Some(after) = after {
                    query.push(("after", after.clone()));
                }
                (Method::GET, vec!["vaults", vault_id, "sessions"])
            }
            RemoteOperation::Session {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec!["vaults", vault_id, "sessions", session_id],
            ),
            RemoteOperation::CreateSession { vault_id, input } => {
                body = Some(value(input)?);
                (Method::POST, vec!["vaults", vault_id, "sessions"])
            }
            RemoteOperation::DeleteRecording {
                vault_id,
                session_id,
                input,
            } => {
                query.push(("expected_sha256", input.expected_sha256.clone()));
                (
                    Method::DELETE,
                    vec!["vaults", vault_id, "sessions", session_id, "audio"],
                )
            }
            RemoteOperation::Documents {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec!["vaults", vault_id, "sessions", session_id, "documents"],
            ),
            RemoteOperation::Document {
                vault_id,
                session_id,
                target,
            }
            | RemoteOperation::SaveDocument {
                vault_id,
                session_id,
                target,
                ..
            } => {
                let method = if let RemoteOperation::SaveDocument { input, .. } = &operation {
                    body = Some(value(input)?);
                    Method::PUT
                } else {
                    Method::GET
                };
                let segments = match target {
                    DocumentTarget::Summary => {
                        vec!["vaults", vault_id, "sessions", session_id, "summary"]
                    }
                    DocumentTarget::TemplateOutput { document_id } => vec![
                        "vaults",
                        vault_id,
                        "sessions",
                        session_id,
                        "outputs",
                        document_id,
                    ],
                };
                (method, segments)
            }
            RemoteOperation::Transcripts {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec!["vaults", vault_id, "sessions", session_id, "transcripts"],
            ),
            RemoteOperation::Tasks {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec!["vaults", vault_id, "sessions", session_id, "tasks"],
            ),
            RemoteOperation::AudioMetadata {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec![
                    "vaults", vault_id, "sessions", session_id, "audio", "metadata",
                ],
            ),
            RemoteOperation::People { vault_id } => {
                (Method::GET, vec!["vaults", vault_id, "people"])
            }
            RemoteOperation::Tags { vault_id } => (Method::GET, vec!["vaults", vault_id, "tags"]),
            RemoteOperation::Note {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec!["vaults", vault_id, "sessions", session_id, "note"],
            ),
            RemoteOperation::NoteEditor {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec!["vaults", vault_id, "sessions", session_id, "note", "editor"],
            ),
            RemoteOperation::SaveNoteEditor {
                vault_id,
                session_id,
                input,
            } => {
                body = Some(value(input)?);
                (
                    Method::PUT,
                    vec!["vaults", vault_id, "sessions", session_id, "note", "editor"],
                )
            }
            RemoteOperation::SaveNote {
                vault_id,
                session_id,
                input,
            } => {
                body = Some(value(input)?);
                (
                    Method::PUT,
                    vec!["vaults", vault_id, "sessions", session_id, "note"],
                )
            }
            RemoteOperation::AgentList { vault_id, input } => {
                body = Some(value(input)?);
                (
                    Method::POST,
                    vec!["vaults", vault_id, "agent", "sessions", "list"],
                )
            }
            RemoteOperation::AgentSearch { vault_id, input } => {
                body = Some(value(input)?);
                (
                    Method::POST,
                    vec!["vaults", vault_id, "agent", "sessions", "search"],
                )
            }
            RemoteOperation::AgentMeeting {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec!["vaults", vault_id, "agent", "sessions", session_id],
            ),
            RemoteOperation::AgentTranscript {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec![
                    "vaults",
                    vault_id,
                    "agent",
                    "sessions",
                    session_id,
                    "transcript",
                ],
            ),
            RemoteOperation::AgentExport {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec![
                    "vaults", vault_id, "agent", "sessions", session_id, "export",
                ],
            ),
            RemoteOperation::AgentSegments {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec![
                    "vaults", vault_id, "agent", "sessions", session_id, "segments",
                ],
            ),
            RemoteOperation::TasksSnapshot {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec![
                    "vaults", vault_id, "sessions", session_id, "tasks", "snapshot",
                ],
            ),
            RemoteOperation::ReplaceTasks {
                vault_id,
                session_id,
                input,
            } => {
                body = Some(value(input)?);
                (
                    Method::PUT,
                    vec!["vaults", vault_id, "sessions", session_id, "tasks"],
                )
            }
            RemoteOperation::Metadata {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec!["vaults", vault_id, "sessions", session_id, "metadata"],
            ),
            RemoteOperation::SaveMetadata {
                vault_id,
                session_id,
                input,
            } => {
                body = Some(value(input)?);
                (
                    Method::PATCH,
                    vec!["vaults", vault_id, "sessions", session_id, "metadata"],
                )
            }
            RemoteOperation::Search { vault_id, input } => {
                body = Some(value(input)?);
                (Method::POST, vec!["vaults", vault_id, "search"])
            }
        };
        let mutation = !matches!(
            operation,
            RemoteOperation::Search { .. }
                | RemoteOperation::AgentList { .. }
                | RemoteOperation::AgentSearch { .. }
        ) && method != Method::GET;
        let mut url = self.endpoint(&segments)?;
        url.query_pairs_mut().extend_pairs(query);
        let access = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled()),
            _ = self.stop.cancelled() => return Err(cancelled()),
            result = self.access_token() => result?,
        };
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(&access)
            .header("x-loofah-api-major", API_MAJOR.to_string())
            .header("x-request-id", request_id)
            .timeout(Duration::from_secs(35));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let result = async {
            let response = request.send().await.map_err(|_| unavailable())?;
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                self.expire_access(&access).await;
            }
            if status == reqwest::StatusCode::CONFLICT
                && matches!(operation, RemoteOperation::SaveNote { .. })
            {
                return Ok(RemoteReply::NoteSave(
                    json_body(response, 8 * 1024 * 1024).await?,
                ));
            }
            if status == reqwest::StatusCode::CONFLICT
                && matches!(operation, RemoteOperation::SaveDocument { .. })
            {
                return Ok(RemoteReply::DocumentSave(
                    json_body(response, 32 * 1024 * 1024).await?,
                ));
            }
            if status == reqwest::StatusCode::CONFLICT
                && matches!(operation, RemoteOperation::SaveNoteEditor { .. })
            {
                return Ok(RemoteReply::NoteEditorSave(
                    json_body(response, 32 * 1024 * 1024).await?,
                ));
            }
            if status == reqwest::StatusCode::CONFLICT
                && matches!(operation, RemoteOperation::SaveMetadata { .. })
            {
                return Ok(RemoteReply::MetadataSave(
                    json_body(response, 16 * 1024 * 1024).await?,
                ));
            }
            if status == reqwest::StatusCode::CONFLICT
                && matches!(operation, RemoteOperation::ReplaceTasks { .. })
            {
                return Ok(RemoteReply::TasksSave(
                    json_body(response, 16 * 1024 * 1024).await?,
                ));
            }
            if !status.is_success() {
                return Err(json_body::<ApplicationError>(response, 131_072)
                    .await
                    .unwrap_or_else(|_| {
                        failure(ErrorCode::Unavailable, "The server rejected the request")
                    }));
            }
            let reply = match operation {
                RemoteOperation::Provision | RemoteOperation::Me => {
                    RemoteReply::Me(json_body(response, 131_072).await?)
                }
                RemoteOperation::Vaults => {
                    RemoteReply::Vaults(json_body(response, 1024 * 1024).await?)
                }
                RemoteOperation::Directory => {
                    RemoteReply::Directory(json_body(response, 1024 * 1024).await?)
                }
                RemoteOperation::CreateVault { .. } | RemoteOperation::Vault { .. } => {
                    RemoteReply::Vault(json_body(response, 131_072).await?)
                }
                RemoteOperation::RenameVault { .. }
                | RemoteOperation::DeleteVault { .. }
                | RemoteOperation::SetMembership { .. }
                | RemoteOperation::RemoveMembership { .. } => {
                    RemoteReply::VaultMutation(json_body(response, 131_072).await?)
                }
                RemoteOperation::Memberships { .. } => {
                    RemoteReply::Memberships(json_body(response, 1024 * 1024).await?)
                }
                RemoteOperation::Sessions { .. } => {
                    RemoteReply::Sessions(json_body(response, 8 * 1024 * 1024).await?)
                }
                RemoteOperation::CreateSession { .. } | RemoteOperation::Session { .. } => {
                    RemoteReply::Session(json_body(response, 8 * 1024 * 1024).await?)
                }
                RemoteOperation::TasksSnapshot { .. } => {
                    RemoteReply::TasksSnapshot(json_body(response, 16 * 1024 * 1024).await?)
                }
                RemoteOperation::ReplaceTasks { .. } => {
                    RemoteReply::TasksSave(json_body(response, 16 * 1024 * 1024).await?)
                }
                RemoteOperation::Metadata { .. } => {
                    RemoteReply::Metadata(json_body(response, 16 * 1024 * 1024).await?)
                }
                RemoteOperation::SaveMetadata { .. } => {
                    RemoteReply::MetadataSave(json_body(response, 16 * 1024 * 1024).await?)
                }
                RemoteOperation::Note { .. } => {
                    RemoteReply::Note(json_body(response, 8 * 1024 * 1024).await?)
                }
                RemoteOperation::NoteEditor { .. } => {
                    RemoteReply::NoteEditor(json_body(response, 32 * 1024 * 1024).await?)
                }
                RemoteOperation::AudioMetadata { .. } => {
                    RemoteReply::AudioMetadata(json_body(response, 256 * 1024).await?)
                }
                RemoteOperation::SaveNoteEditor { .. } => {
                    RemoteReply::NoteEditorSave(json_body(response, 32 * 1024 * 1024).await?)
                }
                RemoteOperation::Documents { .. } => {
                    RemoteReply::Documents(json_body(response, 32 * 1024 * 1024).await?)
                }
                RemoteOperation::Document { .. } => {
                    RemoteReply::Document(json_body(response, 32 * 1024 * 1024).await?)
                }
                RemoteOperation::SaveDocument { .. } => {
                    RemoteReply::DocumentSave(json_body(response, 32 * 1024 * 1024).await?)
                }
                RemoteOperation::Transcripts { .. } => {
                    RemoteReply::Transcripts(json_body(response, 32 * 1024 * 1024).await?)
                }
                RemoteOperation::DeleteRecording { .. } => {
                    RemoteReply::RecordingDeleted(json_body(response, 131_072).await?)
                }
                RemoteOperation::Tasks { .. } => {
                    RemoteReply::Tasks(json_body(response, 8 * 1024 * 1024).await?)
                }
                RemoteOperation::People { .. } => {
                    RemoteReply::People(json_body(response, 1024 * 1024).await?)
                }
                RemoteOperation::Tags { .. } => {
                    RemoteReply::Tags(json_body(response, 1024 * 1024).await?)
                }
                RemoteOperation::SaveNote { .. } => {
                    RemoteReply::NoteSave(json_body(response, 8 * 1024 * 1024).await?)
                }
                RemoteOperation::AgentList { .. } => {
                    RemoteReply::AgentList(json_body(response, 8 * 1024 * 1024).await?)
                }
                RemoteOperation::AgentSearch { .. } => {
                    RemoteReply::AgentSearch(json_body(response, 8 * 1024 * 1024).await?)
                }
                RemoteOperation::AgentMeeting { .. } => {
                    RemoteReply::AgentMeeting(json_body(response, 32 * 1024 * 1024).await?)
                }
                RemoteOperation::AgentTranscript { .. } => {
                    RemoteReply::AgentTranscript(json_body(response, 32 * 1024 * 1024).await?)
                }
                RemoteOperation::AgentExport { .. } => {
                    RemoteReply::AgentExport(json_body(response, 32 * 1024 * 1024).await?)
                }
                RemoteOperation::AgentSegments { .. } => {
                    RemoteReply::AgentSegments(json_body(response, 32 * 1024 * 1024).await?)
                }
                RemoteOperation::Search { .. } => {
                    RemoteReply::Search(json_body(response, 8 * 1024 * 1024).await?)
                }
            };
            Ok(reply)
        };
        let result = tokio::select! {
            _ = cancel.cancelled() => Err(cancelled()),
            _ = self.stop.cancelled() => Err(cancelled()),
            result = result => result,
        };
        result.map_err(|mut error| {
            error.request_id.get_or_insert_with(|| request_id.into());
            if mutation && error.mutation_outcome.is_none() {
                error.mutation_outcome = Some(
                    if matches!(
                        error.code,
                        ErrorCode::Cancelled
                            | ErrorCode::Unavailable
                            | ErrorCode::Internal
                            | ErrorCode::IncompatibleVersion
                            | ErrorCode::ResourceLimit
                    ) {
                        MutationOutcome::Unknown
                    } else {
                        MutationOutcome::NotCommitted
                    },
                );
            }
            error
        })
    }
}

fn value(input: &impl Serialize) -> Result<Value> {
    serde_json::to_value(input).map_err(|_| invalid("Cannot encode request"))
}

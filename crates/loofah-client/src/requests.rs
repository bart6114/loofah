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
    DeleteEmptySession {
        vault_id: String,
        input: DeleteEmptySessionRequest,
    },
    SessionSummaries {
        vault_id: String,
        input: SessionSummaryQuery,
    },
    ProcessingPolicy {
        vault_id: String,
    },
    ProcessingSettings {
        vault_id: String,
    },
    SaveProcessingSettings {
        vault_id: String,
        input: SaveProcessingSettingsRequest,
    },
    SubmitSummaryJob {
        vault_id: String,
        input: SummaryJobRequest,
    },
    ProcessingJobs {
        vault_id: String,
        input: ListProcessingJobsInput,
    },
    ProcessingJob {
        vault_id: String,
        job_id: String,
    },
    ProcessingJobResult {
        vault_id: String,
        job_id: String,
    },
    CancelProcessingJob {
        vault_id: String,
        job_id: String,
    },
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
    AttachmentInfo {
        vault_id: String,
        session_id: String,
        attachment_id: String,
    },
    AttachmentMetadata {
        vault_id: String,
        session_id: String,
        attachment_id: String,
    },
    RemoveAttachment {
        vault_id: String,
        session_id: String,
        attachment_id: String,
        input: RemoveAttachmentRequest,
    },
    AttachmentUploadStatus {
        upload: crate::AttachmentUpload,
    },
    Attachments {
        vault_id: String,
        session_id: String,
        limit: u32,
        after: Option<String>,
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
    SessionCleanup(DeleteEmptySessionOutcome),
    SessionSummaries(Vec<SessionSummary>),
    ProcessingPolicy(ProcessingPolicy),
    ProcessingSettings(ProcessingSettingsSnapshot),
    ProcessingSettingsSave(ProcessingSettingsSaveOutcome),
    ProcessingJob(ProcessingJobSnapshot),
    ProcessingJobs(ProcessingJobPage),
    ProcessingJobResult(Option<ProcessingJobResult>),
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
    AudioMetadata(Option<FileMetadata>),
    AttachmentInfo(Option<FileInfo>),
    AttachmentMetadata(Option<FileMetadata>),
    AttachmentRemoved(bool),
    AttachmentUploadStatus(AttachmentUploadStatus),
    Attachments(AttachmentPage),
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
            let mut error = invalid("A UUID request ID is required");
            if matches!(operation, RemoteOperation::DeleteEmptySession { .. }) {
                error.mutation_outcome = Some(MutationOutcome::NotCommitted);
            }
            return Err(error);
        }
        if self.stop.is_cancelled() || cancel.is_cancelled() {
            let mut error = cancelled();
            if matches!(operation, RemoteOperation::DeleteEmptySession { .. }) {
                error.mutation_outcome = Some(MutationOutcome::NotCommitted);
            }
            return Err(error);
        }
        let required_feature = match &operation {
            RemoteOperation::DeleteEmptySession { .. } => Some("session_cleanup"),
            RemoteOperation::SessionSummaries { .. } => Some("session_summaries"),
            RemoteOperation::ProcessingPolicy { .. }
            | RemoteOperation::ProcessingSettings { .. }
            | RemoteOperation::SaveProcessingSettings { .. } => Some("processing_settings"),
            RemoteOperation::SubmitSummaryJob { .. }
            | RemoteOperation::ProcessingJobs { .. }
            | RemoteOperation::ProcessingJob { .. }
            | RemoteOperation::ProcessingJobResult { .. }
            | RemoteOperation::CancelProcessingJob { .. } => Some("summary_body_jobs"),
            RemoteOperation::NoteEditor { .. } => Some("note_editor_read"),
            RemoteOperation::SaveNoteEditor { .. } => Some("note_editor_write"),
            RemoteOperation::AudioMetadata { .. } => Some("audio_metadata_read"),
            RemoteOperation::AttachmentInfo { .. } => Some("attachment_info"),
            RemoteOperation::AttachmentMetadata { .. } => Some("attachment_metadata"),
            RemoteOperation::RemoveAttachment { .. } => Some("attachment_remove"),
            RemoteOperation::AttachmentUploadStatus { .. } => Some("attachment_upload"),
            RemoteOperation::Attachments { .. } => Some("attachment_list"),
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
                RemoteOperation::DeleteEmptySession { .. } => "conditional session cleanup",
                RemoteOperation::SessionSummaries { .. } => "session summaries",
                RemoteOperation::ProcessingPolicy { .. }
                | RemoteOperation::ProcessingSettings { .. }
                | RemoteOperation::SaveProcessingSettings { .. } => "processing settings",
                RemoteOperation::SubmitSummaryJob { .. }
                | RemoteOperation::ProcessingJobs { .. }
                | RemoteOperation::ProcessingJob { .. }
                | RemoteOperation::ProcessingJobResult { .. }
                | RemoteOperation::CancelProcessingJob { .. } => "summary processing",
                RemoteOperation::AudioMetadata { .. } => "recording metadata",
                RemoteOperation::AttachmentInfo { .. } => "attachment information",
                RemoteOperation::AttachmentMetadata { .. } => "attachment metadata",
                RemoteOperation::RemoveAttachment { .. } => "attachment removal",
                RemoteOperation::AttachmentUploadStatus { .. } => "attachment uploads",
                RemoteOperation::Attachments { .. } => "attachment listing",
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
                RemoteOperation::SaveNoteEditor { .. }
                    | RemoteOperation::DeleteEmptySession { .. }
                    | RemoteOperation::SaveProcessingSettings { .. }
                    | RemoteOperation::SubmitSummaryJob { .. }
                    | RemoteOperation::CancelProcessingJob { .. }
                    | RemoteOperation::DeleteRecording { .. }
                    | RemoteOperation::RemoveAttachment { .. }
            ) {
                error.mutation_outcome = Some(MutationOutcome::NotCommitted);
            }
            return Err(error);
        }
        if matches!(
            &operation,
            RemoteOperation::SubmitSummaryJob {
                input: SummaryJobRequest {
                    target: DocumentTarget::TemplateOutput { .. },
                    ..
                },
                ..
            }
        ) && !self
            .capabilities
            .features
            .iter()
            .any(|feature| feature == "template_output_jobs")
        {
            let mut error = failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support template output generation. Upgrade the server, then reconnect.",
            );
            error.request_id = Some(request_id.to_owned());
            error.mutation_outcome = Some(MutationOutcome::NotCommitted);
            return Err(error);
        }
        if let RemoteOperation::AttachmentInfo { attachment_id, .. }
        | RemoteOperation::AttachmentMetadata { attachment_id, .. }
        | RemoteOperation::RemoveAttachment { attachment_id, .. } = &operation
            && validate_attachment_basename(attachment_id).is_err()
        {
            let mut error = invalid("Invalid attachment ID");
            error.request_id = Some(request_id.to_owned());
            if matches!(operation, RemoteOperation::RemoveAttachment { .. }) {
                error.mutation_outcome = Some(MutationOutcome::NotCommitted);
            }
            return Err(error);
        }
        if let RemoteOperation::RemoveAttachment { input, .. } = &operation
            && !crate::media::valid_sha256(&input.expected_sha256)
        {
            let mut error = invalid("A lowercase SHA-256 attachment revision is required");
            error.request_id = Some(request_id.to_owned());
            error.mutation_outcome = Some(MutationOutcome::NotCommitted);
            return Err(error);
        }
        if let RemoteOperation::Attachments { limit, after, .. } = &operation
            && (!(1..=200).contains(limit)
                || after
                    .as_deref()
                    .is_some_and(|cursor| validate_attachment_basename(cursor).is_err()))
        {
            let mut error = invalid("Invalid attachment page size or cursor");
            error.request_id = Some(request_id.to_owned());
            return Err(error);
        }
        let mut query: Vec<(&str, String)> = Vec::new();
        if let RemoteOperation::DeleteEmptySession { input, .. } = &operation {
            validate_delete_empty_session_request(input).map_err(|message| {
                let mut error = invalid(message);
                error.request_id = Some(request_id.to_owned());
                error.mutation_outcome = Some(MutationOutcome::NotCommitted);
                error
            })?;
        }
        if let RemoteOperation::SessionSummaries { input, .. } = &operation {
            validate_session_summary_query(input).map_err(invalid)?;
        }
        let mut body = None;
        let (method, segments) = match &operation {
            RemoteOperation::DeleteEmptySession { vault_id, input } => {
                body = Some(value(input)?);
                (Method::POST, vec!["vaults", vault_id, "session-cleanups"])
            }
            RemoteOperation::SessionSummaries { vault_id, input } => {
                body = Some(value(input)?);
                (
                    Method::POST,
                    vec!["vaults", vault_id, "session-summaries", "query"],
                )
            }
            RemoteOperation::ProcessingPolicy { vault_id } => (
                Method::GET,
                vec!["vaults", vault_id, "processing", "policy"],
            ),
            RemoteOperation::ProcessingSettings { vault_id } => (
                Method::GET,
                vec!["vaults", vault_id, "processing", "settings"],
            ),
            RemoteOperation::SaveProcessingSettings { vault_id, input } => {
                body = Some(value(input)?);
                (
                    Method::PUT,
                    vec!["vaults", vault_id, "processing", "settings"],
                )
            }
            RemoteOperation::SubmitSummaryJob { vault_id, input } => {
                body = Some(value(input)?);
                (Method::POST, vec!["vaults", vault_id, "jobs"])
            }
            RemoteOperation::ProcessingJobs { vault_id, input } => {
                if input.limit.is_some_and(|limit| !(1..=200).contains(&limit)) {
                    return Err(invalid("Invalid processing job page size"));
                }
                if let Some(limit) = input.limit {
                    query.push(("limit", limit.to_string()));
                }
                if let Some(after) = &input.after {
                    query.push(("after", after.clone()));
                }
                if let Some(session) = &input.session_id {
                    query.push(("session_id", session.clone()));
                }
                query.push(("active_only", input.active_only.to_string()));
                (Method::GET, vec!["vaults", vault_id, "jobs"])
            }
            RemoteOperation::ProcessingJob { vault_id, job_id } => {
                (Method::GET, vec!["vaults", vault_id, "jobs", job_id])
            }
            RemoteOperation::ProcessingJobResult { vault_id, job_id } => (
                Method::GET,
                vec!["vaults", vault_id, "jobs", job_id, "result"],
            ),
            RemoteOperation::CancelProcessingJob { vault_id, job_id } => (
                Method::POST,
                vec!["vaults", vault_id, "jobs", job_id, "cancel"],
            ),
            RemoteOperation::AttachmentUploadStatus { upload } => {
                validate_upload_attachment_request(&upload.input).map_err(|message| {
                    let mut cause = invalid(message);
                    cause.request_id = Some(request_id.into());
                    cause
                })?;
                uuid::Uuid::parse_str(&upload.request_id)
                    .map_err(|_| invalid("A UUID upload request ID is required"))?;
                query.extend([
                    ("filename", upload.input.filename.clone()),
                    ("size_bytes", upload.input.size_bytes.to_string()),
                    ("sha256", upload.input.sha256.clone()),
                ]);
                (
                    Method::GET,
                    vec![
                        "vaults",
                        &upload.vault_id,
                        "sessions",
                        &upload.session_id,
                        "attachment-uploads",
                        &upload.request_id,
                    ],
                )
            }
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
            RemoteOperation::Attachments {
                vault_id,
                session_id,
                limit,
                after,
            } => {
                query.push(("limit", limit.to_string()));
                if let Some(after) = after {
                    query.push(("after", after.clone()));
                }
                (
                    Method::GET,
                    vec!["vaults", vault_id, "sessions", session_id, "attachments"],
                )
            }
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
            RemoteOperation::RemoveAttachment {
                vault_id,
                session_id,
                attachment_id,
                input,
            } => {
                query.push(("expected_sha256", input.expected_sha256.clone()));
                (
                    Method::DELETE,
                    vec![
                        "vaults",
                        vault_id,
                        "sessions",
                        session_id,
                        "attachments",
                        attachment_id,
                    ],
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
            RemoteOperation::AttachmentInfo {
                vault_id,
                session_id,
                attachment_id,
            }
            | RemoteOperation::AttachmentMetadata {
                vault_id,
                session_id,
                attachment_id,
            } => {
                let suffix = if matches!(&operation, RemoteOperation::AttachmentMetadata { .. }) {
                    "metadata"
                } else {
                    "info"
                };
                (
                    Method::GET,
                    vec![
                        "vaults",
                        vault_id,
                        "sessions",
                        session_id,
                        "attachments",
                        attachment_id,
                        suffix,
                    ],
                )
            }
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
                | RemoteOperation::SessionSummaries { .. }
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
                RemoteOperation::DeleteEmptySession { .. } => {
                    RemoteReply::SessionCleanup(json_body(response, 1024).await?)
                }
                RemoteOperation::SessionSummaries { input, .. } => {
                    let summaries: Vec<SessionSummary> =
                        json_body(response, SESSION_SUMMARY_RESPONSE_LIMIT).await?;
                    validate_summary_reply(&summaries, &input)?;
                    RemoteReply::SessionSummaries(summaries)
                }
                RemoteOperation::ProcessingPolicy { .. } => {
                    RemoteReply::ProcessingPolicy(json_body(response, 4 * 1024 * 1024).await?)
                }
                RemoteOperation::ProcessingSettings { .. } => {
                    RemoteReply::ProcessingSettings(json_body(response, 1024 * 1024).await?)
                }
                RemoteOperation::SaveProcessingSettings { .. } => {
                    RemoteReply::ProcessingSettingsSave(json_body(response, 1024 * 1024).await?)
                }
                RemoteOperation::SubmitSummaryJob { input, .. } => {
                    let job: ProcessingJobSnapshot = json_body(response, 131_072).await?;
                    if job.session_id != input.session_id
                        || job.target != input.target
                        || uuid::Uuid::parse_str(&job.id).is_err()
                    {
                        return Err(unavailable());
                    }
                    RemoteReply::ProcessingJob(job)
                }
                RemoteOperation::ProcessingJob { job_id, .. }
                | RemoteOperation::CancelProcessingJob { job_id, .. } => {
                    let job: ProcessingJobSnapshot = json_body(response, 131_072).await?;
                    if job.id != job_id {
                        return Err(unavailable());
                    }
                    RemoteReply::ProcessingJob(job)
                }
                RemoteOperation::ProcessingJobs { input, .. } => {
                    let page: ProcessingJobPage = json_body(response, 1024 * 1024).await?;
                    validate_processing_page(&page, &input)?;
                    RemoteReply::ProcessingJobs(page)
                }
                RemoteOperation::ProcessingJobResult { vault_id, job_id } => {
                    let result: Option<ProcessingJobResult> =
                        json_body(response, 32 * 1024 * 1024).await?;
                    if result.as_ref().is_some_and(|result| {
                        result.vault_id != vault_id || result.job_id != job_id
                    }) {
                        return Err(unavailable());
                    }
                    RemoteReply::ProcessingJobResult(result)
                }
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
                RemoteOperation::AttachmentInfo { attachment_id, .. } => {
                    let info: Option<FileInfo> = json_body(response, 16 * 1024).await?;
                    if let Some(info) = &info {
                        if info.filename != attachment_id {
                            return Err(failure(
                                ErrorCode::IncompatibleVersion,
                                "Invalid attachment information",
                            ));
                        }
                        crate::media::validate_content_type(&info.content_type)?;
                    }
                    RemoteReply::AttachmentInfo(info)
                }
                RemoteOperation::AttachmentMetadata { attachment_id, .. } => {
                    let metadata: Option<FileMetadata> = json_body(response, 16 * 1024).await?;
                    if let Some(metadata) = &metadata {
                        if metadata.filename != attachment_id {
                            return Err(failure(
                                ErrorCode::IncompatibleVersion,
                                "Invalid attachment metadata",
                            ));
                        }
                        crate::media::validate_file_metadata(metadata)?;
                    }
                    RemoteReply::AttachmentMetadata(metadata)
                }
                RemoteOperation::RemoveAttachment { .. } => {
                    RemoteReply::AttachmentRemoved(json_body(response, 16 * 1024).await?)
                }
                RemoteOperation::AttachmentUploadStatus { upload } => {
                    let status: AttachmentUploadStatus = json_body(response, 16 * 1024).await?;
                    if let AttachmentUploadStatus::Committed(metadata) = &status {
                        crate::uploads::validate_uploaded_metadata(metadata, &upload.input)?;
                    }
                    RemoteReply::AttachmentUploadStatus(status)
                }
                RemoteOperation::Attachments { limit, after, .. } => {
                    let page: AttachmentPage = json_body(response, 256 * 1024).await?;
                    validate_attachment_page(&page, limit as usize, after.as_deref())?;
                    RemoteReply::Attachments(page)
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

fn validate_summary_reply(rows: &[SessionSummary], input: &SessionSummaryQuery) -> Result<()> {
    let invalid = || {
        failure(
            ErrorCode::IncompatibleVersion,
            "Invalid session summary response",
        )
    };
    let limit = match input {
        SessionSummaryQuery::List { limit, .. } | SessionSummaryQuery::Title { limit, .. } => {
            *limit as usize
        }
        SessionSummaryQuery::Ids { ids } => ids.len(),
    };
    if rows.len() > limit {
        return Err(invalid());
    }
    let mut seen = std::collections::HashSet::new();
    let mut budget = SESSION_SUMMARY_TEXT_LIMIT;
    let mut previous_id_position = None;
    for row in rows {
        validate_session_id(&row.id).map_err(|_| invalid())?;
        if !seen.insert(&row.id) {
            return Err(invalid());
        }
        budget = budget
            .checked_sub(
                row.id
                    .len()
                    .saturating_add(row.title.len())
                    .saturating_add(row.created_at.len()),
            )
            .ok_or_else(invalid)?;
        match input {
            SessionSummaryQuery::Ids { ids } => {
                let position = ids
                    .iter()
                    .position(|id| id == &row.id)
                    .ok_or_else(invalid)?;
                if previous_id_position.is_some_and(|previous| position <= previous) {
                    return Err(invalid());
                }
                previous_id_position = Some(position);
            }
            SessionSummaryQuery::List {
                titled_only: true, ..
            } if row.title.is_empty() => return Err(invalid()),
            SessionSummaryQuery::Title { query, .. }
                if !row
                    .title
                    .to_lowercase()
                    .contains(&query.trim().to_lowercase()) =>
            {
                return Err(invalid());
            }
            _ => {}
        }
    }
    let order = match input {
        SessionSummaryQuery::List { order, .. } => Some(*order),
        SessionSummaryQuery::Title { .. } => Some(SessionSummaryOrder::NewestFirst),
        SessionSummaryQuery::Ids { .. } => None,
    };
    if let Some(order) = order {
        for pair in rows.windows(2) {
            let comparison = match order {
                SessionSummaryOrder::NewestFirst => pair[1].created_at.cmp(&pair[0].created_at),
                SessionSummaryOrder::OldestFirst => pair[0].created_at.cmp(&pair[1].created_at),
            }
            .then(pair[0].id.cmp(&pair[1].id));
            if comparison.is_gt() {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

fn validate_processing_page(
    page: &ProcessingJobPage,
    input: &ListProcessingJobsInput,
) -> Result<()> {
    let limit = input.limit.unwrap_or(50) as usize;
    let mut ids = std::collections::HashSet::new();
    if page.items.len() > limit
        || page.items.iter().any(|job| {
            uuid::Uuid::parse_str(&job.id).is_err()
                || !ids.insert(&job.id)
                || input.after.as_ref() == Some(&job.id)
                || input
                    .session_id
                    .as_ref()
                    .is_some_and(|session| session != &job.session_id)
                || input.active_only && job.state.is_terminal()
        })
        || page.next_cursor.as_ref().is_some_and(|cursor| {
            page.items.len() != limit || page.items.last().is_none_or(|job| &job.id != cursor)
        })
    {
        return Err(unavailable());
    }
    Ok(())
}

fn validate_attachment_page(
    page: &AttachmentPage,
    limit: usize,
    after: Option<&str>,
) -> Result<()> {
    let invalid = || failure(ErrorCode::IncompatibleVersion, "Invalid attachment page");
    if page.files.len() > limit {
        return Err(invalid());
    }
    let mut previous = after;
    for file in &page.files {
        if validate_attachment_basename(&file.filename).is_err()
            || previous.is_some_and(|cursor| file.filename.as_str() <= cursor)
        {
            return Err(invalid());
        }
        crate::media::validate_content_type(&file.content_type)?;
        previous = Some(&file.filename);
    }
    if let Some(cursor) = page.next_cursor.as_deref()
        && (page.files.len() != limit
            || page.files.last().map(|file| file.filename.as_str()) != Some(cursor))
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod attachment_page_tests {
    use super::*;

    #[test]
    fn session_summary_replies_validate_limits_identity_order_filters_and_text_budget() {
        let a = SessionSummary {
            id: "a".into(),
            title: "Åland plan".into(),
            created_at: "2026-02-02".into(),
        };
        let b = SessionSummary {
            id: "b".into(),
            title: "Other".into(),
            created_at: "2026-02-01".into(),
        };
        let newest = SessionSummaryQuery::List {
            limit: 2,
            order: SessionSummaryOrder::NewestFirst,
            titled_only: false,
        };
        assert!(validate_summary_reply(&[a.clone(), b.clone()], &newest).is_ok());
        for rows in [
            vec![b.clone(), a.clone()],
            vec![a.clone(), a.clone()],
            vec![a.clone(), b.clone(), b.clone()],
        ] {
            assert!(validate_summary_reply(&rows, &newest).is_err());
        }
        let title = SessionSummaryQuery::Title {
            query: " åLAND ".into(),
            limit: 1,
        };
        assert!(validate_summary_reply(std::slice::from_ref(&a), &title).is_ok());
        assert!(validate_summary_reply(std::slice::from_ref(&b), &title).is_err());
        let ids = SessionSummaryQuery::Ids {
            ids: vec!["b".into(), "a".into(), "b".into()],
        };
        assert!(validate_summary_reply(&[b.clone(), a.clone()], &ids).is_ok());
        assert!(validate_summary_reply(&[a.clone(), b.clone()], &ids).is_err());
        for row in [
            SessionSummary {
                id: "foreign".into(),
                ..a.clone()
            },
            SessionSummary {
                id: "../escape".into(),
                ..a.clone()
            },
            SessionSummary {
                title: "x".repeat(SESSION_SUMMARY_TEXT_LIMIT + 1),
                ..a.clone()
            },
        ] {
            assert!(validate_summary_reply(&[row], &ids).is_err());
        }
        assert!(
            validate_summary_reply(
                &[SessionSummary {
                    title: String::new(),
                    ..a
                }],
                &SessionSummaryQuery::List {
                    limit: 1,
                    order: SessionSummaryOrder::NewestFirst,
                    titled_only: true
                }
            )
            .is_err()
        );
    }

    #[test]
    fn job_pages_reject_duplicate_ids_wrong_sessions_activity_and_cursor_repetition() {
        let job = ProcessingJobSnapshot {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: "note".into(),
            target: DocumentTarget::Summary,
            state: ProcessingJobState::Queued,
            phase: None,
            failure: None,
            created_at: "time".into(),
            updated_at: "time".into(),
        };
        let input = ListProcessingJobsInput {
            limit: Some(1),
            session_id: Some("note".into()),
            active_only: true,
            ..Default::default()
        };
        let mut page = ProcessingJobPage {
            items: vec![job.clone()],
            next_cursor: Some(job.id.clone()),
        };
        assert!(validate_processing_page(&page, &input).is_ok());
        page.next_cursor = Some(uuid::Uuid::new_v4().to_string());
        assert!(validate_processing_page(&page, &input).is_err());
        page.next_cursor = None;
        page.items[0].session_id = "other".into();
        assert!(validate_processing_page(&page, &input).is_err());
        page.items[0] = job.clone();
        page.items[0].state = ProcessingJobState::Succeeded;
        assert!(validate_processing_page(&page, &input).is_err());
        page.items[0] = job.clone();
        page.items.push(job.clone());
        assert!(
            validate_processing_page(
                &page,
                &ListProcessingJobsInput {
                    limit: Some(2),
                    ..Default::default()
                }
            )
            .is_err()
        );
        page.items.truncate(1);
        assert!(
            validate_processing_page(
                &page,
                &ListProcessingJobsInput {
                    after: Some(job.id),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn page_validation_rejects_paths_duplicates_bad_order_nonadvancing_cursors_and_malformed_metadata()
     {
        let file = |name: &str| FileInfo {
            filename: name.into(),
            content_type: "application/pdf".into(),
            size_bytes: 42,
        };
        let page = AttachmentPage {
            files: vec![file("a.pdf"), file("z ü.pdf")],
            next_cursor: Some("z ü.pdf".into()),
        };
        assert!(validate_attachment_page(&page, 2, None).is_ok());
        let cases = [
            (
                AttachmentPage {
                    files: vec![file("../private")],
                    next_cursor: None,
                },
                2,
                None,
            ),
            (
                AttachmentPage {
                    files: vec![file(".hidden")],
                    next_cursor: None,
                },
                2,
                None,
            ),
            (
                AttachmentPage {
                    files: vec![file("a"), file("a")],
                    next_cursor: None,
                },
                2,
                None,
            ),
            (
                AttachmentPage {
                    files: vec![file("b"), file("a")],
                    next_cursor: None,
                },
                2,
                None,
            ),
            (
                AttachmentPage {
                    files: vec![file("a")],
                    next_cursor: None,
                },
                2,
                Some("a"),
            ),
            (
                AttachmentPage {
                    files: vec![file("a")],
                    next_cursor: Some("b".into()),
                },
                2,
                None,
            ),
            (
                AttachmentPage {
                    files: vec![],
                    next_cursor: Some("a".into()),
                },
                2,
                None,
            ),
            (page, 1, None),
        ];
        for (page, limit, after) in cases {
            assert_eq!(
                validate_attachment_page(&page, limit, after)
                    .unwrap_err()
                    .code,
                ErrorCode::IncompatibleVersion
            );
        }
        let mut malformed = AttachmentPage {
            files: vec![file("a")],
            next_cursor: None,
        };
        malformed.files[0].content_type = "image/png\r\nprivate".into();
        assert!(validate_attachment_page(&malformed, 2, None).is_err());
    }
}

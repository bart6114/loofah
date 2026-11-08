mod files;
mod state;
#[cfg(test)]
mod tests;
use hypr_export_core::{
    ExportAttachment, ExportInput, ExportLabels, ExportMetadata, TextFormat, Transcript,
    TranscriptItem,
};
use hypr_loofah_client::{AuthenticatedScope, RemoteClient, RemoteOperation, RemoteReply};
use hypr_loofah_core::{VaultContext, media_leases::MediaLeases};
use hypr_loofah_protocol::{
    AgentExport, ApplicationError, CaptureScope, DocumentSnapshot, DocumentTarget, ErrorCode,
    RenderedTranscriptSegment,
};
use state::ExportWork;
pub use state::SessionExportState;
use std::{path::PathBuf, sync::Arc};
use tauri::Manager;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, serde::Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum SessionExportFormat {
    Pdf,
    Txt,
    Md,
    Org,
}
impl SessionExportFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Txt => "txt",
            Self::Md => "md",
            Self::Org => "org",
        }
    }
}
#[derive(Clone, serde::Deserialize, specta::Type)]
pub struct SessionExportInput {
    pub session_id: String,
    pub target: DocumentTarget,
    pub format: SessionExportFormat,
    pub include_note: bool,
    pub include_summary: bool,
    pub include_transcript: bool,
    pub labels: ExportLabels,
}
#[derive(serde::Serialize, specta::Type)]
pub struct SessionExportResult {
    pub path: String,
}

enum Source {
    Local(Arc<VaultContext>),
    Remote {
        client: Arc<RemoteClient>,
        authority: Arc<AuthenticatedScope>,
        connection: String,
        owner: String,
        _pending: Box<dyn Send + Sync>,
    },
}
impl Source {
    async fn snapshot(
        &self,
        input: &SessionExportInput,
        cancel: CancellationToken,
    ) -> Result<ExportInput, ApplicationError> {
        let (export, segments, document) = match self {
            Self::Local(context) => {
                let export = context.agent_export(&input.session_id).await?;
                let segments = if input.include_transcript {
                    context.agent_segments(&input.session_id).await?
                } else {
                    vec![]
                };
                let document = if input.include_summary {
                    Some(
                        context
                            .read_document(&input.session_id, &input.target)
                            .await?,
                    )
                } else {
                    None
                };
                (export, segments, document)
            }
            Self::Remote {
                client, authority, ..
            } => {
                authority.authorize_session(&input.session_id).await?;
                let vault_id = authority.scope().vault_id.clone();
                let export = match execute(
                    client,
                    RemoteOperation::AgentExport {
                        vault_id: vault_id.clone(),
                        session_id: input.session_id.clone(),
                    },
                    cancel.clone(),
                )
                .await?
                {
                    RemoteReply::AgentExport(value) => value,
                    _ => return Err(unexpected()),
                };
                let segments = if input.include_transcript {
                    match execute(
                        client,
                        RemoteOperation::AgentSegments {
                            vault_id: vault_id.clone(),
                            session_id: input.session_id.clone(),
                        },
                        cancel.clone(),
                    )
                    .await?
                    {
                        RemoteReply::AgentSegments(value) => value,
                        _ => return Err(unexpected()),
                    }
                } else {
                    vec![]
                };
                let document = if input.include_summary {
                    Some(
                        match execute(
                            client,
                            RemoteOperation::Document {
                                vault_id,
                                session_id: input.session_id.clone(),
                                target: input.target.clone(),
                            },
                            cancel,
                        )
                        .await?
                        {
                            RemoteReply::Document(value) => value,
                            _ => return Err(unexpected()),
                        },
                    )
                } else {
                    None
                };
                (export, segments, document)
            }
        };
        snapshot(input, export, segments, document)
    }
}
async fn execute(
    client: &Arc<RemoteClient>,
    operation: RemoteOperation,
    cancel: CancellationToken,
) -> Result<RemoteReply, ApplicationError> {
    client
        .execute(operation, &uuid::Uuid::new_v4().to_string(), cancel)
        .await
}
fn snapshot(
    input: &SessionExportInput,
    export: AgentExport,
    segments: Vec<RenderedTranscriptSegment>,
    document: Option<DocumentSnapshot>,
) -> Result<ExportInput, ApplicationError> {
    if export.meeting.id != input.session_id {
        return Err(unexpected());
    }
    let title = if export.meeting.title.trim().is_empty() {
        input.labels.untitled.clone()
    } else {
        export.meeting.title
    };
    let mut participants = std::collections::BTreeSet::new();
    let items = segments
        .into_iter()
        .map(|segment| {
            if !segment.speaker_label.is_empty() {
                participants.insert(segment.speaker_label.clone());
            }
            TranscriptItem {
                speaker: Some(segment.speaker_label),
                text: segment.text,
            }
        })
        .collect();
    Ok(ExportInput {
        enhanced_md: document
            .and_then(|value| value.document)
            .map(|doc| doc.markdown)
            .unwrap_or_default(),
        note_md: if input.include_note {
            export.meeting.note.map(|note| note.markdown)
        } else {
            None
        },
        transcript: input.include_transcript.then_some(Transcript { items }),
        metadata: Some(ExportMetadata {
            title,
            created_at: export.meeting.created_at,
            participants: participants.into_iter().collect(),
            duration: None,
        }),
        attachments: vec![],
    })
}

#[tauri::command]
#[specta::specta]
pub async fn application_export_session<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    request_id: String,
    input: SessionExportInput,
) -> Result<SessionExportResult, ApplicationError> {
    let source = Source::Local(crate::application::context(&app)?);
    let work = app.state::<SessionExportState>().admit(
        window.label(),
        &request_id,
        CancellationToken::new(),
    )?;
    launch(app, source, work, input).await
}
#[tauri::command]
#[specta::specta]
pub async fn remote_export_session<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    scope: CaptureScope,
    request_id: String,
    input: SessionExportInput,
) -> Result<SessionExportResult, ApplicationError> {
    let (client, profile, cancel, pending) = app
        .state::<crate::remote::RemoteState>()
        .admit_export(&connection_id, window.label(), &request_id)?;
    let work = app
        .state::<SessionExportState>()
        .admit(window.label(), &request_id, cancel)?;
    let on_drop = work.cancel.clone().drop_guard();
    let authority = client
        .bind_authenticated_scope(profile, scope, work.cancel.clone())
        .await?;
    let source = Source::Remote {
        client,
        authority,
        connection: connection_id,
        owner: window.label().into(),
        _pending: pending,
    };
    let result = launch(app, source, work, input).await;
    drop(on_drop);
    result
}
#[tauri::command]
#[specta::specta]
pub fn session_export_cancel<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    request_id: String,
) {
    app.state::<SessionExportState>()
        .cancel(window.label(), &request_id);
}
async fn launch<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    source: Source,
    work: ExportWork,
    input: SessionExportInput,
) -> Result<SessionExportResult, ApplicationError> {
    let _on_drop = work.cancel.clone().drop_guard();
    tokio::spawn(async move { run(app, source, work, input).await })
        .await
        .map_err(worker)?
}
async fn run<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    source: Source,
    work: ExportWork,
    input: SessionExportInput,
) -> Result<SessionExportResult, ApplicationError> {
    validate(&input)?;
    work.check()?;
    let mut render = source.snapshot(&input, work.cancel.clone()).await?;
    work.check()?;
    let text_bytes = render.enhanced_md.len()
        + render.note_md.as_ref().map_or(0, String::len)
        + render.transcript.as_ref().map_or(0, |transcript| {
            transcript
                .items
                .iter()
                .map(|item| item.text.len() + item.speaker.as_ref().map_or(0, String::len))
                .sum::<usize>()
        });
    if text_bytes > 8 * 1024 * 1024 {
        return Err(budget());
    }
    let private = tempfile::Builder::new()
        .prefix("loofah-export-")
        .tempdir()
        .map_err(io_error)?;
    let mut remote_assets = Vec::new();
    let local_media = MediaLeases::default();
    if matches!(input.format, SessionExportFormat::Pdf) {
        let images = files::image_references(&render)?;
        let mut remaining = 64 * 1024 * 1024u64;
        for (src, name) in images {
            work.check()?;
            let (path, bytes) = match &source {
                Source::Local(context) => {
                    files::local_attachment(
                        &local_media,
                        context.clone(),
                        &input.session_id,
                        &name,
                        private.path(),
                        remaining.min(32 * 1024 * 1024),
                        &work.cancel,
                    )
                    .await?
                }
                Source::Remote {
                    connection,
                    owner,
                    authority,
                    ..
                } => {
                    authority.check_current().await?;
                    let asset = crate::remote::attachments::export_attachment(
                        &app.state::<crate::remote::RemoteState>(),
                        connection,
                        owner,
                        crate::remote::attachments::AttachmentTarget {
                            vault_id: authority.scope().vault_id.clone(),
                            session_id: input.session_id.clone(),
                            attachment_id: name,
                        },
                        private.path().into(),
                        work.cancel.clone(),
                        remaining.min(32 * 1024 * 1024),
                    )
                    .await?;
                    let info = (asset.path.clone(), asset.metadata.size_bytes);
                    remote_assets.push(asset);
                    info
                }
            };
            remaining = remaining.checked_sub(bytes).ok_or_else(budget)?;
            render.attachments.push(ExportAttachment {
                src,
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    local_media.shutdown().await;
    work.check()?;
    let title = render
        .metadata
        .as_ref()
        .map(|meta| meta.title.clone())
        .unwrap_or_default();
    let format = input.format;
    let labels = input.labels;
    let bytes = tokio::task::spawn_blocking(move || match format {
        SessionExportFormat::Pdf => hypr_export_core::render_pdf_with_labels(&render, &labels)
            .map_err(|cause| {
                tracing::error!(%cause,"PDF export failed");
                error(ErrorCode::Internal, "Could not render the PDF")
            }),
        SessionExportFormat::Md => {
            Ok(hypr_export_core::render_text(&render, TextFormat::Md, &labels).into_bytes())
        }
        SessionExportFormat::Txt => {
            Ok(hypr_export_core::render_text(&render, TextFormat::Txt, &labels).into_bytes())
        }
        SessionExportFormat::Org => {
            Ok(hypr_export_core::render_text(&render, TextFormat::Org, &labels).into_bytes())
        }
    })
    .await
    .map_err(worker)??;
    work.check()?;
    for asset in &remote_assets {
        asset.check_current(&work.cancel)?;
    }
    let root = crate::remote::attachments::download_root(&app)?;
    let stage =
        tokio::task::spawn_blocking(move || files::stage(root, &title, format.extension(), bytes))
            .await
            .map_err(worker)??;
    let guard = match &source {
        Source::Remote { authority, .. } => {
            authority.authorize_session(&input.session_id).await?;
            Some(authority.guard_commit().await?)
        }
        Source::Local(context) => {
            if !Arc::ptr_eq(context, &crate::application::context(&app)?) {
                return Err(error(
                    ErrorCode::Conflict,
                    "The original Local vault is no longer selected",
                ));
            }
            None
        }
    };
    // The guard and all snapshot owners move into the real filesystem commit.
    tokio::task::spawn_blocking(move || {
        let _owners = (source, private, remote_assets);
        work.check()?;
        if let Some(guard) = &guard {
            guard.check_current()?;
        }
        let path = files::publish(stage)?;
        Ok(SessionExportResult {
            path: path.to_string_lossy().into_owned(),
        })
    })
    .await
    .map_err(worker)?
}
fn validate(input: &SessionExportInput) -> Result<(), ApplicationError> {
    if !input.include_note && !input.include_summary && !input.include_transcript {
        return Err(error(
            ErrorCode::Validation,
            "Select at least one export section",
        ));
    }
    for label in [
        &input.labels.untitled,
        &input.labels.created,
        &input.labels.participants,
        &input.labels.duration,
        &input.labels.metadata,
        &input.labels.note,
        &input.labels.summary,
        &input.labels.transcript,
    ] {
        if label.len() > 4096 {
            return Err(budget());
        }
    }
    Ok(())
}
fn error(code: ErrorCode, message: &str) -> ApplicationError {
    hypr_loofah_client::failure(code, message)
}
fn budget() -> ApplicationError {
    error(
        ErrorCode::ResourceLimit,
        "Export exceeds the bounded image or document budget",
    )
}
fn unexpected() -> ApplicationError {
    error(
        ErrorCode::Conflict,
        "The export source did not match the original session",
    )
}
fn io_error(cause: std::io::Error) -> ApplicationError {
    tracing::error!(%cause,"session export I/O failed");
    error(ErrorCode::Internal, "Could not save the session export")
}
fn worker(cause: tokio::task::JoinError) -> ApplicationError {
    tracing::error!(%cause,"session export worker stopped");
    error(ErrorCode::Internal, "Session export stopped")
}

use std::sync::{Arc, Weak};

use hypr_loofah_protocol::ProcessingJobFailure;
use hypr_template_app::{
    EnhanceSystem, EnhanceTagContext, EnhanceUser, Segment, Session, Transcript,
};
use hypr_vault_write::{
    StoreError,
    processing_source::{ProcessingSource, SourceReadLimits},
};
use tokio_util::sync::CancellationToken;

use super::{
    JobFuture, JobWork,
    generation::{SummaryPrompt, SummarySource},
};
use crate::{VaultContext, settings::AppConfig};

mod images;

const LOCAL_OWNER: &str = "00000000-0000-0000-0000-000000000000";

#[derive(Clone, Copy)]
pub enum SummaryIdentity {
    LocalOwner,
    JobMember,
}

pub struct VaultSummarySource {
    context: Weak<VaultContext>,
    identity: SummaryIdentity,
    include_images: bool,
    limits: SourceReadLimits,
    allow_custom_templates: bool,
    system_snapshot: Option<EnhanceSystem>,
}

impl VaultSummarySource {
    pub fn new(
        context: &Arc<VaultContext>,
        identity: SummaryIdentity,
        include_images: bool,
    ) -> Self {
        Self {
            context: Arc::downgrade(context),
            identity,
            include_images,
            limits: SourceReadLimits::default(),
            allow_custom_templates: true,
            system_snapshot: None,
        }
    }

    pub fn with_limits(mut self, limits: SourceReadLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_custom_templates(mut self, allow: bool) -> Self {
        self.allow_custom_templates = allow;
        self
    }

    pub(super) fn with_system_snapshot(mut self, system: EnhanceSystem) -> Self {
        self.system_snapshot = Some(system);
        self
    }
}

impl SummarySource for VaultSummarySource {
    fn prepare(
        &self,
        work: JobWork,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<SummaryPrompt, ProcessingJobFailure>> {
        let context = self.context.upgrade();
        let (identity, include_images, limits) = (self.identity, self.include_images, self.limits);
        let allow_custom_templates = self.allow_custom_templates;
        let system_snapshot = self.system_snapshot.clone();
        Box::pin(async move {
            let context = context.ok_or(ProcessingJobFailure::SourceUnavailable)?;
            let self_id = match identity {
                SummaryIdentity::LocalOwner => LOCAL_OWNER.into(),
                SummaryIdentity::JobMember => uuid::Uuid::parse_str(&work.principal)
                    .map_err(|_| ProcessingJobFailure::AccessDenied)?
                    .to_string(),
            };
            // Dropping a caller must not release the vault lease while source I/O still runs.
            tokio::spawn(async move {
                if cancel.is_cancelled() {
                    return Err(ProcessingJobFailure::Interrupted);
                }
                let id = work.request.session_id;
                context
                    .validate(&id)
                    .map_err(|_| ProcessingJobFailure::SourceUnavailable)?;
                context
                    .ensure_capture_processing_ready(&id)
                    .map_err(|_| ProcessingJobFailure::SourceUnavailable)?;
                context
                    .store
                    .flush_transcript(&id)
                    .await
                    .map_err(|_| ProcessingJobFailure::Storage)?;
                if cancel.is_cancelled() {
                    return Err(ProcessingJobFailure::Interrupted);
                }
                let source = context
                    .store
                    .read_processing_source(
                        context.document_operation_root().as_deref(),
                        &id,
                        limits,
                    )
                    .await
                    .map_err(source_error)?
                    .ok_or(ProcessingJobFailure::SourceUnavailable)?;
                if cancel.is_cancelled() {
                    return Err(ProcessingJobFailure::Interrupted);
                }
                let config = if system_snapshot.is_none() {
                    Some(
                        context
                            .settings
                            .read_current()
                            .await
                            .map_err(|_| ProcessingJobFailure::SourceUnavailable)?,
                    )
                } else {
                    None
                };
                let custom_prompt = system_snapshot
                    .as_ref()
                    .map(|system| system.prompt_override.as_str())
                    .unwrap_or_else(|| config.as_ref().unwrap().auto_summary_prompt.as_str());
                if !allow_custom_templates && !custom_prompt.trim().is_empty() {
                    return Err(ProcessingJobFailure::AccessDenied);
                }
                let legacy_owner =
                    matches!(identity, SummaryIdentity::LocalOwner).then_some(LOCAL_OWNER);
                let (system, user, references) = tokio::task::spawn_blocking(move || {
                    let user = prepare_user(source, &self_id, legacy_owner)?;
                    SummaryPrompt::validate_source(&user)?;
                    let references = if include_images {
                        images::references(&[&user.pre_meeting_memo, &user.post_meeting_memo])?
                    } else {
                        Vec::new()
                    };
                    let system =
                        system_snapshot.unwrap_or_else(|| system_from_config(config.unwrap()));
                    Ok::<_, ProcessingJobFailure>((system, user, references))
                })
                .await
                .map_err(|_| ProcessingJobFailure::SourceUnavailable)??;
                if cancel.is_cancelled() {
                    return Err(ProcessingJobFailure::Interrupted);
                }
                let images =
                    images::collect(context.clone(), &id, references, cancel.clone()).await?;
                if cancel.is_cancelled() {
                    return Err(ProcessingJobFailure::Interrupted);
                }
                tokio::task::spawn_blocking(move || SummaryPrompt::render(system, user, images))
                    .await
                    .map_err(|_| ProcessingJobFailure::SourceUnavailable)?
            })
            .await
            .map_err(|_| ProcessingJobFailure::SourceUnavailable)?
        })
    }
}

pub(super) fn system_from_config(config: AppConfig) -> EnhanceSystem {
    EnhanceSystem {
        language: (!config.ai_language.is_empty()).then_some(config.ai_language),
        prompt_override: if config.auto_summary_prompt.trim().is_empty() {
            String::new()
        } else {
            config.auto_summary_prompt
        },
    }
}

fn source_error(cause: StoreError) -> ProcessingJobFailure {
    match cause {
        StoreError::ResourceLimit(_) => ProcessingJobFailure::ResourceLimit,
        _ => ProcessingJobFailure::SourceUnavailable,
    }
}

fn prepare_user(
    source: ProcessingSource,
    self_id: &str,
    legacy_owner: Option<&str>,
) -> Result<EnhanceUser, ProcessingJobFailure> {
    let note = source.note_markdown.unwrap_or_default();
    if !has_note_text(&note)
        && !source.transcripts.iter().any(|transcript| {
            transcript
                .words
                .iter()
                .any(|word| !word.text.trim().is_empty())
        })
    {
        return Err(ProcessingJobFailure::SourceUnavailable);
    }
    let segments = hypr_agent_access::render_summary_transcripts(
        &source.transcripts,
        &source.people,
        Some(self_id),
        legacy_owner,
    );
    let pre_meeting_memo = source
        .transcripts
        .first()
        .map(|transcript| transcript.memo_md.clone())
        .unwrap_or_default();
    let transcripts = if segments.is_empty() {
        Vec::new()
    } else {
        vec![Transcript {
            started_at: source
                .transcripts
                .iter()
                .map(|transcript| transcript.started_at.round().max(0.0) as u64)
                .min(),
            ended_at: source
                .transcripts
                .iter()
                .map(|transcript| {
                    transcript
                        .ended_at
                        .unwrap_or(transcript.started_at)
                        .round()
                        .max(0.0) as u64
                })
                .max(),
            segments: segments
                .into_iter()
                .filter(|segment| !segment.words.is_empty())
                .map(|segment| Segment {
                    speaker: segment.speaker_label,
                    text: segment.text,
                    is_current_user: Some(segment.is_current_user == Some(true)),
                })
                .collect(),
        }]
    };
    Ok(EnhanceUser {
        tag_context: EnhanceTagContext {
            available: source.tag_context.available,
            attached: source.tag_context.attached,
            dismissed: source.tag_context.dismissed,
        },
        session: Session {
            title: (!source.meta.title.is_empty()).then_some(source.meta.title),
            started_at: None,
            ended_at: None,
            event: None,
        },
        participants: Vec::new(),
        transcripts,
        pre_meeting_memo,
        post_meeting_memo: note,
    })
}

fn has_note_text(note: &str) -> bool {
    let note = note.trim();
    if note.is_empty() {
        return false;
    }
    let parsed = if note.starts_with('{') {
        serde_json::from_str(note).map_err(|_| ())
    } else {
        hypr_tiptap::md_to_tiptap_json(note).map_err(|_| ())
    };
    let Ok(doc) = parsed else {
        return true;
    };
    if doc.get("type").and_then(serde_json::Value::as_str) != Some("doc")
        || !doc.get("content").is_some_and(serde_json::Value::is_array)
    {
        return true;
    }
    let mut nodes = vec![&doc];
    while let Some(node) = nodes.pop() {
        if let Some(text) = node
            .get("text")
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
        {
            if text
                .chars()
                .any(|character| !character.is_whitespace() && character != '\u{feff}')
            {
                return true;
            }
        } else if let Some(content) = node.get("content").and_then(serde_json::Value::as_array) {
            nodes.extend(content);
        }
    }
    false
}

#[cfg(test)]
mod tests;

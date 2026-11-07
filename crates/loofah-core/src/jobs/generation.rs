use std::{pin::Pin, sync::Arc};

use futures_util::{Stream, StreamExt};
use hypr_loofah_protocol::ProcessingJobFailure;
use hypr_template_app::{EnhanceSystem, EnhanceUser, Template};
use hypr_vault_write::ScoredTagSuggestion;
use tokio_util::sync::CancellationToken;

use super::{GeneratedDocument, JobFuture, JobGenerator, JobWork};

const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const FOOTER_START: &str = "<loofah-tags";
const FOOTER_OPEN: &str = "<loofah-tags>";
const FOOTER_CLOSE: &str = "</loofah-tags>";
const MAX_FOOTER_UNITS: usize = 4096;
const VALIDATION_FEEDBACK: &str = "Output must start with a markdown h1 heading (# Title).";
const IMAGE_CONTEXT_NOTE: &str = "Attached note images are included as visual context. Use visible text, diagrams, screenshots, and other image content when it materially improves the summary.";
const CONCISION_NOTE: &str = "Keep the summary concise and proportional to the source. Preserve concrete decisions and explicit actions; do not pad with filler.";

#[derive(Debug, Clone)]
pub struct SummaryImage {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

#[derive(Debug, Clone)]
pub struct SummaryPrompt {
    pub system: String,
    pub prompt: String,
    pub images: Vec<SummaryImage>,
    pub custom_prompt: bool,
}

impl SummaryPrompt {
    pub fn render(
        system: EnhanceSystem,
        user: EnhanceUser,
        images: Vec<SummaryImage>,
    ) -> Result<Self, ProcessingJobFailure> {
        let custom_prompt = !system.prompt_override.trim().is_empty();
        Self::validate_source(&user)?;
        let system = hypr_template_app::render_enhance_system_bounded(&system, 256 * 1024)
            .map_err(|cause| match cause {
                hypr_template_app::Error::ResourceLimit => ProcessingJobFailure::ResourceLimit,
                _ => ProcessingJobFailure::SourceUnavailable,
            })?;
        let mut prompt = hypr_template_app::render(Template::EnhanceUser(Box::new(user)))
            .map_err(|_| ProcessingJobFailure::SourceUnavailable)?;
        if !images.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(IMAGE_CONTEXT_NOTE);
        }
        prompt.push_str("\n\n");
        prompt.push_str(CONCISION_NOTE);
        let prepared = Self {
            system,
            prompt,
            images,
            custom_prompt,
        };
        prepared.validate()?;
        Ok(prepared)
    }

    pub(super) fn validate_source(user: &EnhanceUser) -> Result<(), ProcessingJobFailure> {
        let mut budget = SourceBudget(0);
        serde_json::to_writer(&mut budget, user).map_err(|_| ProcessingJobFailure::ResourceLimit)
    }

    fn validate(&self) -> Result<(), ProcessingJobFailure> {
        if self.system.trim().is_empty() || self.prompt.trim().is_empty() {
            return Err(ProcessingJobFailure::SourceUnavailable);
        }
        if self.system.len() > 256 * 1024
            || self.prompt.len() > 16 * 1024 * 1024
            || self.images.len() > 10
            || self
                .images
                .iter()
                .any(|image| image.bytes.len() > 128 * 1024)
            || self
                .images
                .iter()
                .map(|image| image.bytes.len())
                .sum::<usize>()
                > 768 * 1024
        {
            return Err(ProcessingJobFailure::ResourceLimit);
        }
        if self.images.iter().any(|image| {
            image.bytes.is_empty()
                || !matches!(
                    image.mime_type.as_str(),
                    "image/jpeg" | "image/png" | "image/gif" | "image/webp"
                )
        }) {
            return Err(ProcessingJobFailure::SourceUnavailable);
        }
        Ok(())
    }
}

struct SourceBudget(usize);

impl std::io::Write for SourceBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        if self.0 > 16 * 1024 * 1024 {
            return Err(std::io::Error::other("Summary source is too large"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub trait SummarySource: Send + Sync + 'static {
    fn prepare(
        &self,
        work: JobWork,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<SummaryPrompt, ProcessingJobFailure>>;
}

#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub system: String,
    pub prompt: String,
    pub images: Vec<SummaryImage>,
    pub max_output_tokens: u32,
    pub max_retries: u32,
}

#[derive(Debug, Clone)]
pub enum ModelChunk {
    Text(String),
    Finish { truncated: bool },
}

pub type ModelStream = Pin<Box<dyn Stream<Item = Result<ModelChunk, ProcessingJobFailure>> + Send>>;

/// Streams own their processing resources until EOF, including after cancellation.
pub trait TextModel: Send + Sync + 'static {
    fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<ModelStream, ProcessingJobFailure>>;
}

pub struct SummaryGenerator {
    source: Arc<dyn SummarySource>,
    model: Arc<dyn TextModel>,
}

impl SummaryGenerator {
    pub fn new(source: Arc<dyn SummarySource>, model: Arc<dyn TextModel>) -> Self {
        Self { source, model }
    }
}

impl JobGenerator for SummaryGenerator {
    fn generate(
        &self,
        work: JobWork,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<GeneratedDocument, ProcessingJobFailure>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(ProcessingJobFailure::Interrupted);
            }
            let prepared = self.source.prepare(work, cancel.child_token()).await?;
            prepared.validate()?;
            let mut feedback = None;
            for attempt in 0..2 {
                if cancel.is_cancelled() {
                    return Err(ProcessingJobFailure::Interrupted);
                }
                let mut prompt = prepared.prompt.clone();
                if let Some(feedback) = feedback {
                    prompt.push_str("\n\nIMPORTANT: Previous attempt failed. ");
                    prompt.push_str(feedback);
                }
                let attempt_cancel = cancel.child_token();
                let mut stream = self
                    .model
                    .stream(
                        ModelRequest {
                            system: prepared.system.clone(),
                            prompt,
                            images: prepared.images.clone(),
                            max_output_tokens: 8192,
                            max_retries: 4,
                        },
                        attempt_cancel.clone(),
                    )
                    .await?;
                let mut output = Output::default();
                let mut validated = false;
                let mut retry = false;
                let mut failure = None;
                let mut received = 0usize;
                let mut thinking = ReasoningFilter::new("<thinking>", "</thinking>");
                let mut think = ReasoningFilter::new("<think>", "</think>");
                while let Some(chunk) = stream.next().await {
                    if retry || failure.is_some() {
                        attempt_cancel.cancel();
                        continue;
                    }
                    match chunk {
                        Err(cause) => {
                            failure = Some(cause);
                            attempt_cancel.cancel();
                        }
                        Ok(ModelChunk::Finish { truncated }) => {
                            if output.finished {
                                failure = Some(ProcessingJobFailure::ProviderFailed);
                                attempt_cancel.cancel();
                            }
                            output.truncated |= truncated;
                            output.finished = true;
                        }
                        Ok(ModelChunk::Text(text)) => {
                            received = received.saturating_add(text.len());
                            if received > MAX_OUTPUT_BYTES + MAX_FOOTER_UNITS * 4 {
                                failure = Some(ProcessingJobFailure::InvalidOutput);
                                attempt_cancel.cancel();
                                continue;
                            }
                            if output.finished {
                                failure = Some(ProcessingJobFailure::ProviderFailed);
                                attempt_cancel.cancel();
                                continue;
                            }
                            if output.push(&think.push(&thinking.push(&text))).is_err() {
                                failure = Some(ProcessingJobFailure::InvalidOutput);
                                attempt_cancel.cancel();
                                continue;
                            }
                            if !validated && !cancel.is_cancelled() {
                                let units = output.body.trim().encode_utf16().count();
                                if units >= 10 {
                                    if !prepared.custom_prompt
                                        && !valid_heading(&output.body)
                                        && attempt == 0
                                    {
                                        retry = true;
                                        feedback = Some(VALIDATION_FEEDBACK);
                                        attempt_cancel.cancel();
                                    } else {
                                        validated = true;
                                    }
                                } else if output.body.encode_utf16().count() >= 30 {
                                    validated = true;
                                }
                            }
                        }
                    }
                }
                if cancel.is_cancelled() && (retry || failure.is_some() || !output.finished) {
                    return Err(ProcessingJobFailure::Interrupted);
                }
                if let Some(failure) = failure {
                    return Err(failure);
                }
                if !retry {
                    // A large generated body must not monopolize the async executor.
                    return tokio::task::spawn_blocking(move || output.finish())
                        .await
                        .map_err(|_| ProcessingJobFailure::InvalidOutput)?;
                }
            }
            Err(ProcessingJobFailure::InvalidOutput)
        })
    }
}

struct ReasoningFilter {
    opening: &'static str,
    closing: &'static str,
    pending: String,
    reasoning: bool,
    has_text: bool,
    after_switch: bool,
}

impl ReasoningFilter {
    fn new(opening: &'static str, closing: &'static str) -> Self {
        Self {
            opening,
            closing,
            pending: String::new(),
            reasoning: false,
            has_text: false,
            after_switch: false,
        }
    }

    fn push(&mut self, text: &str) -> String {
        let mut input = std::mem::take(&mut self.pending);
        input.push_str(text);
        let mut rest = input.as_str();
        let mut output = String::new();
        loop {
            let tag = if self.reasoning {
                self.closing
            } else {
                self.opening
            };
            if let Some(start) = rest.find(tag) {
                self.publish(&rest[..start], &mut output);
                rest = &rest[start + tag.len()..];
                self.reasoning = !self.reasoning;
                self.after_switch = true;
            } else {
                let held = (1..tag.len())
                    .rev()
                    .find(|length| rest.ends_with(&tag[..*length]))
                    .unwrap_or(0);
                let split = rest.len() - held;
                self.publish(&rest[..split], &mut output);
                self.pending.push_str(&rest[split..]);
                return output;
            }
        }
    }

    fn publish(&mut self, text: &str, output: &mut String) {
        if !text.is_empty() && !self.reasoning {
            if self.after_switch && self.has_text {
                output.push('\n');
            }
            output.push_str(text);
            self.has_text = true;
            self.after_switch = false;
        }
    }
}

fn valid_heading(text: &str) -> bool {
    text[text.find('#').unwrap_or(0)..].trim().starts_with("# ")
}

#[derive(Default)]
struct Output {
    body: String,
    pending: String,
    footer: Option<String>,
    oversized: bool,
    truncated: bool,
    finished: bool,
    received: usize,
}

impl Output {
    fn push(&mut self, text: &str) -> Result<(), ()> {
        self.received = self.received.saturating_add(text.len());
        if text.len() > MAX_OUTPUT_BYTES || self.received > MAX_OUTPUT_BYTES + MAX_FOOTER_UNITS * 4
        {
            return Err(());
        }
        if let Some(footer) = &mut self.footer {
            if !self.oversized {
                if footer.encode_utf16().count() + text.encode_utf16().count() > MAX_FOOTER_UNITS {
                    self.oversized = true;
                    footer.clear();
                } else {
                    footer.push_str(text);
                }
            }
            return Ok(());
        }
        self.pending.push_str(text);
        if let Some(start) = self.pending.find(FOOTER_START) {
            self.body.push_str(&self.pending[..start]);
            let footer = &self.pending[start..];
            self.oversized = footer.encode_utf16().count() > MAX_FOOTER_UNITS;
            self.footer = Some(if self.oversized {
                String::new()
            } else {
                footer.into()
            });
            self.pending.clear();
        } else {
            let held = (1..FOOTER_START.len())
                .rev()
                .find(|length| self.pending.ends_with(&FOOTER_START[..*length]))
                .unwrap_or(0);
            let split = self.pending.len() - held;
            self.body.push_str(&self.pending[..split]);
            self.pending = self.pending[split..].into();
        }
        if self.body.len() > MAX_OUTPUT_BYTES {
            return Err(());
        }
        Ok(())
    }

    fn finish(mut self) -> Result<GeneratedDocument, ProcessingJobFailure> {
        if !self.finished {
            return Err(ProcessingJobFailure::ProviderFailed);
        }
        if self.pending == "<" {
            self.body.push('<');
        }
        let suggested_tags = if self.oversized || self.truncated {
            None
        } else {
            self.footer.as_deref().and_then(parse_footer)
        };
        let output = GeneratedDocument {
            markdown: normalize_bullets(&self.body).trim().into(),
            suggested_tags,
        };
        super::result::validate_output(&output).map_err(|_| ProcessingJobFailure::InvalidOutput)?;
        Ok(output)
    }
}

fn parse_footer(footer: &str) -> Option<Vec<ScoredTagSuggestion>> {
    let value = footer.strip_prefix(FOOTER_OPEN)?;
    let (json, tail) = value.split_once(FOOTER_CLOSE)?;
    if !tail.trim().is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let tags: Vec<ScoredTagSuggestion> =
        serde_json::from_value(value.as_object()?.get("tags")?.clone()).ok()?;
    let output = GeneratedDocument {
        markdown: "# Tags".into(),
        suggested_tags: Some(tags),
    };
    super::result::validate_output(&output).ok()?;
    output.suggested_tags
}

fn normalize_bullets(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let body = line.trim_start_matches([' ', '\t']);
            if let Some(body) = body.strip_prefix("• ") {
                format!("{}- {body}", &line[..line.len() - body.len() - "• ".len()])
            } else {
                line.into()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;

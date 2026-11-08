use std::sync::Arc;

use futures_util::StreamExt;
use hypr_loofah_protocol::ProcessingJobFailure;
use hypr_template_app::{Template, TitleSystem, TitleUser};
use tokio_util::sync::CancellationToken;

use super::{
    JobFuture,
    generation::{ModelChunk, ModelRequest, ReasoningFilter, TextModel},
};

const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_TITLE_BYTES: usize = 4096;

pub struct TitleGenerator {
    model: Arc<dyn TextModel>,
    language: Option<String>,
    temperature: Option<f64>,
}

impl TitleGenerator {
    pub fn new(
        model: Arc<dyn TextModel>,
        language: Option<String>,
        temperature: Option<f64>,
    ) -> Self {
        Self {
            model,
            language,
            temperature,
        }
    }

    /// Hosts retain their admission and vault lease through completion; dropped callers
    /// request cancellation while the owned model operation drains through actual EOF.
    pub fn generate(
        &self,
        enhanced_markdown: String,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<Option<String>, ProcessingJobFailure>> {
        let model = self.model.clone();
        let language = self.language.clone();
        let temperature = self.temperature;
        Box::pin(async move {
            let stopping = cancel.child_token();
            let _stop_on_drop = stopping.clone().drop_guard();
            tokio::spawn(async move {
                if stopping.is_cancelled() {
                    return Err(ProcessingJobFailure::Interrupted);
                }
                let request = tokio::task::spawn_blocking(move || {
                    prepare(enhanced_markdown, language, temperature)
                })
                .await
                .map_err(|_| ProcessingJobFailure::SourceUnavailable)??;
                if stopping.is_cancelled() {
                    return Err(ProcessingJobFailure::Interrupted);
                }
                let mut stream = match model.stream(request, stopping.child_token()).await {
                    Ok(stream) => stream,
                    Err(_) if cancel.is_cancelled() => {
                        return Err(ProcessingJobFailure::Interrupted);
                    }
                    Err(cause) => return Err(cause),
                };
                let mut output = String::new();
                let mut received = 0usize;
                let mut finished = false;
                let mut failure = None;
                let mut thinking = ReasoningFilter::new("<thinking>", "</thinking>");
                let mut think = ReasoningFilter::new("<think>", "</think>");
                while let Some(chunk) = stream.next().await {
                    if stopping.is_cancelled() && failure.is_none() {
                        failure = Some(ProcessingJobFailure::Interrupted);
                    }
                    if failure.is_some() {
                        stopping.cancel();
                        continue;
                    }
                    match chunk {
                        Err(cause) => failure = Some(cause),
                        Ok(ModelChunk::Finish { .. }) if finished => {
                            failure = Some(ProcessingJobFailure::ProviderFailed);
                        }
                        Ok(ModelChunk::Finish { .. }) => finished = true,
                        Ok(ModelChunk::Text(_)) if finished => {
                            failure = Some(ProcessingJobFailure::ProviderFailed);
                        }
                        Ok(ModelChunk::Text(text)) => {
                            received = received.saturating_add(text.len());
                            if received > MAX_OUTPUT_BYTES {
                                failure = Some(ProcessingJobFailure::InvalidOutput);
                            } else {
                                let filtered = think.push(&thinking.push(&text));
                                let filtered = if output.is_empty() {
                                    filtered.trim_start_matches(is_js_whitespace)
                                } else {
                                    &filtered
                                };
                                let end = filtered
                                    .char_indices()
                                    .rfind(|(_, character)| !is_js_whitespace(*character))
                                    .map(|(offset, character)| {
                                        output.len() + offset + character.len_utf8()
                                    })
                                    .unwrap_or(0);
                                if end > MAX_TITLE_BYTES {
                                    failure = Some(ProcessingJobFailure::InvalidOutput);
                                } else {
                                    output.push_str(filtered);
                                }
                            }
                        }
                    }
                    if failure.is_some() {
                        stopping.cancel();
                    }
                }
                if cancel.is_cancelled() {
                    return Err(ProcessingJobFailure::Interrupted);
                }
                if let Some(cause) = failure {
                    return Err(cause);
                }
                if !finished {
                    return Err(ProcessingJobFailure::ProviderFailed);
                }
                normalize_candidate(&output)
            })
            .await
            .map_err(|_| ProcessingJobFailure::Interrupted)?
        })
    }
}

pub(super) fn normalize_candidate(output: &str) -> Result<Option<String>, ProcessingJobFailure> {
    if output.len() > MAX_OUTPUT_BYTES {
        return Err(ProcessingJobFailure::InvalidOutput);
    }
    let candidate = trim_candidate(output);
    if candidate.is_empty() || candidate == "<EMPTY>" {
        return Ok(None);
    }
    if candidate.len() > MAX_TITLE_BYTES || candidate.chars().any(char::is_control) {
        return Err(ProcessingJobFailure::InvalidOutput);
    }
    Ok(Some(candidate.to_owned()))
}

fn trim_candidate(value: &str) -> &str {
    // String.trim includes BOM but excludes NEL, unlike Rust's Unicode whitespace set.
    value.trim_matches(is_js_whitespace)
}

fn is_js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'..='\u{000d}'
                | '\u{0020}'
                | '\u{00a0}'
                | '\u{1680}'
                | '\u{2000}'..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

pub fn deterministic_title_temperature(provider: &str, model_id: &str) -> Option<f64> {
    let normalized = model_id.to_lowercase().replace('.', "-");
    let name = normalized.rsplit('/').next().unwrap_or(&normalized);
    let deprecated = (provider.starts_with("anthropic") || normalized.starts_with("anthropic/"))
        && ["opus", "sonnet", "haiku"].iter().any(|family| {
            name.strip_prefix(&format!("claude-{family}-4-8"))
                .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('-'))
        });
    if provider == "chatgpt_subscription" || deprecated {
        None
    } else {
        Some(0.0)
    }
}

fn prepare(
    enhanced_markdown: String,
    language: Option<String>,
    temperature: Option<f64>,
) -> Result<ModelRequest, ProcessingJobFailure> {
    if enhanced_markdown.len() > MAX_SOURCE_BYTES
        || language.as_ref().is_some_and(|value| value.len() > 128)
    {
        return Err(ProcessingJobFailure::ResourceLimit);
    }
    if language
        .as_ref()
        .is_some_and(|value| value.chars().any(char::is_control))
        || temperature.is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
    {
        return Err(ProcessingJobFailure::SourceUnavailable);
    }
    let system = hypr_template_app::render(Template::TitleSystem(TitleSystem { language }))
        .map_err(|_| ProcessingJobFailure::SourceUnavailable)?;
    let prompt = hypr_template_app::render(Template::TitleUser(TitleUser {
        enhanced_note: enhanced_markdown,
    }))
    .map_err(|_| ProcessingJobFailure::SourceUnavailable)?;
    if prompt.len() > MAX_SOURCE_BYTES || system.len() > 256 * 1024 {
        return Err(ProcessingJobFailure::ResourceLimit);
    }
    Ok(ModelRequest {
        system,
        prompt,
        images: vec![],
        max_output_tokens: 128,
        max_retries: 4,
        temperature,
    })
}

#[cfg(test)]
mod tests;

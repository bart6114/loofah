use std::{io::Write, net::IpAddr};

use base64::Engine;
use hypr_loofah_protocol::ProcessingJobFailure;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::json;
use url::{Host, Url};

use super::{HttpModelConfig, HttpProvider, ModelRequest};

pub(super) fn destination(
    config: &HttpModelConfig,
) -> Result<(Url, HeaderMap), ProcessingJobFailure> {
    let invalid = || ProcessingJobFailure::ProviderUnavailable;
    if config.model.trim().is_empty()
        || config.model.len() > 512
        || config.model.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    let mut url = Url::parse(&config.base_url).map_err(|_| invalid())?;
    let loopback = match url.host() {
        Some(Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
        Some(Host::Ipv6(ip)) => IpAddr::V6(ip).is_loopback(),
        Some(Host::Domain("localhost")) => true,
        _ => false,
    };
    if !(url.scheme() == "https"
        || url.scheme() == "http" && (loopback || config.allow_insecure_http))
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    let mut segments = url.path_segments_mut().map_err(|_| invalid())?;
    segments.pop_if_empty();
    match config.provider {
        HttpProvider::OpenAi => {
            segments.push("responses");
        }
        HttpProvider::AzureOpenAi => {
            segments.extend(["v1", "responses"]);
        }
        HttpProvider::Anthropic => {
            segments.push("messages");
        }
        HttpProvider::Google => {
            let (namespace, model) = config
                .model
                .split_once('/')
                .unwrap_or(("models", &config.model));
            if !matches!(namespace, "models" | "tunedModels")
                || model.is_empty()
                || model.contains('/')
                || matches!(model, "." | "..")
            {
                return Err(invalid());
            }
            segments
                .push(namespace)
                .push(&format!("{model}:streamGenerateContent"));
        }
        _ => {
            segments.extend(["chat", "completions"]);
        }
    }
    drop(segments);
    match config.provider {
        HttpProvider::AzureOpenAi => {
            url.query_pairs_mut().append_pair("api-version", "v1");
        }
        HttpProvider::Google => {
            url.query_pairs_mut().append_pair("alt", "sse");
        }
        _ => {}
    }
    let mut headers = HeaderMap::new();
    let key = config
        .api_key
        .as_deref()
        .filter(|key| !key.trim().is_empty());
    if key.is_none()
        && !matches!(
            config.provider,
            HttpProvider::Ollama | HttpProvider::ChatCompletions
        )
    {
        return Err(invalid());
    }
    let mut insert = |name: &'static str, value: &str| -> Result<(), ProcessingJobFailure> {
        let mut value = HeaderValue::from_str(value).map_err(|_| invalid())?;
        value.set_sensitive(true);
        headers.insert(name, value);
        Ok(())
    };
    if let Some(key) = key {
        match config.provider {
            HttpProvider::Anthropic => insert("x-api-key", key)?,
            HttpProvider::Google => insert("x-goog-api-key", key)?,
            HttpProvider::AzureOpenAi => insert("api-key", key)?,
            HttpProvider::AzureAi => {
                insert("api-key", key)?;
                insert("authorization", &format!("Bearer {key}"))?;
            }
            HttpProvider::Ollama => {}
            _ => insert("authorization", &format!("Bearer {key}"))?,
        }
    }
    if matches!(config.provider, HttpProvider::Anthropic) {
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    }
    if matches!(config.provider, HttpProvider::Ollama) {
        headers.insert(
            "origin",
            HeaderValue::from_str(&url.origin().ascii_serialization()).map_err(|_| invalid())?,
        );
    }
    Ok((url, headers))
}

pub(super) fn body(
    provider: HttpProvider,
    model: String,
    request: ModelRequest,
) -> Result<Vec<u8>, ProcessingJobFailure> {
    if request
        .temperature
        .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
    {
        return Err(ProcessingJobFailure::SourceUnavailable);
    }
    if request.system.is_empty() || request.prompt.is_empty() || request.max_output_tokens == 0 {
        return Err(ProcessingJobFailure::SourceUnavailable);
    }
    if request.system.len() > 256 * 1024
        || request.prompt.len() > 16 * 1024 * 1024
        || request.images.len() > 10
        || request.max_output_tokens > 128_000
        || request
            .images
            .iter()
            .any(|image| image.bytes.len() > 128 * 1024)
        || request
            .images
            .iter()
            .map(|image| image.bytes.len())
            .sum::<usize>()
            > 768 * 1024
    {
        return Err(ProcessingJobFailure::ResourceLimit);
    }
    if request.images.iter().any(|image| {
        image.bytes.is_empty()
            || !matches!(
                image.mime_type.as_str(),
                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
            )
    }) {
        return Err(ProcessingJobFailure::SourceUnavailable);
    }
    let mut parts = vec![match provider {
        HttpProvider::OpenAi | HttpProvider::AzureOpenAi => {
            json!({"type":"input_text","text":request.prompt})
        }
        HttpProvider::Google => json!({"text":request.prompt}),
        _ => json!({"type":"text","text":request.prompt}),
    }];
    for image in request.images {
        let encoded = base64::engine::general_purpose::STANDARD.encode(image.bytes);
        parts.push(match provider {
            HttpProvider::OpenAi | HttpProvider::AzureOpenAi => json!({"type":"input_image","image_url":format!("data:{};base64,{encoded}",image.mime_type)}),
            HttpProvider::Anthropic => json!({"type":"image","source":{"type":"base64","media_type":image.mime_type,"data":encoded}}),
            HttpProvider::Google => json!({"inlineData":{"mimeType":image.mime_type,"data":encoded}}),
            _ => json!({"type":"image_url","image_url":{"url":format!("data:{};base64,{encoded}",image.mime_type)}}),
        });
    }
    let omit_temperature = matches!(provider, HttpProvider::OpenAi | HttpProvider::AzureOpenAi)
        && (model.starts_with("o1")
            || model.starts_with("o3")
            || model.starts_with("o4-mini")
            || model.starts_with("gpt-5") && !model.starts_with("gpt-5-chat"));
    let mut body = match provider {
        // A stateless summary never needs a provider-stored conversation.
        HttpProvider::OpenAi | HttpProvider::AzureOpenAi => {
            json!({"model":model,"instructions":request.system,"input":[{"role":"user","content":parts}],"max_output_tokens":request.max_output_tokens,"stream":true,"store":false})
        }
        HttpProvider::Anthropic => {
            json!({"model":model,"system":request.system,"messages":[{"role":"user","content":parts}],"max_tokens":request.max_output_tokens,"stream":true})
        }
        HttpProvider::Google if model.to_lowercase().starts_with("gemma-") => {
            let mut combined = vec![json!({"text":request.system})];
            combined.extend(parts);
            json!({"contents":[{"role":"user","parts":combined}],"generationConfig":{"maxOutputTokens":request.max_output_tokens}})
        }
        HttpProvider::Google => {
            json!({"systemInstruction":{"parts":[{"text":request.system}]},"contents":[{"role":"user","parts":parts}],"generationConfig":{"maxOutputTokens":request.max_output_tokens}})
        }
        _ => {
            let content = if parts.len() == 1 {
                parts.remove(0)["text"].take()
            } else {
                serde_json::Value::Array(parts)
            };
            json!({"model":model,"messages":[{"role":"system","content":request.system},{"role":"user","content":content}],"max_tokens":request.max_output_tokens,"stream":true})
        }
    };
    if let Some(temperature) = request.temperature.filter(|_| !omit_temperature) {
        if matches!(provider, HttpProvider::Google) {
            body["generationConfig"]["temperature"] = json!(temperature);
        } else {
            body["temperature"] = json!(temperature);
        }
    }
    let mut output = Body(Vec::new());
    serde_json::to_writer(&mut output, &body).map_err(|_| ProcessingJobFailure::ResourceLimit)?;
    Ok(output.0)
}

struct Body(Vec<u8>);
impl Write for Body {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > 24 * 1024 * 1024 {
            return Err(std::io::Error::other("Model request exceeds limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

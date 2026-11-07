use std::time::Duration;

use futures_util::StreamExt;
use hypr_loofah_protocol::ProcessingJobFailure;
use reqwest::{Client, StatusCode};
use tokio_util::sync::CancellationToken;

use super::{
    JobFuture,
    generation::{ModelRequest, ModelStream, TextModel},
};

pub mod connections;
mod request;
mod sse;
mod wire;

#[derive(Clone, Copy)]
pub enum HttpProvider {
    OpenAi,
    AzureOpenAi,
    AzureAi,
    ChatCompletions,
    Ollama,
    Anthropic,
    Google,
}

/// Host configuration only. Keys and endpoints never come from job requests.
pub struct HttpModelConfig {
    pub provider: HttpProvider,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub allow_insecure_http: bool,
}

pub struct HttpModel {
    client: Client,
    config: HttpModelConfig,
    endpoint: url::Url,
    headers: reqwest::header::HeaderMap,
    retry_delay: Duration,
}

impl HttpModel {
    pub fn validate_config(config: &HttpModelConfig) -> Result<(), ProcessingJobFailure> {
        request::destination(config).map(|_| ())
    }

    pub fn new(config: HttpModelConfig) -> Result<Self, ProcessingJobFailure> {
        Self::with_timeouts(config, Duration::from_secs(90), Duration::from_secs(900))
    }

    fn with_timeouts(
        config: HttpModelConfig,
        idle: Duration,
        total: Duration,
    ) -> Result<Self, ProcessingJobFailure> {
        let (endpoint, headers) = request::destination(&config)?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(idle)
            .timeout(total)
            .build()
            .map_err(|_| ProcessingJobFailure::ProviderUnavailable)?;
        Ok(Self {
            client,
            config,
            endpoint,
            headers,
            retry_delay: Duration::from_secs(2),
        })
    }
}

impl TextModel for HttpModel {
    fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<ModelStream, ProcessingJobFailure>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(ProcessingJobFailure::Interrupted);
            }
            let retries = request.max_retries.min(4);
            let provider = self.config.provider;
            let model = self.config.model.clone();
            let body = tokio::task::spawn_blocking(move || request::body(provider, model, request))
                .await
                .map_err(|_| ProcessingJobFailure::ProviderFailed)??;
            let request = self
                .client
                .post(self.endpoint.clone())
                .headers(self.headers.clone())
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .body(body)
                .build()
                .map_err(|_| ProcessingJobFailure::ProviderUnavailable)?;
            let mut attempt = 0;
            let response = loop {
                let sending = self.client.execute(
                    request
                        .try_clone()
                        .ok_or(ProcessingJobFailure::ProviderUnavailable)?,
                );
                let response = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(ProcessingJobFailure::Interrupted),
                    response = sending => response.map_err(|cause| {
                        if cause.is_connect() { ProcessingJobFailure::ProviderUnavailable }
                        else { ProcessingJobFailure::Interrupted }
                    })?,
                };
                // Only an explicit rate-limit rejection is automatically repeated. A failed
                // connection after dispatch may already have incurred work or provider charges.
                if response.status() == StatusCode::TOO_MANY_REQUESTS && attempt < retries {
                    let delay = match response.headers().get("retry-after") {
                        Some(value) => Duration::from_secs(
                            value
                                .to_str()
                                .ok()
                                .and_then(|value| value.parse::<u64>().ok())
                                .ok_or(ProcessingJobFailure::ProviderUnavailable)?,
                        ),
                        None => self.retry_delay.saturating_mul(1 << attempt),
                    };
                    if delay > Duration::from_secs(60) {
                        return Err(ProcessingJobFailure::ProviderUnavailable);
                    }
                    drop(response);
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Err(ProcessingJobFailure::Interrupted),
                        _ = tokio::time::sleep(delay) => {},
                    }
                    attempt += 1;
                    continue;
                }
                break response;
            };
            if !response.status().is_success() {
                return Err(
                    if matches!(response.status().as_u16(), 401 | 403 | 429 | 503) {
                        ProcessingJobFailure::ProviderUnavailable
                    } else {
                        ProcessingJobFailure::ProviderFailed
                    },
                );
            }
            let event_stream = response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"));
            if !event_stream {
                return Err(ProcessingJobFailure::ProviderFailed);
            }
            Ok(Box::pin(async_stream::try_stream! {
                let mut bytes = response.bytes_stream();
                let mut decoder = sse::Decoder::default();
                let mut wire = wire::Wire::new(provider);
                let mut received = 0usize;
                let mut events = 0usize;
                loop {
                    let chunk = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => Err(ProcessingJobFailure::Interrupted),
                        chunk = bytes.next() => Ok(chunk),
                    }?;
                    let Some(chunk) = chunk else { break };
                    let chunk = chunk.map_err(|_| ProcessingJobFailure::Interrupted)?;
                    received = received.saturating_add(chunk.len());
                    if received > 32 * 1024 * 1024 {
                        Err(ProcessingJobFailure::ResourceLimit)?;
                    }
                    for byte in chunk {
                        if let Some(event) = decoder.push(byte)? {
                            events += 1;
                            if events > 100_000 { Err(ProcessingJobFailure::ResourceLimit)?; }
                            for chunk in wire.event(event)? { yield chunk; }
                        }
                    }
                }
                decoder.end()?;
                wire.end()?;
            }) as ModelStream)
        })
    }
}

#[cfg(test)]
mod tests;

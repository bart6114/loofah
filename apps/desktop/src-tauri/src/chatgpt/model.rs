use std::sync::Arc;

use base64::{Engine, engine::general_purpose::STANDARD};
use hypr_loofah_core::jobs::{
    JobFuture,
    generation::{ModelChunk, ModelRequest, ModelStream, TextModel},
};
use hypr_loofah_protocol::ProcessingJobFailure;
use tokio_util::sync::CancellationToken;

use super::*;

pub struct SummaryModel<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    model: String,
}

impl<R: tauri::Runtime> SummaryModel<R> {
    pub fn new(app: tauri::AppHandle<R>, model: String) -> Self {
        Self { app, model }
    }
}

impl<R: tauri::Runtime> TextModel for SummaryModel<R> {
    fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<ModelStream, ProcessingJobFailure>> {
        Box::pin(async move {
            let request = prepare_request(self.model.clone(), request)?;
            if cancel.is_cancelled() {
                return Err(ProcessingJobFailure::Interrupted);
            }
            let state = self.app.state::<ChatgptState>();
            let account_cancel = state.shared_cancel.lock().unwrap().clone();
            // Drain startup and account probes: dropping an RPC would abandon its
            // pending response and release admission before the runtime actually stops.
            let client = state.client(&self.app).await.map_err(map_error)?;
            if cancel.is_cancelled() || account_cancel.is_cancelled() {
                return Err(ProcessingJobFailure::Interrupted);
            }
            if !client.allowed_models.contains(&request.model) {
                return Err(ProcessingJobFailure::ProviderUnavailable);
            }
            let account = client.account().await.map_err(map_error)?;
            if cancel.is_cancelled() || account_cancel.is_cancelled() {
                return Err(ProcessingJobFailure::Interrupted);
            }
            if account.is_none() {
                return Err(ProcessingJobFailure::ProviderUnavailable);
            }
            Ok(generation_stream(client, request, cancel, account_cancel))
        })
    }
}

fn prepare_request(
    model: String,
    request: ModelRequest,
) -> Result<ChatgptGeneration, ProcessingJobFailure> {
    if request.temperature.is_some() {
        return Err(ProcessingJobFailure::SourceUnavailable);
    }
    if request.system.len() > 256 * 1024
        || request.prompt.len() > 16 * 1024 * 1024
        || request.max_output_tokens == 0
        || request.max_output_tokens > 128_000
        || request.images.len() > 10
        || request.images.iter().any(|image| {
            image.bytes.is_empty()
                || image.bytes.len() > 128 * 1024
                || !matches!(
                    image.mime_type.as_str(),
                    "image/jpeg" | "image/png" | "image/gif" | "image/webp"
                )
        })
        || request
            .images
            .iter()
            .map(|image| image.bytes.len())
            .sum::<usize>()
            > 768 * 1024
    {
        return Err(ProcessingJobFailure::ResourceLimit);
    }
    if request.system.trim().is_empty() || request.prompt.trim().is_empty() {
        return Err(ProcessingJobFailure::SourceUnavailable);
    }
    Ok(ChatgptGeneration {
        request_id: uuid::Uuid::new_v4().to_string(),
        model,
        system: request.system,
        prompt: request.prompt,
        images: request
            .images
            .into_iter()
            .map(|image| {
                format!(
                    "data:{};base64,{}",
                    image.mime_type,
                    STANDARD.encode(image.bytes)
                )
            })
            .collect(),
        max_output_tokens: Some(request.max_output_tokens),
    })
}

fn generation_stream(
    client: Arc<Client>,
    request: ChatgptGeneration,
    cancel: CancellationToken,
    account_cancel: CancellationToken,
) -> ModelStream {
    let stopping = cancel.child_token();
    let guard = stopping.clone().drop_guard();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let task = tokio::spawn(async move {
        if account_cancel.is_cancelled() {
            stopping.cancel();
        }
        let (cancel_tx, mut cancel_rx) = watch::channel(stopping.is_cancelled());
        let mut received = 0usize;
        let generation = run_generation_with(
            &client,
            &request,
            |event| {
                let chunk = match event {
                    ChatgptEvent::Text { delta } => {
                        received = received.saturating_add(delta.len());
                        if received > 4 * 1024 * 1024 + 16 * 1024 {
                            Err(failure("resource_limit", "Summary output is too large."))
                        } else {
                            Ok(ModelChunk::Text(delta))
                        }
                    }
                    ChatgptEvent::Complete { .. } => Ok(ModelChunk::Finish { truncated: false }),
                    ChatgptEvent::Error { error } => Err(error),
                };
                let tx = tx.clone();
                let stopping = stopping.clone();
                async move {
                    let chunk = chunk?;
                    tokio::select! {
                        biased;
                        _ = stopping.cancelled() => Err(failure("cancelled", "Generation cancelled.")),
                        result = tx.send(chunk) => result.map_err(|_| failure("cancelled", "Generation cancelled.")),
                    }
                }
            },
            &mut cancel_rx,
        );
        tokio::pin!(generation);
        let result = tokio::select! {
            result = &mut generation => result,
            _ = stopping.cancelled() => {
                let _ = cancel_tx.send(true);
                generation.await
            }
            _ = account_cancel.cancelled() => {
                stopping.cancel();
                let _ = cancel_tx.send(true);
                generation.await
            }
        };
        // The producer owns the runtime request through interruption and unsubscribe.
        // EOF is observed only after the actual cleanup has stopped using the client.
        result.map_err(map_error)
    });
    Box::pin(async_stream::stream! {
        let _guard = guard;
        while let Some(chunk) = rx.recv().await {
            yield Ok(chunk);
        }
        match task.await {
            Ok(Ok(())) => {},
            Ok(Err(error)) => yield Err(error),
            Err(_) => yield Err(ProcessingJobFailure::Interrupted),
        }
    })
}

fn map_error(error: ChatgptError) -> ProcessingJobFailure {
    match error.code.as_str() {
        "cancelled" | "runtime" | "timeout" => ProcessingJobFailure::Interrupted,
        "resource_limit" => ProcessingJobFailure::ResourceLimit,
        "runtime_missing"
        | "runtime_incompatible"
        | "configuration"
        | "authentication"
        | "keychain"
        | "model"
        | "access" => ProcessingJobFailure::ProviderUnavailable,
        _ => ProcessingJobFailure::ProviderFailed,
    }
}

#[cfg(test)]
mod tests;

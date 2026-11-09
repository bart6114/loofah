use std::collections::BTreeMap;

use hypr_loofah_protocol::ProcessingJobFailure;
use serde_json::Value;

use super::{HttpProvider, sse::Event};
use crate::jobs::generation::ModelChunk;

pub(super) struct Wire {
    provider: HttpProvider,
    finished: bool,
    closed: bool,
    started: bool,
    blocks: BTreeMap<u64, String>,
    stop: Option<bool>,
    text_bytes: usize,
}

fn failed() -> ProcessingJobFailure {
    ProcessingJobFailure::ProviderFailed
}

impl Wire {
    pub fn new(provider: HttpProvider) -> Self {
        Self {
            provider,
            finished: false,
            closed: false,
            started: false,
            blocks: BTreeMap::new(),
            stop: None,
            text_bytes: 0,
        }
    }

    pub fn event(&mut self, event: Event) -> Result<Vec<ModelChunk>, ProcessingJobFailure> {
        if event.data == "[DONE]" {
            if matches!(
                self.provider,
                HttpProvider::OpenAi
                    | HttpProvider::AzureOpenAi
                    | HttpProvider::Anthropic
                    | HttpProvider::Google
            ) || !self.finished
                || self.closed
            {
                return Err(failed());
            }
            self.closed = true;
            return Ok(Vec::new());
        }
        let value: Value = serde_json::from_str(&event.data).map_err(|_| failed())?;
        if !value.is_object()
            || value.get("error").is_some_and(|v| !v.is_null())
            || event.name == "error"
        {
            return Err(failed());
        }
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        if !event.name.is_empty() && !kind.is_empty() && event.name != kind {
            return Err(failed());
        }
        match self.provider {
            HttpProvider::OpenAi | HttpProvider::AzureOpenAi => self.responses(&value, kind),
            HttpProvider::Anthropic => self.anthropic(&value, kind),
            HttpProvider::Google => self.google(&value),
            _ => self.chat(&value),
        }
    }

    fn text(
        &mut self,
        text: &str,
        chunks: &mut Vec<ModelChunk>,
    ) -> Result<(), ProcessingJobFailure> {
        if self.finished || self.closed {
            return Err(failed());
        }
        self.text_bytes = self.text_bytes.saturating_add(text.len());
        if self.text_bytes > 4 * 1024 * 1024 + 16 * 1024 {
            return Err(ProcessingJobFailure::ResourceLimit);
        }
        if !text.is_empty() {
            chunks.push(ModelChunk::Text(text.to_owned()));
        }
        Ok(())
    }

    fn finish(
        &mut self,
        truncated: bool,
        chunks: &mut Vec<ModelChunk>,
    ) -> Result<(), ProcessingJobFailure> {
        if self.finished || self.closed {
            return Err(failed());
        }
        self.finished = true;
        chunks.push(ModelChunk::Finish { truncated });
        Ok(())
    }

    fn chat(&mut self, value: &Value) -> Result<Vec<ModelChunk>, ProcessingJobFailure> {
        if self.closed {
            return Err(failed());
        }
        let mut chunks = Vec::new();
        let choices = value
            .get("choices")
            .and_then(Value::as_array)
            .ok_or_else(failed)?;
        if choices.len() > 1 {
            return Err(failed());
        }
        for choice in choices {
            if choice.get("index").and_then(Value::as_u64) != Some(0) {
                return Err(failed());
            }
            let delta = choice
                .get("delta")
                .and_then(Value::as_object)
                .ok_or_else(failed)?;
            if delta
                .get("refusal")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
                || delta
                    .get("tool_calls")
                    .is_some_and(|v| v.as_array().is_none_or(|v| !v.is_empty()) && !v.is_null())
                || delta.get("function_call").is_some_and(|v| !v.is_null())
            {
                return Err(failed());
            }
            if let Some(text) = delta.get("content").filter(|v| !v.is_null()) {
                self.text(text.as_str().ok_or_else(failed)?, &mut chunks)?;
            }
            if let Some(reason) = choice.get("finish_reason").filter(|v| !v.is_null()) {
                self.finish(
                    match reason.as_str() {
                        Some("stop") => false,
                        Some("length") => true,
                        _ => return Err(failed()),
                    },
                    &mut chunks,
                )?;
            }
        }
        Ok(chunks)
    }

    fn responses(
        &mut self,
        value: &Value,
        kind: &str,
    ) -> Result<Vec<ModelChunk>, ProcessingJobFailure> {
        let mut chunks = Vec::new();
        match kind {
            "response.output_text.delta" => self.text(
                value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or_else(failed)?,
                &mut chunks,
            )?,
            "response.refusal.delta" | "response.failed" | "error" => return Err(failed()),
            "response.output_item.added" | "response.output_item.done" => {
                if self.finished {
                    return Err(failed());
                }
                let item = value.get("item").ok_or_else(failed)?;
                if !matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("message" | "reasoning")
                ) {
                    return Err(failed());
                }
                if let Some(parts) = item.get("content").and_then(Value::as_array)
                    && parts
                        .iter()
                        .any(|part| part.get("type").and_then(Value::as_str) == Some("refusal"))
                {
                    return Err(failed());
                }
            }
            "response.completed" | "response.incomplete" => {
                let response = value.get("response").ok_or_else(failed)?;
                if response.get("error").is_some_and(|v| !v.is_null()) {
                    return Err(failed());
                }
                let truncated = kind == "response.incomplete";
                if response.get("status").and_then(Value::as_str)
                    != Some(if truncated { "incomplete" } else { "completed" })
                    || truncated
                        && response
                            .pointer("/incomplete_details/reason")
                            .and_then(Value::as_str)
                            != Some("max_output_tokens")
                {
                    return Err(failed());
                }
                if let Some(output) = response.get("output").and_then(Value::as_array) {
                    for item in output {
                        if !matches!(
                            item.get("type").and_then(Value::as_str),
                            Some("message" | "reasoning")
                        ) {
                            return Err(failed());
                        }
                        if let Some(parts) = item.get("content").and_then(Value::as_array)
                            && parts.iter().any(|part| {
                                part.get("type").and_then(Value::as_str) == Some("refusal")
                            })
                        {
                            return Err(failed());
                        }
                    }
                }
                self.finish(truncated, &mut chunks)?;
            }
            "" => return Err(failed()),
            _ => {}
        }
        Ok(chunks)
    }

    fn anthropic(
        &mut self,
        value: &Value,
        kind: &str,
    ) -> Result<Vec<ModelChunk>, ProcessingJobFailure> {
        let mut chunks = Vec::new();
        match kind {
            "ping" => {}
            "message_start" => {
                if self.started || self.finished {
                    return Err(failed());
                }
                if value
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .is_none_or(|v| !v.is_empty())
                {
                    return Err(failed());
                }
                self.started = true;
            }
            "content_block_start" => {
                if !self.started || self.stop.is_some() || self.finished || self.blocks.len() >= 128
                {
                    return Err(failed());
                }
                let index = value
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(failed)?;
                let block = value.get("content_block").ok_or_else(failed)?;
                let kind = block
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(failed)?;
                if !matches!(kind, "text" | "thinking" | "redacted_thinking")
                    || self.blocks.insert(index, kind.into()).is_some()
                {
                    return Err(failed());
                }
                if kind == "text" {
                    self.text(
                        block
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(failed)?,
                        &mut chunks,
                    )?;
                }
            }
            "content_block_delta" => {
                if !self.started || self.stop.is_some() || self.finished {
                    return Err(failed());
                }
                let index = value
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(failed)?;
                let block = self.blocks.get(&index).ok_or_else(failed)?;
                match value.pointer("/delta/type").and_then(Value::as_str) {
                    Some("text_delta") if block == "text" => self.text(
                        value
                            .pointer("/delta/text")
                            .and_then(Value::as_str)
                            .ok_or_else(failed)?,
                        &mut chunks,
                    )?,
                    Some("thinking_delta" | "signature_delta") if block == "thinking" => {}
                    _ => return Err(failed()),
                }
            }
            "content_block_stop" => {
                if self
                    .blocks
                    .remove(
                        &value
                            .get("index")
                            .and_then(Value::as_u64)
                            .ok_or_else(failed)?,
                    )
                    .is_none()
                {
                    return Err(failed());
                }
            }
            "message_delta" => {
                if !self.started || self.finished {
                    return Err(failed());
                }
                if let Some(reason) = value.pointer("/delta/stop_reason").filter(|v| !v.is_null()) {
                    if self.stop.is_some() || !self.blocks.is_empty() {
                        return Err(failed());
                    }
                    self.stop = Some(match reason.as_str() {
                        Some("end_turn" | "stop_sequence") => false,
                        Some("max_tokens") => true,
                        _ => return Err(failed()),
                    });
                }
            }
            "message_stop" => {
                if !self.blocks.is_empty() {
                    return Err(failed());
                }
                self.finish(self.stop.ok_or_else(failed)?, &mut chunks)?;
            }
            "error" | "" => return Err(failed()),
            _ => {}
        }
        Ok(chunks)
    }

    fn google(&mut self, value: &Value) -> Result<Vec<ModelChunk>, ProcessingJobFailure> {
        let mut chunks = Vec::new();
        if value
            .pointer("/promptFeedback/blockReason")
            .is_some_and(|v| !v.is_null())
        {
            return Err(failed());
        }
        let candidates = value.get("candidates").and_then(Value::as_array);
        let Some(candidates) = candidates else {
            if value.get("usageMetadata").is_some() {
                return Ok(chunks);
            }
            return Err(failed());
        };
        if candidates.len() > 1 {
            return Err(failed());
        }
        for candidate in candidates {
            if candidate
                .get("index")
                .is_some_and(|v| v.as_u64() != Some(0))
            {
                return Err(failed());
            }
            if let Some(parts) = candidate
                .pointer("/content/parts")
                .and_then(Value::as_array)
            {
                for part in parts {
                    if part.get("functionCall").is_some() || part.get("inlineData").is_some() {
                        return Err(failed());
                    }
                    if part.get("thought").and_then(Value::as_bool) == Some(true) {
                        continue;
                    }
                    if let Some(text) = part.get("text") {
                        self.text(text.as_str().ok_or_else(failed)?, &mut chunks)?;
                    }
                }
            }
            if let Some(reason) = candidate.get("finishReason") {
                self.finish(
                    match reason.as_str() {
                        Some("STOP") => false,
                        Some("MAX_TOKENS") => true,
                        _ => return Err(failed()),
                    },
                    &mut chunks,
                )?;
            }
        }
        Ok(chunks)
    }

    pub fn end(&self) -> Result<(), ProcessingJobFailure> {
        if !self.finished {
            return Err(ProcessingJobFailure::Interrupted);
        }
        Ok(())
    }
}

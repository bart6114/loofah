use std::path::Path;

use futures_util::StreamExt;
use owhisper_interface::ListenParams;
use owhisper_interface::batch_sse::{BatchSseMessage, EVENT_NAME as BATCH_EVENT, IDLE_TIMEOUT};
use owhisper_interface::batch_stream::BatchStreamEvent;
use owhisper_interface::progress::InferenceProgress;
use owhisper_interface::stream::StreamResponse;

use crate::adapter::{StreamingBatchEvent, StreamingBatchStream};
use crate::error::Error;

use super::WhisperCppAdapter;

impl WhisperCppAdapter {
    pub async fn transcribe_file_streaming(
        api_base: &str,
        params: &ListenParams,
        file_path: impl AsRef<Path>,
    ) -> Result<StreamingBatchStream, Error> {
        let path = file_path.as_ref().to_path_buf();
        tracing::info!(
            fmtr.file.path = %path.display(),
            url.full = %api_base,
            "starting_whispercpp_batch_stream"
        );

        let content_type = audio_content_type(&path);
        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| Error::AudioProcessing(format!("read failed: {e}")))?;
        let audio_duration_secs = tokio::task::spawn_blocking(move || audio_duration_secs(&path))
            .await
            .map_err(|e| Error::AudioProcessing(format!("task panicked: {e:?}")))?;

        let url = build_batch_url(api_base, params);

        let response = reqwest::Client::new()
            .post(url.as_str())
            .header("Content-Type", content_type)
            .header("Accept", "text/event-stream")
            .body(file)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::UnexpectedStatus { status, body });
        }

        Ok(batch_event_stream(
            response.bytes_stream(),
            audio_duration_secs,
        ))
    }
}

fn batch_event_stream<S, E>(stream: S, audio_duration_secs: f64) -> StreamingBatchStream
where
    S: futures_util::Stream<Item = Result<bytes::Bytes, E>> + Send + Unpin + 'static,
    E: std::fmt::Debug + Send + 'static,
{
    let event_stream = futures_util::stream::unfold(
        SseParserState::new(stream, audio_duration_secs),
        |mut state| async move {
            loop {
                if let Some(event) = state.pending_events.pop_front() {
                    return Some((event, state));
                }
                if state.finished {
                    return None;
                }

                // Keepalives and partial SSE frames indicate a healthy connection even
                // when decoding or a single inference window takes longer than the deadline.
                match tokio::time::timeout(IDLE_TIMEOUT, state.stream.next()).await {
                    Ok(Some(Ok(chunk))) => {
                        state.buffer.extend_from_slice(&chunk);
                        state.parse_buffer();
                    }
                    Ok(Some(Err(e))) => {
                        state.finished = true;
                        return Some((
                            Err(Error::WebSocket(format!("stream error: {e:?}"))),
                            state,
                        ));
                    }
                    Ok(None) => {
                        state.finished = true;
                        state.parse_buffer();
                    }
                    Err(_) => {
                        state.finished = true;
                        return Some((
                            Err(Error::WebSocket(
                                "Local transcription connection timed out waiting for SSE data."
                                    .into(),
                            )),
                            state,
                        ));
                    }
                }
            }
        },
    );
    Box::pin(event_stream)
}

fn audio_content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("wav") {
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "webm" => "audio/webm",
        _ => "application/octet-stream",
    }
}

fn audio_duration_secs(path: &Path) -> f64 {
    use hypr_audio_utils::Source;
    let Ok(source) = hypr_audio_utils::source_from_path(path) else {
        return 0.0;
    };
    if let Some(d) = source.total_duration() {
        return d.as_secs_f64();
    }
    let sample_rate = u32::from(source.sample_rate()) as f64;
    let channels = u16::from(source.channels()).max(1) as f64;
    let count = source.count() as f64;
    count / channels / sample_rate
}

fn build_batch_url(api_base: &str, params: &ListenParams) -> url::Url {
    let mut url: url::Url = api_base.parse().expect("invalid api_base URL");

    if !url.path().ends_with("/listen") {
        let path = url.path().trim_end_matches('/').to_string();
        url.set_path(&format!("{}/listen", path));
    }

    for lang in &params.languages {
        url.query_pairs_mut()
            .append_pair("language", lang.iso639().code());
    }
    if !params.keywords.is_empty() {
        for kw in &params.keywords {
            url.query_pairs_mut().append_pair("keywords", kw);
        }
    }
    if let Some(ref model) = params.model {
        url.query_pairs_mut().append_pair("model", model);
    }

    url
}

struct SseParserState<S> {
    stream: S,
    buffer: Vec<u8>,
    pending_events: std::collections::VecDeque<Result<StreamingBatchEvent, Error>>,
    audio_duration_secs: f64,
    last_percentage: f64,
    finished: bool,
}

impl<S> SseParserState<S> {
    fn new(stream: S, audio_duration_secs: f64) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            pending_events: std::collections::VecDeque::new(),
            audio_duration_secs,
            last_percentage: 0.0,
            finished: false,
        }
    }

    fn parse_buffer(&mut self) {
        while let Ok(text) = std::str::from_utf8(&self.buffer) {
            let Some(end) = text.find("\n\n") else {
                break;
            };

            let block = text[..end].to_string();
            self.buffer.drain(..end + 2);

            if let Some(event) = self.parse_sse_block(&block) {
                self.pending_events.push_back(event);
            }
        }
    }

    fn parse_sse_block(&mut self, block: &str) -> Option<Result<StreamingBatchEvent, Error>> {
        let mut event_type = String::new();
        let mut data = String::new();

        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("event:") {
                event_type = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.trim());
            } else if line.starts_with(':') {
                // comment, skip
            }
        }

        if data.is_empty() {
            return None;
        }

        match event_type.as_str() {
            BATCH_EVENT => {
                let msg: BatchSseMessage = match serde_json::from_str(&data) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(
                            raw_data = %data,
                            "failed to parse batch SSE event: {e}"
                        );
                        return None;
                    }
                };

                match msg {
                    BatchSseMessage::Progress { progress } => self.handle_progress(progress),
                    BatchSseMessage::Segment { response } => self.handle_segment(response),
                    BatchSseMessage::Result { response } => self.handle_result(response),
                    BatchSseMessage::Error { detail, .. } => {
                        tracing::error!(detail = %detail, "server returned error event");
                        Some(Err(Error::WebSocket(format!("server error: {}", detail))))
                    }
                }
            }
            "progress" => {
                let progress: InferenceProgress = match serde_json::from_str(&data) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(raw_data = %data, "failed to parse progress event: {e}");
                        return None;
                    }
                };
                self.handle_progress(progress)
            }
            "segment" => {
                let response: StreamResponse = match serde_json::from_str(&data) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(raw_data = %data, "failed to parse segment event: {e}");
                        return None;
                    }
                };
                self.handle_segment(response)
            }
            "result" => {
                let batch_response: owhisper_interface::batch::Response =
                    match serde_json::from_str(&data) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!(raw_data = %data, "failed to parse result event: {e}");
                            return Some(Err(Error::WebSocket(format!(
                                "failed to parse result: {e}"
                            ))));
                        }
                    };

                self.handle_result(batch_response)
            }
            "error" => {
                let error_data: serde_json::Value = serde_json::from_str(&data).unwrap_or_default();
                let detail = error_data
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                tracing::error!(detail = %detail, raw_data = %data, "server returned error event");
                Some(Err(Error::WebSocket(format!("server error: {}", detail))))
            }
            _ => None,
        }
    }

    fn handle_progress(
        &mut self,
        progress: InferenceProgress,
    ) -> Option<Result<StreamingBatchEvent, Error>> {
        self.last_percentage = self.last_percentage.max(progress.percentage);

        Some(Ok(BatchStreamEvent::Progress {
            percentage: progress.percentage,
            partial_text: progress.partial_text,
        }))
    }

    fn handle_segment(
        &mut self,
        response: StreamResponse,
    ) -> Option<Result<StreamingBatchEvent, Error>> {
        let segment_end = match &response {
            StreamResponse::TranscriptResponse {
                start, duration, ..
            } => start + duration,
            _ => 0.0,
        };

        let percentage = if self.audio_duration_secs > 0.0 {
            (segment_end / self.audio_duration_secs).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.last_percentage = self.last_percentage.max(percentage);

        let event = match response {
            StreamResponse::TranscriptResponse { .. } => BatchStreamEvent::Segment {
                response,
                percentage: self.last_percentage,
            },
            StreamResponse::TerminalResponse {
                request_id,
                created,
                duration,
                channels,
            } => BatchStreamEvent::Terminal {
                request_id,
                created,
                duration,
                channels,
            },
            StreamResponse::ErrorResponse {
                error_code,
                error_message,
                provider,
            } => BatchStreamEvent::Error {
                error_code,
                error_message,
                provider,
            },
            other => BatchStreamEvent::Segment {
                response: other,
                percentage: self.last_percentage,
            },
        };

        Some(Ok(event))
    }

    fn handle_result(
        &mut self,
        batch_response: owhisper_interface::batch::Response,
    ) -> Option<Result<StreamingBatchEvent, Error>> {
        Some(Ok(BatchStreamEvent::Result {
            response: batch_response,
        }))
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    fn result_bytes() -> bytes::Bytes {
        let message = BatchSseMessage::Result {
            response: owhisper_interface::batch::Response {
                metadata: serde_json::json!({}),
                results: owhisper_interface::batch::Results { channels: vec![] },
            },
        };
        format!(
            "event: batch\ndata: {}\n\n",
            serde_json::to_string(&message).unwrap()
        )
        .into()
    }

    #[tokio::test(start_paused = true)]
    async fn keepalives_allow_slow_inference_without_fake_progress() {
        let start = tokio::time::Instant::now();
        let chunks = futures_util::stream::unfold(0, |index| async move {
            tokio::time::sleep(owhisper_interface::batch_sse::KEEP_ALIVE_INTERVAL).await;
            let bytes = match index {
                0..=4 => bytes::Bytes::from_static(b":\n\n"),
                5 => result_bytes(),
                _ => return None,
            };
            Some((Ok::<_, std::io::Error>(bytes), index + 1))
        });
        let mut events = batch_event_stream(Box::pin(chunks), 1.0);
        assert!(matches!(
            events.next().await,
            Some(Ok(BatchStreamEvent::Result { .. }))
        ));
        assert!(start.elapsed() > IDLE_TIMEOUT);
        assert!(events.next().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn partial_frames_reset_connection_idle_timeout() {
        let result = result_bytes();
        let chunks = futures_util::stream::unfold((result, 0), |(bytes, offset)| async move {
            if offset >= bytes.len() {
                return None;
            }
            tokio::time::sleep(owhisper_interface::batch_sse::KEEP_ALIVE_INTERVAL).await;
            let end = (offset + 20).min(bytes.len());
            Some((
                Ok::<_, std::io::Error>(bytes.slice(offset..end)),
                (bytes, end),
            ))
        });
        let start = tokio::time::Instant::now();
        let mut events = batch_event_stream(Box::pin(chunks), 1.0);
        assert!(matches!(
            events.next().await,
            Some(Ok(BatchStreamEvent::Result { .. }))
        ));
        assert!(start.elapsed() > IDLE_TIMEOUT);
    }

    #[tokio::test(start_paused = true)]
    async fn silent_connection_times_out_once() {
        let chunks = futures_util::stream::pending::<Result<bytes::Bytes, std::io::Error>>();
        let mut events = batch_event_stream(chunks, 1.0);
        let error = events.next().await.unwrap().unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(events.next().await.is_none());
    }

    #[test]
    fn local_batch_preserves_languages_and_dictionary_terms() {
        let params = ListenParams {
            model: Some("whisper-large-v3".into()),
            languages: vec!["nl".parse().unwrap(), "en".parse().unwrap()],
            keywords: vec!["Kubernetes".into(), "ingress controller".into()],
            ..Default::default()
        };
        let url = build_batch_url("http://127.0.0.1:1234/v1", &params);
        assert_eq!(url.path(), "/v1/listen");
        let pairs = url.query_pairs().collect::<Vec<_>>();
        assert_eq!(
            pairs
                .iter()
                .filter(|(key, _)| key == "language")
                .map(|(_, value)| value.as_ref())
                .collect::<Vec<_>>(),
            ["nl", "en"]
        );
        assert_eq!(
            pairs
                .iter()
                .filter(|(key, _)| key == "keywords")
                .map(|(_, value)| value.as_ref())
                .collect::<Vec<_>>(),
            ["Kubernetes", "ingress controller"]
        );
    }
}

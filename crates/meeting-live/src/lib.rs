mod state;
#[cfg(unix)]
pub mod transport;

use serde::{Deserialize, Serialize};
pub use state::LiveState;
use std::path::{Path, PathBuf};

pub const PROTOCOL_VERSION: u32 = 1;
pub const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
pub const MAX_FRAME: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveError {
    pub code: String,
    pub message: String,
}

impl LiveError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl std::fmt::Display for LiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for LiveError {}
pub type Result<T> = std::result::Result<T, LiveError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub channel: String,
    pub vault: PathBuf,
}
impl Identity {
    pub fn new(channel: &str, vault: &Path) -> Result<Self> {
        Ok(Self {
            channel: channel.into(),
            vault: vault
                .canonicalize()
                .map_err(|e| LiveError::new("vault_not_found", e.to_string()))?,
        })
    }
}

pub fn cli_channel() -> &'static str {
    let command = std::env::args_os().next();
    channel_for_command(
        command
            .as_deref()
            .and_then(|p| Path::new(p).file_name())
            .and_then(|p| p.to_str()),
    )
}

pub fn channel_for_command(name: Option<&str>) -> &'static str {
    match name {
        Some("loof-dev" | "loofah-dev" | "fmtr-dev") => "dev",
        Some("loof-staging" | "loofah-staging" | "fmtr-staging") => "staging",
        _ => "stable",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NudgeInput {
    pub text: String,
    #[serde(default)]
    pub recording_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TranscriptInput {
    pub recording_id: String,
    #[serde(default)]
    pub cursor: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    Status,
    Transcript(TranscriptInput),
    Nudge(NudgeInput),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u32,
    pub identity: Identity,
    pub request: Request,
}
impl Envelope {
    pub fn validate(&self, expected: &Identity) -> Result<()> {
        if self.version != PROTOCOL_VERSION {
            return Err(LiveError::new(
                "incompatible_protocol",
                "Update Loofah desktop and CLI together",
            ));
        }
        if &self.identity != expected {
            return Err(LiveError::new(
                "identity_mismatch",
                "Desktop vault or application channel does not match",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Response {
    Status(Status),
    Transcript(TranscriptPage),
    Nudge(NudgeReceipt),
    Error(LiveError),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub active_meeting: Option<ActiveMeeting>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveMeeting {
    pub meeting_id: String,
    pub recording_id: String,
    pub title: String,
    pub state: RecordingState,
    pub live_transcription_available: bool,
    pub nudges_available: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    Recording,
    Finalizing,
    Stopped,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Word {
    pub id: String,
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub channel: i32,
    pub speaker_index: Option<i32>,
    pub speaker_label: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    Upsert { word: Word },
    Remove { id: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptPage {
    pub meeting_id: String,
    pub recording_id: String,
    pub state: RecordingState,
    pub live_transcription_available: bool,
    pub changes: Vec<Change>,
    pub cursor: String,
    pub has_more: bool,
    pub reset: bool,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NudgeReceipt {
    pub nudge_id: String,
    pub recording_id: String,
}

pub fn normalize_nudge(text: &str) -> Result<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() || text.chars().count() > 200 || text.chars().any(char::is_control) {
        return Err(LiveError::new(
            "invalid_input",
            "Nudge text must contain 1–200 printable characters",
        ));
    }
    Ok(text)
}

pub async fn call(identity: Identity, request: Request) -> Result<Response> {
    #[cfg(target_os = "macos")]
    {
        let envelope = Envelope {
            version: PROTOCOL_VERSION,
            identity,
            request,
        };
        match tokio::time::timeout(REQUEST_TIMEOUT, transport::call(envelope)).await {
            Ok(Ok(Response::Error(error))) => Err(error),
            Ok(result) => result,
            Err(_) => Err(LiveError::new(
                "delivery_timeout",
                "Desktop request timed out; a nudge may already have been displayed. Do not retry automatically",
            )),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (identity, request);
        Err(LiveError::new(
            "unsupported_platform",
            "Live meeting tools require the macOS desktop app",
        ))
    }
}

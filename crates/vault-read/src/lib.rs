//! Read-only access to the on-disk vault layout: shared file-format types plus
//! scanning/parsing helpers. The desktop store (`apps/desktop/src-tauri/src/session_store`)
//! owns every write path and reuses the types and pure parse/render functions from here;
//! `crates/agent-access` (loof CLI/MCP) reads vaults exclusively through this crate.

pub mod enhanced;
pub mod layout;
pub mod meta;
pub mod paths;
pub mod people;
pub mod reserved;
pub mod strip;
pub mod summary;
pub mod tags;
pub mod tasks;
pub mod transcript;

pub use enhanced::{ENHANCED_KINDS, EnhancedDoc, parse_enhanced_file, render_enhanced_file};
pub use layout::{
    SessionDirKind, SessionDiscovery, SessionDiscoveryError, SessionLocation, SessionLookupError,
    classify_session_dir, discover_sessions, discover_sessions_with_progress, find_session,
    has_session_boundary,
};
pub use meta::{SessionMeta, TagSuggestionItem, TagSuggestionState, TagSuggestionStatus};
pub use people::{Person, read_people};
pub use reserved::{
    SESSION_OWNED_DIRS, SESSION_OWNED_FILES, SESSION_PEAKS_FILE, SESSION_TRANSIENT_FILES,
    is_session_owned_name,
};
pub use strip::strip_leading_frontmatter;
pub use tags::normalize_tag_name;
pub use tasks::{TaskItem, TasksFile};

pub use hypr_fs_format::{
    TranscriptJson, TranscriptSpeakerHint, TranscriptWithData, TranscriptWord,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(String),
    #[error("parse error: {0}")]
    Parse(String),
}

pub type Result<T> = std::result::Result<T, Error>;

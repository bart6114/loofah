#![forbid(unsafe_code)]

pub mod events;
pub mod jobs;
pub mod processing_settings;
pub mod search;
pub mod settings;
pub mod startup;
mod vault;
pub mod vault_watch;
pub use vault::VaultContext;
mod runtime;
pub use runtime::VaultRuntime;
mod agent;
mod attachment_upload;
mod attachments;
mod content;
mod media;
pub mod media_leases;
mod metadata;
mod note_editor;
mod operations;
mod session_cleanup;
mod session_summaries;
mod tasks;

#![forbid(unsafe_code)]

pub mod events;
pub mod search;
pub mod settings;
pub mod startup;
mod vault;
pub mod vault_watch;
pub use vault::VaultContext;
mod runtime;
pub use runtime::VaultRuntime;
mod agent;
mod content;
mod metadata;
mod note_editor;
mod operations;
mod tasks;

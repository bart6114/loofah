mod cache;
mod engine;
mod query;
pub mod schema;
mod snapshot;
mod source;
pub mod tokenizer;
mod types;
pub use cache::*;
pub use engine::{matching_range, search};
pub use schema::build_schema;
pub use source::plain_text;
pub use source::{CachedSession, Fingerprint, fingerprints};
pub use tokenizer::register_tokenizers;
pub use types::*;

#[cfg(target_os = "macos")]
mod history;

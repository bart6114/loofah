//! Append artifacts are prepared from a pinned, immutable source. These helpers
//! grant no publication authority: Store must still check the original recording
//! selection, checksum, layout and raw transcript revision at compound commit.

mod audio;
mod transcript;

pub use audio::{NormalizedAppendAudio, concatenate, normalize_baseline};
pub use transcript::merge_transcript;

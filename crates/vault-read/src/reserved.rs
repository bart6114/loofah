//! The session-directory ownership contract.
//!
//! Inside `sessions/<id>/` the app owns exactly the names listed here; **every other
//! file is a user attachment** and must be ignored by scans, indexing, sync, and
//! cleanup. Nothing may enumerate a session directory and claim unknown files as
//! app content -- readers address app files by exact name only.
//!
//! Dot-prefixed names are never content in either direction: the app's atomic-write
//! machinery stages `.tmp-<pid>-<nonce>-<name>` siblings, and platforms drop files
//! like `.DS_Store`.

/// App-owned files addressed by exact name directly in a session directory.
///
/// - `_meta.json` -- session identity + metadata; its presence is what makes a
///   directory a session.
/// - `notes.md` -- the user's note (canonical since the `_memo.md` rename).
/// - `_memo.md` -- pre-rename note file; read as a fallback, migrated to trash on
///   the next note write.
/// - `transcript.json`, `tasks.json` -- transcript and session tasks.
/// - `audio.mp3` / `audio.wav` / `audio.ogg` -- the recording (one of), plus its
///   `audio.peaks.json` waveform cache.
pub const SESSION_OWNED_FILES: [&str; 8] = [
    "_meta.json",
    "notes.md",
    "_memo.md",
    "transcript.json",
    "tasks.json",
    "audio.mp3",
    "audio.wav",
    "audio.ogg",
];

/// Waveform cache for the session's recording.
pub const SESSION_PEAKS_FILE: &str = "audio.peaks.json";

/// Short-lived recording/write intermediates: per-channel captures before the
/// mix-down, and `.tmp`-suffixed in-progress audio artifacts
/// (`hypr-fs-sync-core`'s `AUDIO_ARTIFACTS` is the writer-side list).
pub const SESSION_TRANSIENT_FILES: [&str; 6] = [
    "audio_mic.wav",
    "audio_spk.wav",
    "audio.mp3.tmp",
    "audio.wav.tmp",
    "audio.ogg.tmp",
    "audio.peaks.json.tmp",
];

/// App-owned directories inside a session directory.
///
/// - `enhanced/` -- AI-generated documents (`enhanced/<uuid>.md`); the app owns the
///   whole namespace, every file inside is treated as a document.
/// - `attachments/` -- files embedded in the note (`SessionStore::save_attachment`);
///   app-managed storage the editor resolves by relative src, distinct from the loose
///   user attachments this contract leaves alone.
/// - `audio/` -- legacy recording location; nothing writes it anymore, kept readable
///   for vaults written by old builds.
pub const SESSION_OWNED_DIRS: [&str; 3] = ["enhanced", "attachments", "audio"];

/// True when `name` (a bare file or directory name directly inside a session
/// directory) is owned by the app under the contract above -- including dot-prefixed
/// names, which are never user attachments. Everything else is a user attachment.
pub fn is_session_owned_name(name: &str) -> bool {
    name.starts_with('.')
        || SESSION_OWNED_FILES.contains(&name)
        || name == SESSION_PEAKS_FILE
        || SESSION_TRANSIENT_FILES.contains(&name)
        || SESSION_OWNED_DIRS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_names_cover_the_app_surface_and_nothing_else() {
        for owned in [
            "_meta.json",
            "notes.md",
            "_memo.md",
            "transcript.json",
            "tasks.json",
            "audio.mp3",
            "audio.wav",
            "audio.ogg",
            "audio.peaks.json",
            "audio_mic.wav",
            "audio_spk.wav",
            "audio.mp3.tmp",
            "audio.peaks.json.tmp",
            "enhanced",
            "attachments",
            "audio",
            ".DS_Store",
            ".tmp-1234-5678-notes.md",
        ] {
            assert!(is_session_owned_name(owned), "{owned} should be app-owned");
        }

        for attachment in [
            "contract.pdf",
            "whiteboard.png",
            "minutes.md",
            "notes.txt",
            "recording.mp3",
            "audio2.mp3",
        ] {
            assert!(
                !is_session_owned_name(attachment),
                "{attachment} should be a user attachment"
            );
        }
    }
}

use std::path::{Path, PathBuf};

use crate::{Error, Result, output};
use hypr_fs_sync_core::runtime::{AudioImportEvent, AudioImportRuntime};
use hypr_vault_write::SessionStore;

/// The formats the desktop app accepts in its import dialog. `normalize_file`
/// re-encodes all of them to the vault's 16 kHz `audio.mp3`.
const SUPPORTED_EXTENSIONS: [&str; 8] = ["wav", "mp3", "ogg", "mp4", "m4a", "flac", "webm", "aac"];

/// Returns the process exit code: 0 on success, and the transcription error's
/// exit code when `--transcribe` fails after a successful import (the meeting
/// id is still reported so the partial result stays identifiable).
pub async fn run(
    vault: &Path,
    file: PathBuf,
    title: Option<String>,
    into: Option<String>,
    transcribe: bool,
    timestamps: super::NewSessionOptions,
    json: bool,
) -> Result<u8> {
    // Validate the input before touching the vault, so a bad file creates nothing.
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if !SUPPORTED_EXTENSIONS.contains(&extension.as_str()) {
        return Err(Error::operation(
            "import audio",
            format!(
                "unsupported audio format for {}; supported extensions: {}",
                file.display(),
                SUPPORTED_EXTENSIONS.join(", ")
            ),
        ));
    }
    if !file.is_file() {
        return Err(Error::NotFound(format!("audio file {}", file.display())));
    }

    let store = SessionStore::new(vault.to_path_buf());
    // `--into` targets an existing meeting; otherwise a new session is created,
    // titled after the file when no `--title` is given (clap rejects `--into`
    // together with `--title` or the timestamp overrides, so `title` is None
    // and `timestamps` is empty on the `--into` branch).
    let (meta, created) = match into {
        Some(id) => {
            let meta = store
                .read_meta(&id)
                .await
                .map_err(|error| Error::operation("import audio", error.to_string()))?
                .ok_or_else(|| Error::NotFound(format!("meeting '{id}'")))?;
            let existing_dir = vault.join(
                store
                    .session_dir(&id)
                    .await
                    .map_err(|error| Error::operation("import audio", error.to_string()))?,
            );
            if let Some(existing) = super::transcribe::find_session_audio(&existing_dir) {
                return Err(Error::operation(
                    "import audio",
                    format!(
                        "meeting {id} already has a recording ({}); the CLI never replaces existing audio",
                        existing.display()
                    ),
                ));
            }
            (meta, false)
        }
        None => {
            let title = title.unwrap_or_else(|| {
                file.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            let meta =
                super::create_session(vault, &store, "import audio", title, timestamps).await?;
            (meta, true)
        }
    };
    let session_id = meta.id.clone();

    let audio_guard = store
        .lock_session_audio(&session_id)
        .await
        .map_err(|error| Error::operation("import audio", error.to_string()))?;
    let session_dir = vault.join(
        store
            .session_dir(&session_id)
            .await
            .map_err(|error| Error::operation("import audio", error.to_string()))?,
    );
    let source_path = file.clone();
    // Both arms name the session so a partial import stays identifiable —
    // whether the converter failed cleanly or panicked (JoinError). A freshly
    // created session additionally reports that it now exists as an orphan.
    let convert_failed = |error: String| {
        Error::operation(
            "import audio",
            if created {
                format!("meeting {session_id} was created, but importing its audio failed: {error}")
            } else {
                format!("importing audio for meeting {session_id} failed: {error}")
            },
        )
    };
    let runtime = CliAudioImportRuntime {
        store: store.clone(),
        handle: tokio::runtime::Handle::current(),
    };
    let import_id = session_id.clone();
    let audio_path = tokio::task::spawn_blocking(move || {
        let _audio_guard = audio_guard;
        hypr_fs_sync_core::audio::import_to_session(
            &runtime,
            &import_id,
            &session_dir,
            &source_path,
        )
    })
    .await
    .map_err(|error| convert_failed(error.to_string()))?
    .map_err(|error| convert_failed(error.to_string()))?;

    let mut data = serde_json::json!({
        "id": session_id,
        "title": meta.title,
        "created_at": meta.created_at,
        "audio": audio_path,
    });

    let mut exit_code = 0u8;
    if transcribe {
        match super::transcribe::transcribe_session(vault, &store, &session_id).await {
            Ok(outcome) => {
                data["transcript"] = serde_json::json!({
                    "id": outcome.transcript_id,
                    "words": outcome.words,
                });
            }
            Err(error) => {
                // The import itself succeeded; report the meeting id on stdout
                // (plain or embedded in the JSON payload with the error) and
                // exit non-zero so callers see the partial failure.
                data["transcript"] = serde_json::Value::Null;
                data["transcript_error"] = serde_json::json!({
                    "code": error.code(),
                    "message": error.to_string(),
                });
                eprintln!(
                    "error: meeting {session_id} was imported, but transcription failed: {error}"
                );
                exit_code = error.exit_code();
            }
        }
    }

    let rendered = if json {
        output::json("import", &data, None)?
    } else {
        session_id
    };
    output::emit(&rendered);
    Ok(exit_code)
}

struct CliAudioImportRuntime {
    store: SessionStore,
    handle: tokio::runtime::Handle,
}

impl AudioImportRuntime for CliAudioImportRuntime {
    fn emit(&self, _event: AudioImportEvent) {}

    fn prepare_commit(&self, session_id: &str) -> std::io::Result<()> {
        self.handle
            .block_on(self.store.begin_audio_import(session_id))
            .map_err(|error| std::io::Error::other(error.to_string()))
    }

    fn finish_commit(&self, session_id: &str) -> std::io::Result<()> {
        self.handle
            .block_on(self.store.finish_audio_import(session_id))
            .map_err(|error| std::io::Error::other(error.to_string()))
    }
}

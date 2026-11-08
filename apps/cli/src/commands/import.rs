use crate::{Error, Result, local_owner::LocalGateway, output};
use hypr_loofah_protocol::{LocalCliOperation, LocalCliReply, LocalInitialSessionMetadata};
use std::path::{Path, PathBuf};

pub async fn run(
    vault: &Path,
    file: PathBuf,
    title: Option<String>,
    into: Option<String>,
    transcribe: bool,
    timestamps: super::NewSessionOptions,
    json: bool,
) -> Result<u8> {
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if !["wav", "mp3", "ogg", "mp4", "m4a", "flac", "webm", "aac"].contains(&extension.as_str()) {
        return Err(Error::operation(
            "import audio",
            format!(
                "unsupported audio format for {}; supported extensions: wav, mp3, ogg, mp4, m4a, flac, webm, aac",
                file.display()
            ),
        ));
    }
    if !file.is_file() {
        return Err(Error::NotFound(format!("audio file {}", file.display())));
    }
    let source = std::fs::File::open(&file)
        .map_err(|cause| Error::operation("read recording", cause.to_string()))?;
    let filename = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| Error::operation("import audio", "Cannot derive source filename"))?;
    let response = LocalGateway::new(vault.to_owned(), crate::vault::cache_for_vault(vault)?)
        .execute_with_upload(
            LocalCliOperation::Import {
                filename,
                title,
                into,
                transcribe,
                metadata: LocalInitialSessionMetadata {
                    created_at: timestamps.created_at,
                    started_at: timestamps.started_at,
                    ended_at: timestamps.ended_at,
                    tags: timestamps.tags,
                    author: timestamps.author,
                    skill: timestamps.skill,
                },
            },
            Some(source),
            super::transcribe::progress(),
        )
        .await?;
    let LocalCliReply::Imported {
        meta,
        audio_path,
        transcription,
    } = response
        .response
        .result
        .map_err(|cause| super::transcribe::media_error("import audio", cause))?
    else {
        return Err(crate::profiles::unexpected());
    };
    let session_id = meta.id.clone();
    let mut data = serde_json::json!({ "id": session_id, "title": meta.title, "created_at": meta.created_at, "audio": audio_path });
    let mut exit_code = 0;
    if let Some(transcription) = transcription {
        match transcription {
            Ok(outcome) => {
                data["transcript"] =
                    serde_json::json!({ "id": outcome.transcript_id, "words": outcome.words })
            }
            Err(cause) => {
                let cause = super::transcribe::media_error("transcribe audio", cause);
                data["transcript"] = serde_json::Value::Null;
                data["transcript_error"] =
                    serde_json::json!({ "code": cause.code(), "message": cause.to_string() });
                eprintln!(
                    "error: meeting {session_id} was imported, but transcription failed: {cause}"
                );
                exit_code = cause.exit_code();
            }
        }
    }
    output::emit(&if json {
        output::json("import", &data, None)?
    } else {
        session_id
    });
    Ok(exit_code)
}

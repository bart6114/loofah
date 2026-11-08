use std::path::Path;

use hypr_export_core::{
    ExportInput, ExportLabels, ExportMetadata, TextFormat, Transcript, TranscriptItem,
};
use hypr_loofah_protocol::{LocalCliOperation as Operation, LocalCliReply as Reply};

use crate::cli::{ExportContent, ExportFormat, MeetingCommand};
use crate::{Error, Result, output};

pub async fn run(
    reader: &crate::agent_reader::AgentReader,
    command: MeetingCommand,
    json: bool,
) -> Result<()> {
    let vault = reader.vault()?;
    let MeetingCommand::Export {
        id,
        format,
        include,
        output: path,
        force,
    } = command
    else {
        unreachable!();
    };
    if format == ExportFormat::Pdf && (path.is_none() || path.as_deref() == Some(Path::new("-"))) {
        return Err(Error::operation(
            "export session",
            "PDF requires --output FILE (not stdout)",
        ));
    }
    if let Some(path) = &path {
        let vault = vault
            .canonicalize()
            .map_err(|e| Error::operation("resolve vault", e.to_string()))?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let destination = path
            .canonicalize()
            .or_else(|_| {
                parent
                    .canonicalize()
                    .map(|p| p.join(path.file_name().unwrap_or_default()))
            })
            .map_err(|e| Error::operation("resolve output", e.to_string()))?;
        if destination.starts_with(&vault) {
            return Err(Error::operation(
                "export session",
                "output must be outside the source vault",
            ));
        }
        if !force && path.symlink_metadata().is_ok() {
            return Err(Error::OutputExists(path.clone()));
        }
    }
    let mut meeting = reader.export(id.clone()).await?;
    // Preserve the pre-existing Markdown/JSON artifact and envelope contract.
    if include.is_empty() && matches!(format, ExportFormat::Markdown | ExportFormat::Json) {
        let content = match (format, json) {
            (ExportFormat::Markdown, false) => meeting.to_markdown(),
            (ExportFormat::Json, false) => output::raw_json(&meeting)?,
            (ExportFormat::Markdown, true) => output::json(
                "meetings.export",
                &serde_json::json!({"format": "markdown", "content": meeting.to_markdown()}),
                None,
            )?,
            (ExportFormat::Json, true) => output::json("meetings.export", &meeting, None)?,
            _ => unreachable!(),
        };
        return output::write_or_emit(&content, path.as_deref(), force);
    }
    let include = if include.is_empty() {
        vec![ExportContent::Note, ExportContent::Summary]
    } else {
        include
    };
    if !include.contains(&ExportContent::Note) {
        meeting.meeting.note = None;
    }
    if !include.contains(&ExportContent::Summary) {
        meeting.meeting.summaries.clear();
    }
    let duration = meeting
        .transcripts
        .iter()
        .map(|t| t.started_at_ms)
        .min()
        .zip(
            meeting
                .transcripts
                .iter()
                .filter_map(|t| t.ended_at_ms)
                .max(),
        )
        .map(|(start, end)| {
            let minutes = (end - start).max(0) / 60_000;
            if minutes >= 60 {
                format!("{}h {}m", minutes / 60, minutes % 60)
            } else {
                format!("{minutes}m")
            }
        });
    if !include.contains(&ExportContent::Transcript) {
        meeting.transcripts.clear();
    }
    let mut attachment_owners = Vec::new();
    let input = if format == ExportFormat::Json {
        None
    } else {
        let transcript = if include.contains(&ExportContent::Transcript) {
            Some(Transcript {
                items: reader
                    .segments(id.clone())
                    .await?
                    .into_iter()
                    .map(|s| TranscriptItem {
                        speaker: Some(s.speaker_label),
                        text: s.text,
                    })
                    .collect(),
            })
        } else {
            None
        };
        let mut input = ExportInput {
            note_md: meeting.meeting.note.as_ref().map(|n| n.markdown.clone()),
            enhanced_md: meeting
                .meeting
                .summaries
                .first()
                .map(|s| s.markdown.clone())
                .unwrap_or_default(),
            transcript,
            metadata: Some(ExportMetadata {
                title: if meeting.meeting.title.is_empty() {
                    "Untitled".into()
                } else {
                    meeting.meeting.title.clone()
                },
                created_at: format_date(&meeting.meeting.created_at),
                participants: vec![],
                duration,
            }),
            attachments: vec![],
        };
        if format == ExportFormat::Pdf {
            let (files, owners) = collect_owned_attachments(reader, &id, &input).await?;
            input.attachments = files;
            attachment_owners = owners;
        }
        Some(input)
    };
    let name = match format {
        ExportFormat::Markdown => "markdown",
        ExportFormat::Json => "json",
        ExportFormat::Pdf => "pdf",
        ExportFormat::Txt => "txt",
        ExportFormat::Org => "org",
    };
    let bytes = if let Some(input) = input {
        tokio::task::spawn_blocking(move || {
            let _attachment_owners = attachment_owners;
            if format == ExportFormat::Pdf {
                hypr_export_core::render_pdf(&input)
                    .map_err(|e| Error::operation("render PDF", e.to_string()))
            } else {
                let format = match format {
                    ExportFormat::Markdown => TextFormat::Md,
                    ExportFormat::Txt => TextFormat::Txt,
                    ExportFormat::Org => TextFormat::Org,
                    _ => unreachable!(),
                };
                Ok(
                    hypr_export_core::render_text(&input, format, &ExportLabels::default())
                        .into_bytes(),
                )
            }
        })
        .await
        .map_err(|e| Error::operation("export session", e.to_string()))??
    } else {
        output::raw_json(&meeting)?.into_bytes()
    };
    if let Some(path) = &path {
        output::write_bytes(&bytes, path, force)?;
        if json {
            output::emit(&output::json(
                "meetings.export",
                &serde_json::json!({"format": name, "output": path, "bytes": bytes.len()}),
                None,
            )?);
        }
    } else {
        let content = String::from_utf8(bytes)
            .map_err(|e| Error::operation("export session", e.to_string()))?;
        if json {
            output::emit(&output::json(
                "meetings.export",
                &serde_json::json!({"format": name, "content": content}),
                None,
            )?);
        } else {
            output::emit(&content);
        }
    }
    Ok(())
}

fn format_date(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%A, %B %-d, %Y at %-I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|_| value.to_string())
}

async fn collect_owned_attachments(
    reader: &crate::agent_reader::AgentReader,
    session_id: &str,
    input: &ExportInput,
) -> Result<(
    Vec<hypr_export_core::ExportAttachment>,
    Vec<tempfile::NamedTempFile>,
)> {
    let gateway = reader.local_gateway()?;
    let markdown = format!(
        "{}\n{}",
        input.note_md.as_deref().unwrap_or_default(),
        input.enhanced_md
    );
    let mut after = None;
    let mut files = Vec::new();
    let mut owners = Vec::new();
    loop {
        let page = match gateway
            .execute(Operation::AttachmentPage {
                session_id: session_id.to_owned(),
                limit: 200,
                after: after.clone(),
            })
            .await?
            .response
            .result?
        {
            Reply::AttachmentPage { page } => page,
            _ => return Err(crate::profiles::unexpected()),
        };
        for file in page.files {
            if file.filename.starts_with('.') {
                continue;
            }
            let src = super::meetings::to_portable_attachment_src(&file.filename);
            if !markdown.contains(&format!("]({src}")) {
                continue;
            }
            let response = gateway
                .execute(Operation::Attachment {
                    session_id: session_id.to_owned(),
                    attachment_id: file.filename.clone(),
                })
                .await?;
            match response.response.result {
                Ok(Reply::Attachment { .. }) => {}
                Ok(_) => return Err(crate::profiles::unexpected()),
                Err(cause)
                    if matches!(
                        cause.code,
                        hypr_loofah_protocol::ErrorCode::NotFound
                            | hypr_loofah_protocol::ErrorCode::Internal
                    ) =>
                {
                    continue;
                }
                Err(cause) => return Err(cause.into()),
            }
            let blob = response.blob.ok_or_else(crate::profiles::unexpected)?;
            let private = blob.trusted_path().map_err(|cause| {
                Error::operation("prepare export attachment", cause.to_string())
            })?;
            let directory = private
                .parent()
                .ok_or_else(|| {
                    Error::operation(
                        "prepare export attachment",
                        "Private attachment has no directory",
                    )
                })?
                .to_owned();
            let suffix = Path::new(&file.filename)
                .extension()
                .and_then(|value| value.to_str())
                .map(|extension| format!(".{extension}"))
                .unwrap_or_default();
            let staged = blob
                .with_file(move |source| {
                    let mut output = tempfile::Builder::new()
                        .suffix(&suffix)
                        .tempfile_in(directory)?;
                    std::io::copy(source, output.as_file_mut())?;
                    Ok(output)
                })
                .await
                .map_err(|cause| {
                    Error::operation("prepare export attachment", cause.to_string())
                })?;
            files.push(hypr_export_core::ExportAttachment {
                src,
                path: staged.path().to_string_lossy().into_owned(),
            });
            owners.push(staged);
        }
        match page.next_cursor {
            Some(next) if after.as_ref() != Some(&next) => after = Some(next),
            Some(_) => {
                return Err(Error::operation(
                    "read export attachments",
                    "Attachment cursor did not advance",
                ));
            }
            None => break,
        }
    }
    files.sort_by(|a, b| a.src.cmp(&b.src));
    Ok((files, owners))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn owner_export_only_stages_referenced_managed_images_with_original_extensions() {
        const PNG: &[u8] = &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 252, 207,
            192, 80, 15, 0, 4, 133, 1, 128, 132, 169, 140, 33, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
            96, 130,
        ];
        let root = tempfile::tempdir().unwrap();
        let vault = root.path().join("vault");
        let session = vault.join("sessions/session");
        std::fs::create_dir_all(session.join("attachments")).unwrap();
        std::fs::write(
            session.join("_meta.json"),
            br#"{"id":"session","title":"Export","created_at":"2026-11-05T00:00:00Z","tags":[]}"#,
        )
        .unwrap();
        for name in ["photo é.PNG", "unused.png", ".hidden.png"] {
            std::fs::write(session.join("attachments").join(name), PNG).unwrap();
        }
        std::fs::write(session.join("unknown.png"), b"User-owned attachment").unwrap();
        let outside = root.path().join("outside.png");
        std::fs::write(&outside, b"Outside bytes").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, session.join("attachments/link.png")).unwrap();
        let mut input = ExportInput { note_md: Some("![Photo](attachments/photo%20%C3%A9.PNG)\n![Hidden](attachments/.hidden.png)\n![Unknown](unknown.png)\n![Link](attachments/link.png)".into()), enhanced_md:String::new(), transcript:None, metadata:None, attachments:vec![] };
        let reader = crate::agent_reader::AgentReader::local(
            vault.clone(),
            hypr_search_cache::Cache::new(&root.path().join("cache"), &vault).unwrap(),
        );
        let (attachments, owners) = collect_owned_attachments(&reader, "session", &input)
            .await
            .unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].src, "attachments/photo%20%C3%A9.PNG");
        assert_eq!(
            Path::new(&attachments[0].path).extension(),
            Some(std::ffi::OsStr::new("PNG"))
        );
        assert_eq!(std::fs::read(&attachments[0].path).unwrap(), PNG);
        std::fs::write(
            session.join("attachments/photo é.PNG"),
            b"Concurrent source edit",
        )
        .unwrap();
        assert_eq!(std::fs::read(&attachments[0].path).unwrap(), PNG);
        input.attachments = attachments;
        let pdf = tokio::task::spawn_blocking(move || {
            let _owners = owners;
            hypr_export_core::render_pdf(&input).unwrap()
        })
        .await
        .unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert_eq!(
            std::fs::read(session.join("unknown.png")).unwrap(),
            b"User-owned attachment"
        );
        assert_eq!(std::fs::read(outside).unwrap(), b"Outside bytes");
        assert!(!vault.join("config.json").exists());
    }
}

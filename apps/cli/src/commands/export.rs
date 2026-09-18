use std::path::Path;

use hypr_agent_access::{get_meeting_export, get_meeting_export_segments};
use hypr_export_core::{
    ExportInput, ExportLabels, ExportMetadata, TextFormat, Transcript, TranscriptItem,
};

use crate::cli::{ExportContent, ExportFormat, MeetingCommand};
use crate::{Error, Result, output};

pub async fn run(vault: &Path, command: MeetingCommand, json: bool) -> Result<()> {
    let MeetingCommand::Export {
        id,
        format,
        include,
        summary_id,
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
    if summary_id.is_some() && !include.contains(&ExportContent::Summary) {
        return Err(Error::operation(
            "export session",
            "--summary-id requires --include summary",
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
    let mut meeting = get_meeting_export(vault, id.clone()).await?;
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
    } else if let Some(id) = summary_id.as_deref() {
        meeting.meeting.summaries.retain(|s| s.id == id);
        if meeting.meeting.summaries.is_empty() {
            return Err(Error::NotFound(format!("summary '{id}'")));
        }
    } else {
        meeting.meeting.summaries.truncate(1);
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
    let input = if format == ExportFormat::Json {
        None
    } else {
        let transcript = if include.contains(&ExportContent::Transcript) {
            Some(Transcript {
                items: get_meeting_export_segments(vault, id.clone())
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
            let session_dir = super::meetings::session_path(vault, &id).await?;
            input.attachments = collect_attachments(&session_dir, &input);
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

fn collect_attachments(
    session_dir: &Path,
    input: &ExportInput,
) -> Vec<hypr_export_core::ExportAttachment> {
    let Ok(root) = session_dir.join("attachments").canonicalize() else {
        return vec![];
    };
    let Ok(session) = session_dir.canonicalize() else {
        return vec![];
    };
    if !root.starts_with(session) {
        return vec![];
    }
    let Ok(entries) = std::fs::read_dir(&root) else {
        return vec![];
    };
    let markdown = format!(
        "{}\n{}",
        input.note_md.as_deref().unwrap_or_default(),
        input.enhanced_md
    );
    let mut attachments = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            if name.starts_with('.') {
                return None;
            }
            let src = super::meetings::to_portable_attachment_src(&name);
            if !markdown.contains(&format!("]({src}")) {
                return None;
            }
            let path = entry.path().canonicalize().ok()?;
            if !path.starts_with(&root) || !path.is_file() {
                return None;
            }
            Some(hypr_export_core::ExportAttachment {
                src,
                path: path.to_string_lossy().into_owned(),
            })
        })
        .collect::<Vec<_>>();
    attachments.sort_by(|a, b| a.src.cmp(&b.src));
    attachments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachments_only_include_referenced_managed_files() {
        let session = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(session.path().join("attachments")).unwrap();
        for name in ["photo é.png", "unused.png", ".hidden.png"] {
            std::fs::write(session.path().join("attachments").join(name), "image").unwrap();
        }
        std::fs::write(session.path().join("unknown.png"), "user attachment").unwrap();
        std::fs::write(other.path().join("outside.png"), "outside").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            other.path().join("outside.png"),
            session.path().join("attachments/link.png"),
        )
        .unwrap();
        let input = ExportInput {
            note_md: Some("![photo](attachments/photo%20%C3%A9.png)\n![hidden](attachments/.hidden.png)\n![unknown](unknown.png)\n![link](attachments/link.png)".into()),
            enhanced_md: String::new(), transcript: None, metadata: None, attachments: vec![],
        };
        let attachments = collect_attachments(session.path(), &input);
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].src, "attachments/photo%20%C3%A9.png");
        assert_eq!(std::fs::read(&attachments[0].path).unwrap(), b"image");
    }
}

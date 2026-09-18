use regex::Regex;

use crate::{ExportInput, ExportLabels, TextFormat};

pub fn render_text(input: &ExportInput, format: TextFormat, labels: &ExportLabels) -> String {
    let metadata = input.metadata.as_ref();
    let title = metadata
        .map(|m| m.title.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&labels.untitled);
    let created = metadata.map(|m| m.created_at.as_str()).unwrap_or_default();
    let mut lines = match format {
        TextFormat::Md => vec![format!("# {title}")],
        TextFormat::Txt => vec![title.to_string(), "=".repeat(title.encode_utf16().count())],
        TextFormat::Org => vec![format!("#+TITLE: {title}")],
    };
    if matches!(format, TextFormat::Org) {
        if !created.is_empty() {
            lines.push(format!("#+DATE: {created}"));
        }
        lines.extend([String::new(), format!("* {}", labels.metadata)]);
    }
    let mut field = |label: &str, value: &str| {
        if !value.is_empty() {
            lines.push(match format {
                TextFormat::Md => format!("- {label}: {value}"),
                TextFormat::Txt if label == labels.created => value.to_string(),
                TextFormat::Txt => format!("{label}: {value}"),
                TextFormat::Org => format!("- {label} :: {value}"),
            });
        }
    };
    field(&labels.created, created);
    if let Some(metadata) = metadata {
        field(&labels.participants, &metadata.participants.join(", "));
        field(
            &labels.duration,
            metadata.duration.as_deref().unwrap_or_default(),
        );
    }
    let transcript = input
        .transcript
        .as_ref()
        .map(|t| {
            t.items
                .iter()
                .map(
                    |item| match item.speaker.as_deref().filter(|s| !s.is_empty()) {
                        Some(speaker) => format!("{speaker}: {}", item.text),
                        None => item.text.clone(),
                    },
                )
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default();
    for (heading, body, markdown) in [
        (
            &labels.note,
            input.note_md.as_deref().unwrap_or_default(),
            true,
        ),
        (&labels.summary, input.enhanced_md.as_str(), true),
        (&labels.transcript, transcript.as_str(), false),
    ] {
        if body.trim().is_empty() {
            continue;
        }
        lines.push(String::new());
        match format {
            TextFormat::Md => lines.push(format!("## {heading}")),
            TextFormat::Org => lines.push(format!("* {heading}")),
            TextFormat::Txt => {
                lines.extend([heading.clone(), "-".repeat(heading.encode_utf16().count())])
            }
        }
        lines.push(if markdown {
            convert_markdown(body, format)
        } else {
            body.to_string()
        });
    }
    lines.join("\n")
}

// Keep these conversions in one headless implementation for desktop and CLI.
fn convert_markdown(content: &str, format: TextFormat) -> String {
    let replacements: &[(&str, &str)] = match format {
        TextFormat::Md => return content.to_string(),
        TextFormat::Txt => &[
            (r"(?m)^#{1,6}\s+", ""),
            (r"\[([^\]]+)\]\(([^)]+)\)", "$1 ($2)"),
            (r"(?m)^\s*[-*+]\s+", "• "),
            (r"(?m)^\s*\d+\.\s+", ""),
            (r"\*\*(.*?)\*\*", "$1"),
            (r"\*(.*?)\*", "$1"),
            (r"__(.*?)__", "$1"),
            (r"_(.*?)_", "$1"),
            (r"`([^`]+)`", "$1"),
            (r"\n{3,}", "\n\n"),
        ],
        TextFormat::Org => &[
            (r"\[([^\]]+)\]\(([^)]+)\)", "[[$2][$1]]"),
            (r"\*\*(.*?)\*\*", "*$1*"),
            (r"__(.*?)__", "*$1*"),
            (r"`([^`]+)`", "~$1~"),
        ],
    };
    let mut result = content.to_string();
    if matches!(format, TextFormat::Org) {
        result = Regex::new(r"(?m)^(#{1,6})\s+")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures| {
                format!("{} ", "*".repeat(caps[1].len()))
            })
            .into_owned();
    }
    for (pattern, replacement) in replacements {
        result = Regex::new(pattern)
            .unwrap()
            .replace_all(&result, *replacement)
            .into_owned();
    }
    result.trim().to_string()
}

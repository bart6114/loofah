use crate::{Result, SearchDocument};
use hypr_vault_read::{SessionMeta, enhanced, meta, summary, transcript};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SessionListItem {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: String,
    pub ended_at: String,
    pub tags: Vec<String>,
    pub author: Option<String>,
    pub skill: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagItem {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

pub(crate) fn project(vault: &Path, id: &str) -> Result<Option<(SearchDocument, SessionListItem)>> {
    let dir = Path::new("sessions").join(id);
    let meta = match hypr_vault_read::classify_session_dir(&vault.join(&dir)) {
        hypr_vault_read::SessionDirKind::Session(meta) => *meta,
        hypr_vault_read::SessionDirKind::Folder => return Ok(None),
        hypr_vault_read::SessionDirKind::Corrupt(reason) => {
            return Err(crate::Error::InvalidInput(format!(
                "session {id}: {reason}"
            )));
        }
    };
    if meta.id != id {
        return Ok(None);
    }
    let mut parts = Vec::new();
    if let Some(note) = meta::read_note_in(vault, &dir)? {
        parts.push(extract_plain_text(&note));
    }
    let mut docs = Vec::new();
    for (_, parsed) in enhanced::scan_legacy_docs_in(vault, &dir, id)? {
        docs.push(parsed?);
    }
    let mut summaries: Vec<_> = docs.iter().filter(|doc| doc.kind == "summary").collect();
    summaries.sort_by(|a, b| (a.sort_order, &a.id).cmp(&(b.sort_order, &b.id)));
    let selected = summaries
        .first()
        .map(|doc| summary::as_document(id, doc.markdown.clone()));
    docs.retain(|doc| doc.kind != "summary");
    if let Some(doc) = selected {
        docs.push(doc);
    } else if let Some(markdown) = summary::read_canonical_in(vault, &dir)? {
        docs.push(summary::as_document(id, markdown));
    }
    docs.sort_by(|a, b| (a.sort_order, &a.id).cmp(&(b.sort_order, &b.id)));
    parts.extend(docs.iter().map(|doc| extract_plain_text(&doc.markdown)));
    let mut transcripts = transcript::read_transcript_json_in(vault, &dir)?.transcripts;
    transcripts.sort_by(|a, b| {
        a.started_at
            .total_cmp(&b.started_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    for transcript in transcripts {
        parts.push(
            transcript
                .words
                .iter()
                .map(|word| word.text.trim())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    let document = SearchDocument {
        id: id.to_owned(),
        doc_type: "session".into(),
        language: None,
        title: if meta.title.trim().is_empty() {
            "Untitled".into()
        } else {
            meta.title.trim().into()
        },
        content: parts
            .iter()
            .map(|part| part.trim())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        created_at: chrono::DateTime::parse_from_rfc3339(&meta.created_at)
            .map(|date| date.timestamp_millis())
            .unwrap_or(0),
        facets: Vec::new(),
    };
    Ok(Some((document, list_item(vault, &dir, meta)?)))
}

fn list_item(vault: &Path, dir: &Path, meta: SessionMeta) -> Result<SessionListItem> {
    let mut tags: Vec<_> = meta
        .tags
        .iter()
        .filter_map(|tag| hypr_vault_read::normalize_tag_name(tag))
        .collect();
    tags.sort();
    tags.dedup();
    let modified = std::fs::metadata(vault.join(dir).join("_meta.json"))?.modified()?;
    Ok(SessionListItem {
        id: meta.id,
        title: meta.title,
        kind: "meeting".into(),
        status: "active".into(),
        created_at: meta.created_at,
        updated_at: chrono::DateTime::<chrono::Utc>::from(modified)
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string(),
        started_at: meta.started_at.unwrap_or_default(),
        ended_at: meta.ended_at.unwrap_or_default(),
        tags,
        author: meta.author,
        skill: meta.skill,
    })
}

pub(crate) fn extract_plain_text(value: &str) -> String {
    let trimmed = value.trim();
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed)
        && parsed.get("type").and_then(|value| value.as_str()) == Some("doc")
        && parsed.get("content").is_some_and(|value| value.is_array())
    {
        fn text(node: &serde_json::Value) -> String {
            if let Some(text) = node.get("text").and_then(|value| value.as_str()) {
                return text.into();
            }
            node.get("content")
                .and_then(|value| value.as_array())
                .map(|nodes| nodes.iter().map(text).collect::<Vec<_>>().join(" "))
                .unwrap_or_default()
        }
        return text(&parsed)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
    }
    trimmed.into()
}

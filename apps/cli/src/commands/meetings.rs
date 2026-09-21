use std::path::Path;

use crate::cli::{DocumentKind, MeetingCommand, TagCommand};
use crate::{Error, Result, output};
use hypr_agent_access::{
    Document, GetMeetingInput, GetMeetingTranscriptInput, ListMeetingsInput, MeetingListItem,
    SearchHit, SearchMeetingsInput, get_meeting, get_meeting_transcript, list_meetings,
    search_meetings,
};
use hypr_vault_write::SessionStore;

pub async fn run(vault: &Path, command: MeetingCommand, json: bool) -> Result<()> {
    match command {
        MeetingCommand::List {
            query,
            limit,
            offset,
            tag,
            untagged,
        } => {
            let page = list_meetings(
                vault,
                ListMeetingsInput {
                    query,
                    limit: Some(limit),
                    offset: Some(offset),
                    tags: (!tag.is_empty()).then_some(tag),
                    untagged: untagged.then_some(true),
                },
            )
            .await?;
            let rendered = if json {
                output::json("meetings.list", &page.meetings, Some(&page.pagination))?
            } else {
                render_list(&page.meetings)
            };
            output::emit(&rendered);
            Ok(())
        }
        MeetingCommand::Search {
            query,
            speaker,
            kind,
            limit,
            offset,
        } => {
            let page = search_meetings(
                vault,
                SearchMeetingsInput {
                    query,
                    speaker,
                    kinds: (!kind.is_empty()).then(|| kind.into_iter().map(Into::into).collect()),
                    limit: Some(limit),
                    offset: Some(offset),
                },
            )
            .await?;
            let rendered = if json {
                output::json("meetings.search", &page.hits, Some(&page.pagination))?
            } else {
                render_search(&page.hits)
            };
            output::emit(&rendered);
            Ok(())
        }
        MeetingCommand::Get { id } => {
            let meeting = get_meeting(vault, GetMeetingInput { meeting_id: id }).await?;
            let rendered = if json {
                output::json("meetings.get", &meeting, None)?
            } else {
                meeting.to_markdown()
            };
            output::emit(&rendered);
            Ok(())
        }
        MeetingCommand::Rename { id, title } => rename_session(vault, &id, title, json).await,
        MeetingCommand::New {
            title,
            note,
            created_at,
            started_at,
            ended_at,
            tag,
            author,
            skill,
        } => {
            // Read the body before touching the vault, so a bad --note path creates nothing.
            let body = note.as_deref().map(read_body).transpose()?;
            let store = SessionStore::new(vault.to_path_buf());
            let meta = super::create_session(
                vault,
                &store,
                "create meeting",
                title,
                super::NewSessionOptions {
                    created_at,
                    started_at,
                    ended_at,
                    tags: tag,
                    author,
                    skill,
                },
            )
            .await?;
            let session_id = meta.id.clone();
            if let Some(body) = body {
                // The meta write above already created the session; name it in the
                // error so a partial failure leaves an identifiable meeting instead
                // of an anonymous orphan (recover with `meetings note ID --set`).
                store.write_note(&session_id, &body).await.map_err(|error| {
                    Error::operation(
                        "write note",
                        format!(
                            "meeting {session_id} was created, but writing its note failed: {error}"
                        ),
                    )
                })?;
            }
            // The tags already sit in the session's `_meta.json`; registering
            // them in the vault-root `tags.json` is a separate write, so a
            // failure here must also name the meeting it leaves behind.
            for tag in &meta.tags {
                store.ensure_tag(tag).await.map_err(|error| {
                    Error::operation(
                        "register tag",
                        format!(
                            "meeting {session_id} was created, but registering tag '{tag}' failed: {error}"
                        ),
                    )
                })?;
            }

            let rendered = if json {
                output::json(
                    "meetings.new",
                    &serde_json::json!({
                        "id": session_id,
                        "title": meta.title,
                        "created_at": meta.created_at,
                        "started_at": meta.started_at,
                        "ended_at": meta.ended_at,
                        "tags": meta.tags,
                        "author": meta.author,
                        "skill": meta.skill,
                    }),
                    None,
                )?
            } else {
                session_id
            };
            output::emit(&rendered);
            Ok(())
        }
        MeetingCommand::Note {
            id,
            kind,
            set,
            append,
        } => {
            if set.is_some() || append.is_some() {
                return edit_note(vault, &id, set.as_deref(), append.as_deref(), json).await;
            }

            let meeting = get_meeting(
                vault,
                GetMeetingInput {
                    meeting_id: id.clone(),
                },
            )
            .await?;
            if json {
                match kind {
                    DocumentKind::Note => {
                        output::emit(&output::json("meetings.note", &meeting.note, None)?)
                    }
                    DocumentKind::Summary => {
                        output::emit(&output::json("meetings.note", &meeting.summaries, None)?)
                    }
                    DocumentKind::All => output::emit(&output::json(
                        "meetings.note",
                        &serde_json::json!({
                            "note": meeting.note,
                            "summaries": meeting.summaries,
                        }),
                        None,
                    )?),
                }
                return Ok(());
            }

            let text = match kind {
                DocumentKind::Note => meeting
                    .note
                    .map(|note| note.markdown)
                    .ok_or_else(|| crate::Error::NotFound(format!("note for meeting '{id}'")))?,
                DocumentKind::Summary => render_documents(&meeting.summaries),
                DocumentKind::All => {
                    let mut documents = meeting.note.into_iter().collect::<Vec<_>>();
                    documents.extend(meeting.summaries);
                    render_documents(&documents)
                }
            };
            output::emit(&text);
            Ok(())
        }
        MeetingCommand::Transcript { id } => {
            let transcript =
                get_meeting_transcript(vault, GetMeetingTranscriptInput { meeting_id: id }).await?;
            let rendered = if json {
                output::json("meetings.transcript", &transcript, None)?
            } else {
                transcript.text
            };
            output::emit(&rendered);
            Ok(())
        }
        MeetingCommand::Tag { command } => match command {
            TagCommand::Add { id, tags } => edit_tags(vault, &id, tags, true, json).await,
            TagCommand::Remove { id, tags } => edit_tags(vault, &id, tags, false, json).await,
        },
        MeetingCommand::Delete { id } => delete_session(vault, &id, json).await,
        MeetingCommand::Path { id } => {
            let path = session_path(vault, &id).await?;
            let rendered = if json {
                output::json(
                    "meetings.path",
                    &serde_json::json!({
                        "id": id,
                        "path": path.to_string_lossy(),
                    }),
                    None,
                )?
            } else {
                path.to_string_lossy().into_owned()
            };
            output::emit(&rendered);
            Ok(())
        }
        MeetingCommand::Attach { id, file, name } => {
            let saved = attach_file(vault, &id, &file, name).await?;
            let rendered = if json {
                output::json(
                    "meetings.attach",
                    &serde_json::json!({
                        "id": id,
                        "attachment_id": saved.attachment_id,
                        "path": saved.relative_path.to_string_lossy(),
                        "src": to_portable_attachment_src(&saved.attachment_id),
                    }),
                    None,
                )?
            } else {
                saved.attachment_id
            };
            output::emit(&rendered);
            Ok(())
        }
        command @ MeetingCommand::Export { .. } => super::export::run(vault, command, json).await,
    }
}

async fn rename_session(vault: &Path, id: &str, title: String, json: bool) -> Result<()> {
    let store = SessionStore::new(vault.to_path_buf());
    store
        .read_meta(id)
        .await
        .map_err(|error| Error::operation("rename session", error.to_string()))?
        .ok_or_else(|| Error::NotFound(format!("session '{id}'")))?;

    store
        .update_meta(
            id,
            hypr_vault_write::SessionMetaPatch {
                title: Some(title.clone()),
                ..Default::default()
            },
        )
        .await
        .map_err(|error| Error::operation("rename session", error.to_string()))?;

    let rendered = if json {
        output::json(
            "sessions.rename",
            &serde_json::json!({ "id": id, "title": title }),
            None,
        )?
    } else {
        format!("Renamed session {id}.")
    };
    output::emit(&rendered);
    Ok(())
}

async fn edit_note(
    vault: &Path,
    id: &str,
    set: Option<&Path>,
    append: Option<&Path>,
    json: bool,
) -> Result<()> {
    let store = SessionStore::new(vault.to_path_buf());
    let exists = store
        .read_meta(id)
        .await
        .map_err(|error| Error::operation("edit note", error.to_string()))?
        .is_some();
    if !exists {
        return Err(Error::NotFound(format!("meeting '{id}'")));
    }

    let (action, source) = match (set, append) {
        (Some(path), None) => ("set", path),
        (None, Some(path)) => ("append", path),
        _ => unreachable!("clap enforces exactly one of --set/--append"),
    };
    let body = read_body(source)?;

    let markdown = if action == "append" {
        match store
            .read_note(id)
            .await
            .map_err(|error| Error::operation("edit note", error.to_string()))?
        {
            Some(existing) if !existing.is_empty() => {
                if existing.ends_with('\n') {
                    format!("{existing}{body}")
                } else {
                    format!("{existing}\n{body}")
                }
            }
            _ => body,
        }
    } else {
        body
    };

    store
        .write_note(id, &markdown)
        .await
        .map_err(|error| Error::operation("edit note", error.to_string()))?;

    let rendered = if json {
        output::json(
            "meetings.note",
            &serde_json::json!({ "id": id, "action": action, "updated": true }),
            None,
        )?
    } else {
        format!("Updated note for meeting {id}.")
    };
    output::emit(&rendered);
    Ok(())
}

/// Add or remove tags on a meeting's `_meta.json`. Stored tags come back
/// normalized (trimmed, `#`-stripped, lowercased), deduped, and sorted —
/// the same shape the desktop writes. Adding a present tag or removing an
/// absent one is a no-op; `remove` never touches the append-only `tags.json`
/// registry, while `add` registers each newly added tag there.
async fn edit_tags(vault: &Path, id: &str, tags: Vec<String>, add: bool, json: bool) -> Result<()> {
    let requested = tags
        .iter()
        .map(|tag| {
            hypr_vault_read::normalize_tag_name(tag)
                .ok_or_else(|| Error::operation("edit tags", "tag name cannot be empty"))
        })
        .collect::<Result<Vec<_>>>()?;

    let store = SessionStore::new(vault.to_path_buf());
    let meta = store
        .read_meta(id)
        .await
        .map_err(|error| Error::operation("edit tags", error.to_string()))?
        .ok_or_else(|| Error::NotFound(format!("meeting '{id}'")))?;

    let current = {
        let mut current = meta
            .tags
            .iter()
            .filter_map(|tag| hypr_vault_read::normalize_tag_name(tag))
            .collect::<Vec<_>>();
        current.sort();
        current.dedup();
        current
    };

    let (changed, finalized) = if add {
        let mut changed = requested
            .iter()
            .filter(|tag| !current.contains(tag))
            .cloned()
            .collect::<Vec<_>>();
        changed.sort();
        changed.dedup();
        let mut finalized = current.clone();
        finalized.extend(changed.iter().cloned());
        finalized.sort();
        (changed, finalized)
    } else {
        let mut changed = requested
            .iter()
            .filter(|tag| current.contains(tag))
            .cloned()
            .collect::<Vec<_>>();
        changed.sort();
        changed.dedup();
        let finalized = current
            .iter()
            .filter(|tag| !requested.contains(tag))
            .cloned()
            .collect::<Vec<_>>();
        (changed, finalized)
    };

    if !changed.is_empty() {
        store
            .update_meta(
                id,
                hypr_vault_write::SessionMetaPatch {
                    tags: Some(finalized.clone()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| Error::operation("edit tags", error.to_string()))?;
        if add {
            // The tags already sit in the session's `_meta.json`; registering
            // them in the vault-root `tags.json` is a separate write, so a
            // failure here must name the meeting the edit leaves behind.
            for tag in &changed {
                store.ensure_tag(tag).await.map_err(|error| {
                    Error::operation(
                        "register tag",
                        format!(
                            "tags for meeting {id} were updated, but registering tag '{tag}' failed: {error}"
                        ),
                    )
                })?;
            }
        }
    }

    let rendered = if json {
        let (command, changed_key) = if add {
            ("meetings.tag.add", "added")
        } else {
            ("meetings.tag.remove", "removed")
        };
        output::json(
            command,
            &serde_json::json!({
                "id": id,
                changed_key: changed,
                "tags": finalized,
            }),
            None,
        )?
    } else if finalized.is_empty() {
        format!("Tags for meeting {id}: (none)")
    } else {
        format!("Tags for meeting {id}: {}", finalized.join(", "))
    };
    output::emit(&rendered);
    Ok(())
}

async fn delete_session(vault: &Path, id: &str, json: bool) -> Result<()> {
    let relative = hypr_vault_read::paths::validated_session_dir(id)
        .map_err(|error| Error::operation("delete session", error.to_string()))?;
    let vault = std::fs::canonicalize(vault)
        .map_err(|error| Error::operation("resolve vault path", error.to_string()))?;
    let path = vault.join(relative);
    let trash_path = SessionStore::new(vault)
        .delete_session(id)
        .await
        .map_err(|error| Error::operation("delete session", error.to_string()))?
        .ok_or_else(|| Error::NotFound(format!("session '{id}' (missing or already deleted)")))?;
    let deleted_at = chrono::Utc::now().to_rfc3339();
    let rendered = if json {
        output::json(
            "sessions.delete",
            &serde_json::json!({
                "id": id,
                "status": "deleted",
                "mode": "soft",
                "path": path,
                "trash_path": trash_path,
                "deleted_at": deleted_at,
            }),
            None,
        )?
    } else {
        format!(
            "Deleted session {id}; recover from {}",
            trash_path.display()
        )
    };
    output::emit(&rendered);
    Ok(())
}

/// Resolve only `sessions/<id>`, verifying its metadata without discovery.
pub(super) async fn session_path(vault: &Path, id: &str) -> Result<std::path::PathBuf> {
    let scan_vault = vault.to_path_buf();
    let scan_id = id.to_string();
    let location =
        tokio::task::spawn_blocking(move || hypr_vault_read::find_session(&scan_vault, &scan_id))
            .await
            .map_err(|error| Error::operation("resolve meeting path", error.to_string()))?
            .map_err(|error| Error::operation("resolve meeting path", error.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("meeting '{id}'")))?
            .0;

    std::path::absolute(vault.join(&location.relative_dir))
        .map_err(|error| Error::operation("resolve meeting path", error.to_string()))
}

async fn attach_file(
    vault: &Path,
    id: &str,
    file: &Path,
    name: Option<String>,
) -> Result<hypr_vault_write::SavedAttachment> {
    // Read the bytes before touching the vault, so a bad path creates nothing.
    if !file.is_file() {
        return Err(Error::NotFound(format!("file {}", file.display())));
    }
    let bytes = std::fs::read(file).map_err(|error| {
        Error::operation("read attachment", format!("{}: {error}", file.display()))
    })?;

    let filename = match name {
        Some(name) => name,
        None => file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| {
                Error::operation(
                    "attach file",
                    format!("cannot derive a filename from {}", file.display()),
                )
            })?,
    };

    let store = SessionStore::new(vault.to_path_buf());
    store
        .read_meta(id)
        .await
        .map_err(|error| Error::operation("attach file", error.to_string()))?
        .ok_or_else(|| Error::NotFound(format!("meeting '{id}'")))?;

    store
        .save_attachment(id, &filename, bytes)
        .await
        .map_err(|error| Error::operation("attach file", error.to_string()))
}

/// Byte-matches the desktop editor's `toPortableAttachmentSrc`
/// (`packages/editor/src/note/portable-attachments.ts`): JavaScript
/// `encodeURIComponent` over the attachment id, with parens additionally
/// encoded so the src never breaks markdown link syntax.
pub(super) fn to_portable_attachment_src(attachment_id: &str) -> String {
    use std::fmt::Write;

    let mut src = String::from("attachments/");
    for byte in attachment_id.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\'' => src.push(byte as char),
            _ => write!(src, "%{byte:02X}").expect("writing to a String cannot fail"),
        }
    }
    src
}

fn read_body(source: &Path) -> Result<String> {
    if source == Path::new("-") {
        let mut body = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut body)
            .map_err(|error| Error::operation("read note body", error.to_string()))?;
        return Ok(body);
    }
    std::fs::read_to_string(source).map_err(|error| {
        Error::operation("read note body", format!("{}: {error}", source.display()))
    })
}

fn render_list(meetings: &[MeetingListItem]) -> String {
    if meetings.is_empty() {
        return "No meetings found.".to_string();
    }

    let title_width = meetings
        .iter()
        .map(|meeting| meeting.title.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(5, 48);
    let mut lines = vec![format!("{:<24}  {:<title_width$}  ID", "DATE", "TITLE")];
    for meeting in meetings {
        let occurred_at = if meeting.started_at.is_empty() {
            &meeting.created_at
        } else {
            &meeting.started_at
        };
        lines.push(format!(
            "{:<24}  {:<title_width$}  {}",
            truncate(occurred_at, 24),
            truncate(
                if meeting.title.is_empty() {
                    "Untitled"
                } else {
                    &meeting.title
                },
                title_width,
            ),
            meeting.id,
        ));
    }
    lines.join("\n")
}

fn render_search(hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        return "No matches found.".to_string();
    }

    let mut lines = vec![format!(
        "{:<10}  {:<10}  {:<26}  SNIPPET",
        "DATE", "KIND", "ID"
    )];
    for hit in hits {
        let speaker = hit
            .speaker
            .as_deref()
            .map(|speaker| format!("{speaker}: "))
            .unwrap_or_default();
        lines.push(format!(
            "{:<10}  {:<10}  {:<26}  {speaker}{}",
            truncate(&hit.occurred_at, 10),
            hit.kind,
            truncate(&hit.meeting_id, 26),
            truncate(&hit.snippet, 100),
        ));
    }
    lines.join("\n")
}

fn render_documents(documents: &[Document]) -> String {
    documents
        .iter()
        .filter(|document| !document.markdown.trim().is_empty())
        .map(|document| {
            let title = if document.title.trim().is_empty() {
                "Summary"
            } else {
                document.title.trim()
            };
            format!("## {title}\n\n{}", document.markdown.trim())
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    let mut text = value
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>();
    text.push('…');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_render_is_bounded_and_contains_ids() {
        let rendered = render_list(&[MeetingListItem {
            id: "meeting-1".to_string(),
            title: "A very long planning meeting title that should not own the terminal"
                .to_string(),
            kind: "meeting".to_string(),
            status: "active".to_string(),
            created_at: "2026-07-13T09:00:00Z".to_string(),
            updated_at: "2026-07-13T09:00:00Z".to_string(),
            started_at: String::new(),
            ended_at: String::new(),
            tags: Vec::new(),
            author: None,
            skill: None,
        }]);
        assert!(rendered.contains("meeting-1"));
        assert!(rendered.contains('…'));
    }

    #[test]
    fn portable_src_matches_the_editor_encoding() {
        assert_eq!(
            to_portable_attachment_src("image 73.png"),
            "attachments/image%2073.png"
        );
        assert_eq!(
            to_portable_attachment_src("weird (v2)!.png"),
            "attachments/weird%20%28v2%29!.png"
        );
        assert_eq!(
            to_portable_attachment_src("présentation.png"),
            "attachments/pr%C3%A9sentation.png"
        );
    }

    #[tokio::test]
    async fn attach_stores_dedupes_and_encodes_the_src() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        let meta = super::super::create_session(
            vault.path(),
            &store,
            "create meeting",
            "Standup".to_string(),
            super::super::NewSessionOptions::default(),
        )
        .await
        .unwrap();

        let source = tempfile::tempdir().unwrap();
        let file = source.path().join("image 73.png");
        std::fs::write(&file, b"png-bytes").unwrap();

        let first = attach_file(vault.path(), &meta.id, &file, None)
            .await
            .unwrap();
        let second = attach_file(vault.path(), &meta.id, &file, None)
            .await
            .unwrap();

        assert_eq!(first.attachment_id, "image 73.png");
        assert_eq!(second.attachment_id, "image 73 1.png");

        let session_dir = vault
            .path()
            .join(store.session_dir(&meta.id).await.unwrap());
        for saved in [&first, &second] {
            let abs = vault.path().join(&saved.relative_path);
            assert!(abs.is_file());
            assert_eq!(abs.parent().unwrap(), session_dir.join("attachments"));
        }
        assert_eq!(
            to_portable_attachment_src(&first.attachment_id),
            "attachments/image%2073.png"
        );
        assert_eq!(
            to_portable_attachment_src(&second.attachment_id),
            "attachments/image%2073%201.png"
        );

        // --name overrides the stored filename.
        let named = attach_file(
            vault.path(),
            &meta.id,
            &file,
            Some("weird (v2)!.png".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(named.attachment_id, "weird (v2)!.png");
        assert_eq!(
            to_portable_attachment_src(&named.attachment_id),
            "attachments/weird%20%28v2%29!.png"
        );
    }

    #[tokio::test]
    async fn attach_to_a_missing_meeting_or_file_creates_nothing() {
        let vault = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let file = source.path().join("doc.pdf");
        std::fs::write(&file, b"%PDF").unwrap();

        let error = attach_file(vault.path(), "no-such-meeting", &file, None)
            .await
            .unwrap_err();
        assert!(matches!(error, crate::Error::NotFound(_)));
        assert!(!vault.path().join("sessions").exists());

        let store = SessionStore::new(vault.path().to_path_buf());
        let meta = super::super::create_session(
            vault.path(),
            &store,
            "create meeting",
            "Standup".to_string(),
            super::super::NewSessionOptions::default(),
        )
        .await
        .unwrap();
        let error = attach_file(
            vault.path(),
            &meta.id,
            &source.path().join("missing.pdf"),
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, crate::Error::NotFound(_)));
        let session_dir = vault
            .path()
            .join(store.session_dir(&meta.id).await.unwrap());
        assert!(!session_dir.join("attachments").exists());
    }
}

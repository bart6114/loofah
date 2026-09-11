---
title: "Vault guide for agents"
description: "The standing AGENTS.md every vault carries: what Loofah is, how the vault is structured, and how to work with it."
---

Loofah is a local-first knowledge and note-taking app that began with meeting transcription. Its **vault** can hold recorded meetings, standalone notes, imported material, and work an agent creates from other authorized sources. The vault is a plain folder of Markdown and JSON files and the only source of truth. There is no database or cloud copy. A copy of this page is kept at the vault root as `AGENTS.md`. Full, current documentation lives at https://loofah.io/, with machine-readable indexes at https://loofah.io/llms.txt and https://loofah.io/llms-full.txt.

For agents with shell access, use the `loof` CLI for both reading and writing. Start with the [skill installation prompt](https://loofah.io/agents/skills/#install-with-a-prompt) to install the Loofah skill and remember to use it through the CLI when the user wants to save or retrieve knowledge.

## Vault structure

```text
<vault>/
  AGENTS.md              this file (auto-regenerated)
  config.json            app configuration
  settings.json          app settings
  tags.json  tasks.json  people.json  events.json  calendars.json
  humans/  organizations/
  .trash/                soft-deleted files, kept by date; recoverable
  sessions/<id>/         one session per directory; may be a meeting or standalone note
    _meta.json           identity + metadata; its presence marks a session
    notes.md             the user's note (legacy vaults: _memo.md)
    transcript.json      speaker-labeled transcript
    tasks.json           session tasks
    audio.mp3|wav|ogg    the recording, with audio.peaks.json waveform cache
    enhanced/<uuid>.md   AI-generated documents (summaries)
    attachments/         files embedded in the note
```

Ownership rules:

- Inside a session directory the app owns exactly the names above. **Any other file is a user attachment: leave it alone**, and never claim unknown files as app content.
- Dot-prefixed files (`.tmp-*`, `.DS_Store`, `.trash/`) are never content.
- Do not create or rename files under the app-owned names; use the CLI to write.

## Authorship

`_meta.json` may carry an optional `author` field. When it is absent the note
was written by the vault owner; when set (a free-form name such as
`claude-code`) the note was written by someone else, and the app marks it as
not written by the owner. Next to `author`, an optional `skill` field records
the skill (a named, reusable instruction set such as `meeting-summarizer`)
the author ran to produce the note, if any.

Rules for agents:

- **Always pass `--author <your-agent-name>` when creating a session** with
  `loof sessions new` or `loof import`. Pick one stable name (for example
  `claude-code`) and keep using it.
- **If a skill produced the note, also pass `--skill <skill-name>`** so the
  session records which skill was used. Use the skill's stable name; omit the
  flag when no skill was involved.
- Write your own notes as **new** sessions with `--author` set. When asked to
  edit an existing note, never add, change, or remove its `author` or `skill`
  Editing the owner's note does not make it yours.

## Reading session data

Use Loofah's typed, read-only interfaces for session data.
Do not use `find`, `grep`, `rg`, filesystem crawling, or direct SQLite queries
to find or read sessions.

Prefer the `loof` CLI with `--json`, even when MCP tools are connected:

(`meetings` is a compatibility alias for `sessions` while deprecation is phased in.)

```sh
loof --json sessions list --query "planning"
loof --json sessions get SESSION_ID
loof --json sessions transcript SESSION_ID
```

The CLI discovers Loofah's vault from the platform
application-data directory, following the `vault_path` redirect in its
`global.json` when the vault has been relocated. Use
`--vault-path ABSOLUTE_VAULT_DIR` only when the user explicitly provides a
non-default vault path; do not crawl the filesystem to find one. Never guess a
session ID. Fetch a transcript only when notes and summaries do not contain
the needed context. Transcript commands return the complete transcript and can produce large responses.

If CLI access is unavailable or the user requests MCP, use connected Loofah MCP tools for read-only access: `list_meetings` or `search_meetings` to resolve an ID, `get_meeting` for notes, summaries, and action items, and `get_meeting_transcript` for the full transcript. The tool names retain `meeting` for compatibility with standalone notes too. Saving or editing notes requires the CLI.

## The loof CLI

Run `loof doctor` first to verify the CLI can reach the vault (it also repairs
a missing or stale `AGENTS.md`). Always pass `--json` for machine-readable
output.

| Command | Purpose |
| --- | --- |
| `doctor` | Check CLI and vault access without changing data. |
| `sessions list` | List sessions, optionally filtered with `--query`. |
| `sessions search` | Full-text search across titles, notes, summaries, and transcripts. |
| `sessions get` | Metadata, note, summaries, and action items for one session. |
| `sessions new` | Create a standalone note and print its id; pass `--author` when writing as an agent, plus `--skill` when a skill produced the note. |
| `sessions note` | Show a session's note, or edit it with `--set` / `--append`. |
| `sessions transcript` | The full speaker-labeled transcript. |
| `sessions tag add` | Add tags to a session, registering new ones in the vault. |
| `sessions tag remove` | Remove tags from a session. |
| `sessions path` | Print the absolute path of a session directory. |
| `sessions attach` | Store a file as a note attachment and print its id. |
| `sessions export` | Export a session to Markdown or JSON. |
| `import` | Import an audio file as a new session or into an existing one. |
| `transcribe` | Transcribe a session's audio with the configured on-device model. |
| `mcp` | Run the read-only MCP server over stdio. |
| `tags list` | List every tag registered in the vault. |

Per-command flags are documented at
https://loofah.io/reference/cli/.

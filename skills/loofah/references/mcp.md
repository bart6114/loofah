# MCP tools and resources

Use these tools when CLI access is unavailable or the user specifically requests MCP. Prefer the CLI for agents with shell access. Meeting and live-context tools are read-only. `nudge` has a UI side effect and restarts its timer when repeated; it never changes vault content.

| Tool | Use |
| --- | --- |
| `list_meetings` | List recent sessions, or use `query` for relevance-ranked full-text matches; `tags` (all must match) or `untagged` filter by tags, and each result includes its normalized `tags` and its `author` (`null` when the vault owner wrote it). |
| `search_meetings` | Desktop full-text matching, one relevance-ranked session hit with ID, score, and snippets. Fetch transcript details separately. |
| `get_meeting` | Read metadata, canonical note, summaries, and action items. |
| `get_meeting_transcript` | Read the full transcript as `[HH:MM:SS] Speaker: ...` lines, one per speaker turn. |

Available resources:

- `loofah://meetings/{meeting_id}`
- `loofah://meetings/{meeting_id}/transcript`

Prefer tools when the workflow needs structured JSON. Use resources when the client needs concise Markdown or plain-text context.

The first search, list, or resource-list request initializes a missing or incompatible shared cache; live tools and detail reads are independent of it. Use `loof --json init` for repair. Search and list requests reconcile external edits.

## Live coaching through an external agent

- `get_active_meeting()` discovers `active_meeting` (or `null`), its `recording_id`, and live-transcription/nudge availability.
- `get_live_transcript({ recording_id, cursor? })` returns finalized word upserts/removals. Poll every five seconds, drain `has_more`, reuse `cursor`, and replace context on `reset`. Initial snapshots cover at most five minutes/1,000 words; `truncated` reports omitted context. Use the full transcript tool for earlier saved material. Context expires two minutes after stop.
- `nudge({ text, recording_id? })` displays up to 200 Unicode characters beneath the recording pill for 20 seconds, paused on hover. Always pass the discovered recording ID. It replaces the previous nudge and requires the floating bar to be enabled. Success is UI acceptance, not proof the person read it. Never automatically retry `delivery_timeout`.

All three require the running macOS desktop app with the same vault/channel. An external agent runs the coaching loop; these tools do not keep it running. Stop nudging when the recording ends, and report unavailable transcription rather than assuming the meeting is silent. Nudge text is transient and never saved in the vault.

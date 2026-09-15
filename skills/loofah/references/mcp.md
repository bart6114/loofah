# MCP tools and resources

Use these tools when CLI access is unavailable or the user specifically requests MCP. Prefer the CLI for agents with shell access. All MCP tools are read-only and idempotent.

| Tool | Use |
| --- | --- |
| `list_meetings` | Find recent meetings by title or ID fragment; `tags` (all must match) or `untagged` filter by tags, and each result includes its normalized `tags` and its `author` (`null` when the vault owner wrote it). |
| `search_meetings` | Full-text search across titles, notes, summaries, and transcript words; `speaker` (id or name) limits results to meetings where that person spoke. Transcript hits return a `start_ms` matching the transcript's timestamps. |
| `get_meeting` | Read metadata, canonical note, summaries, and action items. |
| `get_meeting_transcript` | Read the full transcript as `[HH:MM:SS] Speaker: ...` lines, one per speaker turn. |

Available resources:

- `loofah://meetings/{meeting_id}`
- `loofah://meetings/{meeting_id}/transcript`

Prefer tools when the workflow needs structured JSON. Use resources when the client needs concise Markdown or plain-text context.

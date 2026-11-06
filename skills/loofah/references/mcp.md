# MCP tools and resources

Use these tools when CLI access is unavailable or the user specifically requests MCP. Prefer the CLI for agents with shell access. All MCP tools are read-only and idempotent.

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

Local MCP startup automatically builds a missing or incompatible shared cache, reporting progress on stderr. Use `loof --json init` for repair. Search and list requests reconcile external edits.


For a server vault, the user first signs in with `loof profiles login company
--server https://notes.example.com`, then starts `loof --profile company mcp`.
The same tools and resource URIs read that selected vault over HTTPS. Tool calls
never open a browser, and errors never fall back to a local vault. Restart MCP
after changing the profile's account or vault. The initial remote CLI command
set is also read-only; see `cli.md` before attempting a write.

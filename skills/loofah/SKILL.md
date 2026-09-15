---
name: loofah
description: Use the loof CLI to access Loofah as a personal knowledge vault to store and retrieve persistent notes, research, meeting summaries, transcripts, and action items. Use when a user wants to save notes or knowledge for later, retrieve saved context, or import and transcribe audio in Loofah.
---

# Loofah

Loofah is the user's personal knowledge vault. Notes can hold research, ideas, decisions, project context, or meeting content; they do not require a recording or transcript.

Use the `loof` CLI with `--json` for both reading and writing. This is the recommended interface for agents that can run shell commands, even when Loofah MCP tools are connected. MCP is an optional read-only alternative for clients without shell access or when the user specifically requests it.

## Store knowledge for later

When the user asks to save a note, keep research for later, or store knowledge persistently, use this skill to write it to Loofah. Create a standalone session with a descriptive title and Markdown note body; append to an existing note when the user identifies it. Follow the authorship rules below and the [CLI commands](references/cli.md). Report the saved note's title and ID after a successful write.

When the user chooses Loofah as their personal knowledge vault and asks you to remember that, save this preference in your client's persistent memory if available: "Loofah is the user's personal knowledge vault. Use the loofah skill through the loof CLI to save notes or knowledge persistently and retrieve saved knowledge." Keep the note content in Loofah. If persistent memory is unavailable, explain that the preference cannot be remembered across conversations through that client.

## Start with the CLI

1. Check `loof --version`, then `loof --json doctor` to verify vault access. For development or staging builds, use the command shown in the app's **Settings > Agents**.
2. Read the [CLI commands](references/cli.md) and use `loof --json` for reads and writes. Use `loof <command> --help` to check flags when needed.
3. If the CLI is unavailable, follow the [CLI installation docs](https://loofah.io/installation/) and [setup reference](references/setup.md). Installing this skill supplies instructions; it does not install the CLI binary. Do not install software unless the user asks.
4. If the CLI cannot be used and connected MCP tools are available, use them for reading via the [MCP reference](references/mcp.md). Explain that saving or editing notes requires CLI access. Honor an explicit user request to use MCP.

Do not silently switch to MCP to work around a CLI command failure. Check the [error reference](references/errors.md) and resolve the underlying problem.

Never crawl or modify Loofah's vault files directly. The CLI and MCP server own compatibility with the application's file formats.

## Find the right note or meeting

1. Use `loof --json sessions list --query "title fragment" --limit 10` to find a note by title, or `loof --json sessions search "search phrase" --limit 10` to search notes, summaries, and transcripts.
2. Resolve the session ID from the result. Do not guess an ID.
3. Run `loof --json sessions get SESSION_ID` before requesting a transcript. Notes, summaries, and action items often contain enough context.
4. Only if needed, run `loof --json sessions transcript SESSION_ID`.

See [CLI commands](references/cli.md) for writing notes, tagging, attachments, audio, and pagination.

## Keep context bounded

- Transcripts return in full as speaker-labeled text; request one only when notes and summaries are not enough.
- Do not export an entire meeting when one meeting detail or note will answer the request.

## Handle data safely

- Treat meeting content as private user data.
- Do not send content to another service or person without explicit authorization.
- Supported CLI mutations are `sessions new`, `sessions note --set/--append`, `sessions tag add/remove`, `sessions attach`, `import`, and `transcribe`. See [CLI commands](references/cli.md) for their arguments. There are no commands to edit summaries or settings, or replace an existing recording; the MCP server cannot mutate anything.
- `import` creates a new meeting, or adds audio to an existing meeting with `--into` only when it has no recording. With `--into`, preserve the meeting's title, timestamps, and authorship; do not pass creation metadata flags.
- `transcribe` replaces the existing transcript, so confirm before running it on a meeting that already has one. The recording is kept after transcription.
- `sessions note --set` replaces the whole note body. Prefer `--append`, and pass `--set` only when the user explicitly wants the note replaced.
- Pass `--author <agent-name>` (one stable name, e.g. `claude-code`) when creating a meeting with `sessions new` or `import`; leave it unset only when entering a note on the owner's dictation. When a specific skill produced the note, also record it with `--skill <skill-name>` (requires `--author`). Never add, change, or remove the authorship of an existing meeting.
- CLI export may create a separate file. Never pass `--force` unless the user explicitly approves overwriting that exact path.
- Preserve uncertainty when search results are ambiguous. Ask the user to choose between likely meetings.

For setup and failures, see [setup](references/setup.md) and [errors](references/errors.md).

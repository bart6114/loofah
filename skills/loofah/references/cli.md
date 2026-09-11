# CLI commands

Use the CLI for both reading and writing, even when MCP is connected. Pass `--json` for agent-readable output. Start with `loof --version` and `loof --json doctor`; use `loof <command> --help` when you need to verify an option.

(`meetings` is a compatibility alias for `sessions` and will be removed later.)

```bash
loof --json doctor
loof --json sessions list --query "planning" --limit 20 --offset 0
loof --json sessions get MEETING_ID
loof --json sessions note MEETING_ID --kind note
loof --json sessions note MEETING_ID --kind summary
```

`doctor` exits with status 1 when its response contains `ready: false`. Inside a vault it also restores the root `AGENTS.md` agent guide when missing or stale (reported as `agents_md`).

Search across titles, notes, summaries, and transcript words (query and/or `--speaker` required; transcript hits return a `start_ms` matching the transcript's timestamps):

```bash
loof --json sessions search "budget forecast" --limit 20
loof --json sessions search --speaker "bob" --kind transcript
```

Read the full speaker-labeled transcript (`[HH:MM:SS] Speaker: ...` lines):

```bash
loof --json sessions transcript MEETING_ID
```

Filter the meeting list by tags (`--tag` is repeatable and all given tags must match, case-insensitively; `--untagged` keeps only sessions without tags and cannot be combined with `--tag`; each listed meeting's JSON includes its normalized `tags`):

```bash
loof --json sessions list --tag project-x --tag review
loof --json sessions list --untagged
```

Edit a meeting's tags (`sessions tag add` registers new tags in the vault's registry; `sessions tag remove` never unregisters them; tags are normalized — trimmed, `#` stripped, lowercased — and adding a present tag or removing an absent one is a no-op):

```bash
loof --json sessions tag add MEETING_ID project-x review
loof --json sessions tag remove MEETING_ID review
```

List every registered tag in the vault (`tags list`), and resolve a meeting id to its absolute session directory (`sessions path`):

```bash
loof --json tags list
loof --json sessions path MEETING_ID
```

JSON success responses contain `schema_version`, `command`, `data`, and optional `pagination`. Continue from `pagination.next_offset` only when more context is necessary.

Create a meeting note (prints the new meeting id; `--note` seeds the body from a file, or stdin with `-`). `--created-at`, `--started-at`, and `--ended-at` take RFC 3339 timestamps for backdating historical notes (`--created-at` sets the meeting's place on the timeline and in its folder name; invalid timestamps are rejected before anything is written), and `--tag` is repeatable and both tags the meeting and registers new tags in the vault. **Always pass `--author <your-agent-name>` (e.g. `--author claude-code`)** — it marks the meeting as not written by the vault owner, and the app surfaces that; leave it unset only when entering a note on the owner's dictation:

```bash
loof --json sessions new --title "Weekly sync" --note notes.md --author claude-code
echo "Agenda" | loof --json sessions new --title "Weekly sync" --note - --author claude-code
loof --json sessions new --title "Q1 review" --created-at 2024-03-05T14:00:00Z --started-at 2024-03-05T14:00:00Z --ended-at 2024-03-05T15:00:00Z --tag project-x --tag review --author claude-code
```

When a specific skill produced the note, use `--skill NAME` to record it alongside `--author`. This optional flag is supported by both `sessions new` and `import` when creating a meeting, requires `--author`, and cannot be used with `import --into`:

```bash
loof --json sessions new --title "Weekly sync" --note notes.md --author claude-code --skill loofah
```

Edit an existing meeting's note (`--set` replaces, `--append` adds after a separating newline; exactly one of the two, fails if the meeting does not exist):

```bash
loof --json sessions note MEETING_ID --set notes.md
echo "Follow-up" | loof --json sessions note MEETING_ID --append -
```

Attach a file to a meeting's note (prints the stored attachment id; `--name` overrides the stored filename, which otherwise defaults to the file's name). Use the printed id — not the input name — when referencing the attachment: it is deduplicated when the meeting already has an attachment of that name. Notes reference attachments as `![alt](attachments/<id>)` with the id URL-encoded; `--json` returns the ready-to-use `src` alongside `attachment_id` and the vault-relative `path`:

```bash
loof --json sessions attach MEETING_ID diagram.png
loof --json sessions attach MEETING_ID /tmp/export.pdf --name "Q1 report.pdf"
```

Import an audio file as a new meeting (prints the new meeting id; the audio is converted into the vault's format; accepts wav, mp3, ogg, mp4, m4a, flac, webm, or aac; `--title` defaults to the file name; add `--transcribe` to transcribe right after the import). `--created-at`, `--started-at`, and `--ended-at` take RFC 3339 timestamps to backdate a historical recording on the timeline and in its folder name, and `--author` records who created the meeting — always set it when importing as an agent. With `--into MEETING_ID` the audio goes into that existing meeting instead — e.g. a note created with `sessions new` — keeping its title, timestamps, and authorship (`--title`, the timestamp flags, and `--author` are rejected alongside `--into`) and failing if the meeting already has a recording:

```bash
loof --json import recording.m4a --title "Weekly sync" --author claude-code
loof --json import recording.m4a --transcribe --author claude-code --skill loofah
loof --json import recording.m4a --created-at 2024-03-05T14:00:00Z --started-at 2024-03-05T14:00:00Z --author claude-code
loof --json import recording.m4a --into MEETING_ID --transcribe
```

Transcribe a meeting's audio with the on-device model configured in the desktop app (replaces the meeting's transcript; requires the model to be downloaded via the desktop app first; progress goes to stderr; honors the app's audio retention setting — with retention "none", the recording is deleted once the transcript is saved):

```bash
loof --json transcribe MEETING_ID
```

If `import --transcribe` exits non-zero but prints a meeting id, the import succeeded and only the transcription failed — fix the reported problem (usually a missing model or configuration) and run `loof transcribe MEETING_ID`.

Export is intended for an explicit user request to save or transfer a complete meeting:

```bash
loof sessions export MEETING_ID --format markdown --output meeting.md
loof sessions export MEETING_ID --format json --output meeting.json
```

Export refuses to replace an existing file. Pass `--force` only after the user explicitly approves overwriting that exact path.

Global vault overrides:

```bash
loof --vault-path /path/to/vault --json sessions list
loof --base /path/to/vault --json sessions list
```

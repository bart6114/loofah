<p align="center">
  <img src="apps/desktop/src-tauri/icons/stable/128x128.png" width="128" alt="Loofah app icon" />
</p>

<h1 align="center">Loofah</h1>

<p align="center">
  <a href="https://github.com/bart6114/loofah/releases/download/updater/Loofah_latest_aarch64.dmg"><img src="https://img.shields.io/badge/Download_for_macOS-Apple_Silicon-1b2a6b?style=for-the-badge&logo=apple&logoColor=white" alt="Download for macOS (Apple Silicon)" /></a>
</p>

**[Loofah](https://loofah.io) is a free, open-source meeting transcription app for Apple Silicon Macs running macOS 15 or later.** Record without a bot, transcribe on-device, and keep your notes as Markdown files.

I wanted a meeting notetaker that did a few things well: transcribe locally,
keep my notes as ordinary Markdown files, and stay out of the way. I couldn't
find one without accounts, subscriptions, or a cloud service in the middle, so
I built Loofah on [anarlog](https://github.com/fastrepl/anarlog).

<p align="center">
  <a href="docs/public/screenshots/loofah-primary.png">
    <img src="docs/public/screenshots/loofah-primary.png" alt="Loofah showing a meeting brief, notes, and recent sessions in a local vault" />
  </a>
</p>

There is no backend, account, billing, telemetry, or premium tier. Your notes
live on your computer and remain usable without this app. If you want AI-generated
summaries, connect your own LLM—OpenAI, Anthropic, Gemini, OpenRouter, Ollama,
LM Studio, or another OpenAI-compatible provider.

This is software I built for my own day-to-day use. It may be rough around the
edges, but if you want the same kind of tool, I hope it is useful to you too.

## Getting started

Download the signed and notarized Apple Silicon build using the button above.
That link always points to the latest published version; previous versions are
available on the [releases page](https://github.com/bart6114/loofah/releases).
You can also build it from source using the instructions below.

Open the app, start a session, and join your meeting. The app records and
transcribes on your Mac, then saves the notes as Markdown in your vault.

## What matters here

- **The files are yours.** Notes and summaries are Markdown files you can read,
  search, edit, back up, or sync. Transcripts are stored as structured JSON;
  export a session when you want the transcript in Markdown too.
- **Transcription stays local.** Your meeting audio does not need to be sent to
  a transcription service.
- **AI is optional and bring-your-own.** Use a hosted provider or run a local
  model with Ollama or LM Studio.
- **No account or tracking.** Install the app and use it. There is nothing to
  sign up for.
- **CLI and MCP support.** The included `loof` CLI and MCP server can give your
  scripts and coding agents read-only access to meeting notes through MCP.
  The CLI also creates notes, imports recordings, and saves agent work.

Read the [first-meeting guide](https://loofah.io/quickstart/), prepare an
[offline workflow](https://loofah.io/offline/), or
[compare meeting tools](https://loofah.io/compare/).

## Development

This is a pnpm workspace containing a Tauri desktop app (`apps/desktop/`) and a
Rust CLI (`apps/cli/`). There is no database: the files in the vault are the
source of truth. The vault format lives in `crates/vault-read/`; the interface
uses Zustand for state and TipTap for editing.

To run it locally:

```sh
pnpm install
pnpm -F @hypr/desktop tauri:dev   # run the desktop app
cargo build -p loof-cli              # build the loof CLI
```

See [AGENTS.md](./AGENTS.md) for development notes, including formatting,
typechecking, and code-style conventions.

## License

MIT. See [LICENSE](./LICENSE) for the complete license and copyright history,
including the original [anarlog](https://github.com/fastrepl/anarlog)
copyright.

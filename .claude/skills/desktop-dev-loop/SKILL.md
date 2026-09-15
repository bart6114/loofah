---
name: desktop-dev-loop
description: Use when running, restarting, or resetting the desktop dev app, when hunting for its vault, config, logs or downloaded STT/LLM models, when a recording/transcript/summary is missing and you need runtime evidence, or when `dprint fmt` and `cargo check` produce diffs you did not write.
---

# Desktop Dev Loop

Run: `pnpm -F @hypr/desktop tauri:dev` (first Rust build is slow; later ones ~10s).

Cargo builds `target/debug/desktop`, but on macOS `dev-runner.mjs` hardlinks it to
`target/debug/Loofah Dev` and runs that, so the Dock shows the
product name — the running process is the hardlink, not `desktop`.

## Where state actually lives

Dev builds use the **raw bundle id** as the folder name; release builds use
`loofah` (`crates/storage/src/global.rs:19`). For dev that means
`~/Library/Application Support/io.loofah.dev/`.

| What | Where | Notes |
|---|---|---|
| Sessions, `config.json` | *vault base* | Defaults to the app-support dir, but is relocatable — `global.json` in the global base points at it |
| `models/stt/`, `models/llm/` | *global base* — always the app-support dir | Never moves with the vault |
| **Soniqo STT models** | `~/Library/Caches/qwen3-speech/models/aufklarer/` | Neither of the above |
| Logs | `~/Library/Logs/<bundle-id>/app.log` | |

## An empty `models/stt/` does NOT mean "no STT model"

Soniqo models (`soniqo-parakeet-*`, the on-device default) live in their own cache and
never touch `models/stt/`. `LocalStt::is_model_downloaded` branches on the model type:
Soniqo asks `soniqo_download_state`, everything else asks the `models_dir` downloader
(`plugins/local-stt/src/ext.rs`).

Check the real thing before concluding a model is missing:

```bash
du -sh ~/Library/Caches/qwen3-speech/models/aufklarer/*/
```

A ready Parakeet model is ~611 MB with `encoder/decoder/joint.mlmodelc` and `vocab.json`.
**This wipes out the "user has no model installed" diagnosis** — a real session lost hours
to it. `models/llm/` is for summaries only and is irrelevant to transcription.

## Logs are the primary evidence

`app.log` carries backend tracing **and** frontend `console.*` (bridged through
`tauri_plugin_tracing`), so a failed `console.error` in React lands there. Read it before
theorising about a missing recording, transcript, or summary.

```bash
grep -iE "error|warn|stt|transcri|session_" ~/Library/Logs/io.loofah.dev/app.log | tail -40
```

## Clean-slate reset

Move the state aside rather than deleting it — recordings are unrecoverable otherwise.

```bash
pkill -f "target/debug/Loofah Dev"; pkill -f "tauri dev"
mv ~/Library/Application\ Support/io.loofah.dev ~/loofah-backup-$(date +%s)
```

The Soniqo model cache is **not** under that path, so a reset costs no re-download — but
`config.json` is, so the transcription model must be re-selected in Settings.

## Build gotchas

- **`dprint fmt` skips Rust files unless cargo is on PATH** — it prints
  `Cannot start formatter process ... Had N errors formatting` and **still exits 0**, so
  neither `fmt` nor a CI `check` catches it. Always `source "$HOME/.cargo/env"` first.
- **`cargo check` regenerates `apps/desktop/src/types/tauri.gen.ts` unformatted**, producing
  a ~500-line phantom diff. If the command/type sets are unchanged, `git checkout` it.
- Desktop crate is `desktop` (lib `loofah_desktop_lib`): `cargo test -p desktop --lib`.

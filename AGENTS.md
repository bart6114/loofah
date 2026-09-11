# Overview

Tauri desktop note-taking app (`apps/desktop/`) and a CLI (`apps/cli/`).
Uses pnpm workspaces.
Files in the user's vault directory are the only source of truth — there is no database. The vault format lives in `crates/vault-read/`; the desktop write path and in-memory index are `apps/desktop/src-tauri/src/session_store/`. App settings are a `config.json` in the vault, not rows. Zustand is used for UI state, and TipTap powers the editor. Sessions are the core entity — all notes are backed by sessions, stored under `sessions/<id>/`.

## Supported platform

- The desktop app is macOS-only. Design, implement, and QA desktop features for macOS; do not add Windows or Linux compatibility paths unless explicitly requested.
- Shared crates and the CLI may remain portable where they already are, but desktop work does not need cross-platform abstractions solely for hypothetical Windows or Linux support.

## Session directory ownership

Inside `sessions/<id>/` the app owns a fixed set of names (canonical list: `crates/vault-read/src/reserved.rs`): `_meta.json`, `notes.md` (user note; legacy vaults may still have `_memo.md`, readable via fallback and migrated to trash on the next note write), `transcript.json`, `tasks.json`, the recording (`audio.mp3`/`audio.wav`/`audio.ogg`) with `audio.peaks.json` and its `audio_mic.wav`/`audio_spk.wav`/`*.tmp` transients, and the `enhanced/` (AI documents, `enhanced/<uuid>.md`) and `attachments/` (note-embedded files) directories; the `audio/` directory is a legacy recording location. Every other file is a user attachment: the app must ignore it and never enumerate unknown files as content. Dot-prefixed files are never content.

## Commands

- Format: `pnpm exec dprint fmt`
- Typecheck (TS): `pnpm -r typecheck`
- Typecheck (Rust): `cargo check`
- Desktop dev: `pnpm -F @hypr/desktop tauri:dev`
- CLI executable: `loof` (Cargo package: `loof-cli`)
- Docs site: `docs/` (Astro Starlight; published at https://loofah.io via Cloudflare Workers static assets)

## Shared cargo target cache

- In a fresh worktree, run `scripts/setup-shared-target.sh` before the first Rust build. It symlinks the two cargo target dirs (workspace root `target/` and `apps/desktop/src-tauri/target/`) to a machine-shared cache in `~/.cache/loofah/`, so compiled dependencies are reused instead of rebuilt cold (~10 min saved per worktree).
- The script is idempotent and seeds the cache from an existing local build when the cache is empty. Symlinks are used (rather than cargo config) because the checked-in `apps/desktop/src-tauri/.cargo/config.toml` pins `target-dir`, which takes precedence over any parent-directory cargo config.
- Caveats: concurrent builds from different worktrees serialize on cargo's file lock, the cache grows as branches accumulate artifacts (`rm -rf ~/.cache/loofah` resets it at the cost of one cold build), and deleting a worktree no longer frees build space.
- The `.gitignore` patterns for `target` deliberately have no trailing slash — `target/` only matches real directories, not the symlinks.

## Guidelines

- Format via dprint after making changes.
- JavaScript/TypeScript formatting runs through `oxfmt` via dprint's exec plugin.
- Run `pnpm -r typecheck` after TypeScript changes, `cargo check` after Rust changes.
- After editing files, run the relevant verification commands before finishing.
- For `apps/desktop/` TypeScript changes, prefer `pnpm -F desktop typecheck` to match CI.
- After edits, run `pnpm exec dprint fmt`.
- Use `useForm` (tanstack-form) and `useQuery`/`useMutation` (tanstack-query) for form/mutation state. Avoid manual state management (e.g. `setError`).
- Keep file I/O, atomic writes, and index maintenance on the Rust side. TypeScript reads through the typed store commands and subscribes to changes via `useIndexQuery` (`src/shared/index-query.ts`), which fans out the coalesced `index-changed` event — never read or write vault files directly from the frontend.
- Branch naming: `fix/`, `chore/`, `refactor/` prefixes.

## Releases & Versioning

- Every push to `main` auto-releases: `.github/workflows/release.yaml` bumps the version, commits `chore(release): vX.Y.Z [skip ci]`, tags, and creates a GitHub release with generated notes. No binary is built at this point.
- Bump size comes from conventional-commit keywords across the commits since the last `v*` tag (largest wins): `feat!:`/`BREAKING CHANGE:` → major, `feat:` → minor, everything else → patch.
- The version's source of truth is the root `package.json`; `apps/desktop/package.json` is kept in sync by the workflow, and `tauri.conf.json` reads it (`"version": "../package.json"`) — never bump versions by hand.
- Signed + notarized stable DMGs are built on demand: run the `desktop-release` workflow (`gh workflow run desktop-release`, optional `tag` input, defaults to the latest release) and it attaches the DMG + sha256 to that release.
- Ship stable releases through the `desktop-release` skill (preflight, trigger, verify). Release notes are generated by the workflow itself: its `changelog` job has Claude draft `packages/changelog/content/<version>.md` (covering everything since the last shipped version), commits it to `main` with `[skip ci]`, fills the GitHub release body from it, and gates the DMG build on success. Backfill notes for any tag with `gh workflow run changelog -f tag=<tag>`. To fix generated notes, edit the content file on `main` (the app fetches it at runtime), never the release body — the workflow overwrites the body from the file.
- That same workflow also builds the in-app updater artifact (`.app.tar.gz` + minisign `.sig`, signed with the `TAURI_SIGNING_PRIVATE_KEY` repo secret) and refreshes `latest.json` on the rolling `updater` prerelease — the endpoint stable builds poll (`tauri.conf.stable.json`). Never delete the `updater` release; the feed only moves forward (version-guarded against rebuilding old tags).
- `desktop_build.yaml` is the separate staging lane: unversioned DMG artifact on every push to `main`.
- Each push leaves a bot `chore(release)` commit on `main`, so local `main` is behind after every push — `git pull --rebase origin main` before pushing.

## Code Style

- Avoid creating types/interfaces unless shared. Inline function props.
- Do not write comments unless code is non-obvious. Comments should explain "why", not "what".
- Use `cn` from `@hypr/utils` for conditional classNames. Always pass an array, split by logical grouping.
- Use `motion/react` instead of `framer-motion`.

## CLI TUI Command Architecture

Choose the lightest command structure that fits the workflow.

Use the full reducer/effect/runtime split only when the command has async orchestration, a multi-step workflow, or substantial state transitions that benefit from reducer-style tests.

```
commands/<name>/
  mod.rs        -- Screen impl, Args, run()          [glue]
  app.rs        -- App or screen-local state          [optional]
  action.rs     -- Action enum                        [optional]
  effect.rs     -- Effect enum                        [optional]
  runtime.rs    -- Runtime, RuntimeEvent              [async I/O]
  ui.rs         -- draw(frame, app)                   [rendering]
```

Naming rules:

- Types drop the command prefix: `App`, `Action`, `Effect`, `Runtime`, `RuntimeEvent`
- `app.rs` → `app/mod.rs` with private submodules when state is complex
- `ui.rs` → `ui/mod.rs` with sub-files when rendering is complex
- `action.rs`/`effect.rs` are siblings of `mod.rs` when they exist; do not create them by default for simple list/detail screens
- `app.rs` contains no rendering logic, no API calls, no async code when using the reducer pattern
- Prefer `screen.rs` plus a small local state struct for simple browse/select flows
- Do not add parent-level action/effect translation layers that proxy child workflows through another command's reducer

## Misc

- Do not create summary docs or example code files unless requested.

# Plan: human-readable personal session folders

> Superseded by the flat, immutable `sessions/<id>/` cutover. This document records
> the earlier readable-layout design; it is not current implementation guidance.


Status: implemented (all phases, 2026-08-16). Note: contrary to the two-release rollout
sketched below, all phases shipped together — machines sharing an externally synchronized
vault must all run a build with this change before any of them starts the app (startup
migration is the one-way cutover). The `resource_dir` value passed to user hook scripts
(`--resource-dir`) now carries the physical (readable) directory path — call this out in
release notes, since scripts that parse the path to extract a session id will break.

## Goal

Change the personal vault's session directory layout from an opaque folder name:

```text
sessions/550e8400-e29b-41d4-a716-446655440000/
```

to a human-readable folder name:

```text
sessions/2026-03-20 — Product planning — 550e84/
```

The convention is:

```text
YYYY-MM-DD — <sanitized title> — <short session id>
```

The session's full UUID remains its stable application identity in `_meta.json`. The physical directory name is presentation only. Routes, IPC payloads, transcript ownership, search document IDs, CLI arguments, and editor references continue to use the full session ID.

This plan covers the personal vault only. Shared spaces, permissions, cloud providers, and live multi-user editing are out of scope.

## Product decisions

### Keep one folder per session

Only the session directory name changes. The contents remain unchanged:

```text
sessions/2026-03-20 — Product planning — 550e84/
  _meta.json
  _memo.md
  transcript.json
  tasks.json
  audio.mp3
  attachments/
  enhanced/
    <document-id>.md
```

This preserves the existing file format and avoids a second migration for note, transcript, task, audio, attachment, or enhanced-document filenames.

### Identity comes from `_meta.json`, never from the directory name

Today most of the code assumes `directory basename == session id`. That assumption must be removed before any directories are renamed.

A directory is a session directory when it contains a parseable `_meta.json`; `_meta.json.id` is the logical identity. The readable date, title, and short ID must never be parsed to recover the full ID.

Consequences:

- Users may manually move or rename a session folder without changing its identity.
- Two directories claiming the same `_meta.json.id` are an explicit ambiguity and must be reported, never resolved by traversal order.
- A directory with an unreadable `_meta.json` remains untouched and is reported as corrupt. It must not be treated as deleted or migrated.
- The full UUID remains in `_meta.json`; the six-character suffix is only a recognition and collision-avoidance aid.

### Folder names are stable after receiving their first meaningful title

New sessions are frequently created before they have a title. The lifecycle will be:

1. A session created with a title immediately receives its final readable name.
2. A session created without a title starts as:

   ```text
   2026-03-20 — Untitled — 550e84
   ```

3. When that session first receives a non-empty title, rename the provisional directory once.
4. Later title edits do not rename the directory automatically.
5. Later date edits do not rename the directory automatically.
6. A user-initiated filesystem rename is respected.

This gives the common case a useful name while avoiding rename churn, broken external references, and sync-provider conflict copies on every title edit.

A first title can be entered while recording. Renaming the directory while the recorder is active is unsafe because `listener-core`'s `DiskSink` retains paths such as `wav_path` and uses them during finalization. The store must therefore defer the provisional-to-final rename while a session is active and reconcile it after `CaptureLifecycleEvent::Stopped`. Startup reconciliation covers a crash before the stop event.

### Date semantics

The date prefix represents the session's local calendar date at the time the directory receives its stable name:

- Prefer `started_at` when available.
- Otherwise use `created_at`.
- Parse the RFC3339 timestamp and convert it to the machine's local date.
- If parsing fails, use the first valid `YYYY-MM-DD` prefix found in the stored value.
- If neither is usable, use the current local date and report the malformed timestamp in migration diagnostics.

Once chosen, preserve the existing date prefix during the provisional-to-final rename. Moving a vault across time zones must not rename established directories.

For deterministic unit tests, the naming helper should accept an already-resolved date rather than reading the process clock or local timezone internally.

### Title sanitization

The title segment should remain Unicode and human-readable; do not ASCII-slugify it. Apply these rules:

1. Trim leading and trailing whitespace.
2. Collapse internal whitespace, including newlines and tabs, to one space.
3. Replace filesystem-hostile characters (`/`, `\\`, `:`, `*`, `?`, `"`, `<`, `>`, `|`) and control characters with `-` or a space.
4. Collapse repeated separators and whitespace introduced by replacement.
5. Remove trailing periods and spaces for cross-platform compatibility.
6. Use `Untitled` if nothing remains.
7. Normalize the result to Unicode NFC before use.
8. Truncate by UTF-8 boundary so the complete directory component, including date and ID suffix, stays comfortably below the common 255-byte limit. Target a maximum component length of 180 bytes.

The separator is the Unicode em dash surrounded by spaces: ` — `. Titles may themselves contain an em dash; no code may depend on splitting the directory name.

`plugins/path2/src/sanitize.rs` already implements essentially this policy (hostile characters, reserved names, trailing dots/spaces, truncation) but has no in-app caller today; share or adapt that implementation rather than writing a second sanitizer.

### Unicode normalization

The repo currently has no NFC/NFD handling for filenames, which is harmless while directory basenames are ASCII UUIDs. Once basenames contain user titles it becomes a live bug: macOS APFS preserves whatever form it is given, but Drive/iCloud/Dropbox and cross-machine sync can return a differently composed form, and several comparisons are byte-for-byte today (`fs-sync-core/src/session.rs` basename matching, `fs-sync-core/src/audio/mod.rs` known-ID checks, and the `_meta.json.folder` string, which is derived from and compared against physical path segments).

Rules:

- The naming module emits NFC (rule 7 above).
- Every directory-name and folder-path comparison in layout/discovery, `fs-sync-core`, and the watcher classification path compares NFC-normalized values, never raw bytes.
- `_meta.json.folder` values are normalized to NFC on write and on comparison.

### Short ID and collisions

Use the first six lowercase hexadecimal characters of a UUID with hyphens removed, matching the proposed `a1b2c3` form. Production session IDs are UUIDs. For a legacy non-UUID ID, derive six lowercase hexadecimal characters from a stable hash instead of interpolating unsafe ID text.

Before creating or renaming a directory:

1. Try the six-character suffix.
2. If that exact target is occupied by a different session, expand the suffix to eight, then twelve characters.
3. If it is still occupied, use the full UUID without hyphens.
4. Never merge directories and never overwrite an occupied target.

A target containing `_meta.json` for the same full ID is not automatically safe to merge: it indicates a duplicate copy and must be reported for manual recovery.

## Current-state audit

The change is broader than replacing `paths::session_dir(id)`. The flat ID-based path is embedded in every layer that reads, writes, records, watches, deletes, restores, imports, or reveals a session.

### Canonical vault readers

`crates/vault-read/src/paths.rs` builds every session artifact as `sessions/<id>/...`.

`crates/vault-read/src/meta.rs`:

- `read_session_meta` reads `sessions/<id>/_meta.json`.
- `list_session_metas` scans only direct children and treats each child basename as the ID.
- `read_note` and `list_legacy_docs` resolve through the same assumption.

`crates/vault-read/src/enhanced.rs`, `transcript.rs`, and `tasks.rs` also resolve artifacts from the logical ID as though it were the physical directory name.

`crates/agent-access/src/lib.rs` and `search.rs` use those APIs for CLI and MCP reads. They also compute `updated_at` using ID-derived relative paths. Fixing only desktop writes would therefore make sessions disappear from `loof`, MCP, search, export, and doctor commands.

### Desktop write store and in-memory index

`crates/vault-write` is the canonical write path, but it re-exports `vault-read`'s ID-based path helpers.

Important assumptions include:

- `content.rs`: meta/note/document reads and writes, delete, trash, and restore all target `sessions/<id>`.
- `enhanced.rs`: enhanced-document CRUD targets `sessions/<id>/enhanced`.
- `transcript.rs`: live and batch transcript persistence targets `sessions/<id>/transcript.json`.
- `tasks.rs`: task scope resolution and fallback scans treat direct child names as session IDs.
- `audio.rs`: imports and retention mostly use ID-derived paths. It contains partial support for `sessions/<folder>/<id>`, but only when the final directory is still named exactly as the ID.
- `rebuild.rs`: `scan_session_ids` returns direct child basenames, then every refresh reconstructs paths from those names.
- `index.rs`: logical map keys are already full session IDs, which is correct and should not change.
- `lib.rs`: the write journal records physical relative paths, but session write callers currently generate those paths from IDs.

`SessionStore` currently has no session-ID-to-directory map. A location catalog must become part of the store so writes do not repeatedly scan the vault and so a rename can update path resolution atomically.

### File watching

`apps/desktop/src-tauri/src/vault_watch.rs` extracts the second path segment from `sessions/<id>/...` and treats it as the full session ID. With readable directories it would emit a refresh for `2026-03-20 — Product planning — 550e84`, not for the actual UUID.

The watcher also needs to handle:

- an externally renamed session directory;
- a newly copied session directory;
- old and new endpoints from an app-owned rename;
- a bare `sessions` directory event;
- an unknown directory whose `_meta.json` has not been catalogued yet.

Known artifact edits can still refresh one logical ID. Structural or unknown paths should trigger one coalesced session-layout rebuild rather than guessing an ID.

### Recording and transcription

`crates/listener-core/src/actors/recorder/mod.rs` has a second recursive `find_session_dir`, but it also identifies sessions by a directory whose basename equals the UUID. Its fallback creates `sessions/<id>`. `crates/listener-core/src/actors/root.rs` constructs `vault_base().join("sessions")` in three places (the `sessions_base` fed to the recorder's resolver and `emit_session_ended`).

The TypeScript listener bridge in `apps/desktop/src/session/resource-path.ts` constructs `vault/sessions/<id>` for hook `resource_dir` values. `general-live.ts` uses it before and after listening.

`plugins/transcription/src/listener2/ext.rs` writes `transcript.vtt` directly to `sessions/<id>`.

These paths must use the same identity-aware resolver as the store. Duplicated recursive resolvers should be removed rather than taught slightly different variants of the new layout.

### fs-sync plugin and existing personal folders

`crates/fs-sync-core` already supports nested personal folders such as `sessions/Work/<id>`, but identity still depends on UUID basenames:

- `session.rs::find_session_dir` matches the basename.
- `folder.rs` uses UUID checks while scanning and collecting folder updates.
- `path.rs::build_session_dir` always appends the session ID.
- `FsSyncCore::move_session` moves to `<target folder>/<id>`.
- `lib.rs::folder_contains_sessions` gates `delete_folder` on `is_uuid(basename) && _meta.json exists` and recurses only into non-UUID names. After the rename, a personal folder full of readable-named sessions would look empty and `delete_folder` would recursively delete real sessions. This is the most dangerous single call site in the migration and needs an identity-based check plus a regression test before any readable directory can exist.
- `lib.rs::resolve_session_dir` is the façade the plugin commands go through and inherits `find_session_dir`'s basename assumption.
- `audio/mod.rs::delete_orphaned_expired` (orphan-audio retention) identifies session directories by `is_uuid(basename)` and returns basenames as session IDs. Readable directories fail the UUID check and are recursed into as if they were folders, so retention silently breaks — and could scan inside session content (`enhanced/`, `attachments/`).
- `scan.rs::scan_directory_for_files` uses `is_uuid` to decide folder-versus-session, so every readable session directory would be reported to the frontend as a user folder and fully recursed.

`plugins/fs-sync/src/commands.rs` exposes the resolver used by audio, attachments, “Show in Finder,” and `sessionDir` — except `audio_delete_orphaned_expired`, which bypasses `resolve_session_dir`, joins `sessions/` directly, and passes basenames back to JS. The generated bindings also expose `scanAndRead` and `audioDeleteOrphanedExpired` with no in-app callers; both are basename-UUID-dependent and should be fixed or removed rather than left as live-but-dead surface.

The new scanner should preserve existing parent folders. A migration of `sessions/Work/<uuid>` should produce `sessions/Work/<readable name>`, not flatten it to the root. `folder.rs` must read `_meta.json.id` when populating `session_folder_map` and `FolderSessionUpdate`.

### CLI write paths

The CLI creates sessions through `SessionStore`, which is a good foundation, but several commands bypass it for path construction:

- `apps/cli/src/commands/mod.rs` checks collisions at `sessions/<candidate-id>`.
- `import.rs` writes conversion output to `sessions/<id>/audio.mp3`.
- `transcribe.rs` finds and deletes audio in `sessions/<id>`.

All of these must resolve through the shared location API. Read-only CLI and MCP behavior will follow `vault-read`/`agent-access` once those are fixed; that includes `doctor.rs`, whose session-count health check goes through `list_session_metas` and would otherwise report zero sessions.

### Delete, restore, and trash

`SessionStore::delete_session` currently trashes `sessions/<id>`.

`restore_session` assumes trash entries are named `<id>`, `<id>-1`, and so on. Readable directories invalidate that lookup. Delete should retain the exact path returned by `move_to_trash` in an in-memory recent-deletions map, and undo should restore that exact directory to its original readable relative path. It must not rebuild either path from the logical ID. The undo toast is already process-local and short-lived, so crash-resistant trash history lookup is not required.

The existing `.trash/<UTC-date>/<relative path>` behavior is otherwise useful and should be retained.

### Session creation and title timing

Desktop session creation is in `apps/desktop/src/session/queries.ts`. It generates a UUID and initially writes `_meta.json`; many call sites create an empty title. Titles later arrive through:

- title input blur/Enter;
- first-heading edits in the raw note editor;
- AI title generation through `content-mutations.ts`;
- imports, onboarding, and calendar/event flows.

The naming policy must therefore live in Rust's canonical write store, not in one TypeScript creation path. CLI-created and desktop-created sessions must behave identically.

`created_at` is editable through `session-date.tsx`; because established folder names are stable, this remains a metadata-only change.

### Mixed-layout and downgrade compatibility

A readable directory layout is a vault-format change. Builds that only understand `sessions/<id>` cannot read newly named sessions. Symlinks, duplicate compatibility directories, and shadow copies are rejected because they behave poorly with Drive/iCloud and undermine the “files are truth” model.

Rollout should therefore happen in two releases if mixed app versions across machines are a supported scenario:

1. Ship identity-aware readers/writers that can read both layouts but continue creating UUID-named directories.
2. In a later release, enable readable names and migration.

The second release is the one-way cutover. Document that all machines using the same externally synchronized vault should be upgraded before enabling migration. Downgrading after cutover is unsupported unless the directories are renamed back manually.

## Target architecture

### `vault-read`: shared discovery and location model

Add a layout module, for example `crates/vault-read/src/layout.rs`, containing the filesystem identity rules.

Suggested internal model:

```rust
pub struct SessionLocation {
    pub id: String,
    pub relative_dir: PathBuf,
}

pub struct SessionDiscovery {
    pub sessions: Vec<(SessionLocation, SessionMeta)>,
    pub errors: Vec<SessionDiscoveryError>,
}
```

Required operations:

- Recursively scan `sessions/`, skipping hidden directories and never descending into a directory once it has `_meta.json`.
- Parse `_meta.json` to obtain the full ID.
- Return vault-relative physical directories.
- Detect duplicate IDs and expose both paths in an error.
- Resolve one full ID to a location.
- Build artifact paths relative to a `SessionLocation`, not from the logical ID.
- Support both legacy UUID directories and readable directories during the transition.

Keep low-level helpers for fixed artifact names, for example `meta_path_in(session_dir)`, `note_path_in(session_dir)`, and `enhanced_doc_path_in(session_dir, doc_id)`. Deprecate and then remove APIs that imply `session_dir(id)`.

Discovery must be read-only and tolerant: one corrupt session cannot hide healthy sessions. Callers that list sessions receive healthy entries plus diagnostics; a direct lookup returns a distinct not-found, corrupt, or ambiguous result.

### `vault-write`: authoritative location catalog

Extend `SessionStore` with a catalog such as:

```rust
locations: Arc<RwLock<HashMap<String, PathBuf>>>,
recent_deletions: Arc<Mutex<HashMap<String, DeletedSessionLocation>>>,
```

Location values are vault-relative session directories. Add a reverse lookup or longest-prefix lookup for watcher classification. A recent deletion records both the exact trash path returned by `move_to_trash` and the original vault-relative path; it exists only to back the current process's undo toast.

Rules:

- `rebuild_index` refreshes the catalog from `vault-read` discovery before reconciling content.
- A cache miss performs a targeted discovery so `SessionStore::new(...).read_meta(id)` works before an explicit rebuild, as required by CLI commands.
- Session creation chooses a readable directory, writes `_meta.json`, and inserts the location only after the file write succeeds.
- Session rename, delete, restore, and external rebuild update the catalog while holding the store write lock.
- A duplicate ID is an error; do not cache an arbitrary winner.
- Logical index keys remain full IDs.

Session-scoped writes need a primitive that acquires the store write lock, resolves the physical location under that lock, and then writes the artifact. Computing a path before taking the lock would allow a concurrent directory rename to strand a write in the old location or recreate it.

The write journal continues to use physical relative paths. Add a journal operation to discard or remap entries under a renamed directory prefix so late filesystem events cannot be mistaken for current writes at stale paths.

### Naming module

Add a pure naming module in `vault-write`, for example `layout_name.rs`, with:

- `sanitize_title`;
- `short_id_candidates`;
- `format_session_dir_name(date, title, id_suffix)`;
- collision-safe candidate selection;
- recognition of the exact provisional `Untitled` form used by the app.

Do not make parsing a general identity API. Provisional-name recognition is only a lifecycle hint; `_meta.json.id` remains authoritative.

### Resolver use outside the store

Prefer one shared resolver implementation from `vault-read` over the duplicated finders in `fs-sync-core` and `listener-core`.

Where a component already has access to `SessionStore`, expose a store-backed `session_dir` operation so it uses the warm catalog. Components that are intentionally independent, such as the CLI read layer, may call the shared read-only resolver directly.

## Implementation phases

### Phase 1: introduce dual-layout discovery without changing any names

1. Add `vault-read::layout` and its typed location/discovery errors.
2. Refactor `vault-read` meta, note, transcript, tasks, enhanced-document, and legacy-document readers to operate on discovered locations.
3. Refactor `agent-access` to carry a `SessionLocation` while assembling a meeting so `updated_at` reads the real file path.
4. Add fixtures covering:
   - `sessions/<uuid>`;
   - `sessions/<readable-name>`;
   - `sessions/Work/<uuid>` and `sessions/Work/<readable-name>`;
   - manually renamed directories;
   - mismatched basename and metadata ID;
   - corrupt metadata;
   - duplicate full IDs.
5. Keep session creation on UUID directory names during this phase.

Acceptance criterion: desktop, CLI, MCP, and agent search read old and readable fixtures identically by full ID.

### Phase 2: make `SessionStore` location-aware

1. Add the location catalog to `SessionStore`.
2. Change startup rebuild to scan `(id, physical directory)` pairs rather than child basenames.
3. Refactor all session-scoped store reads and writes:
   - meta and raw notes;
   - legacy and enhanced documents;
   - transcript buffers and batch replacement;
   - session tasks and enhanced-note task scope fallback;
   - audio import/list/delete;
   - delete and restore.
4. Make every session-scoped path resolve while the write lock is held.
5. Preserve physical parent folders from the existing personal-folder model.
6. Extend `RebuildReport` with layout diagnostics or include them in `errors` with physical paths.
7. Keep logical index notifications unchanged: all events still carry full IDs.

Acceptance criterion: run the complete `vault-write` test suite against both UUID-named and readable directories, including concurrent writes and cold rebuilds.

### Phase 3: update watcher and all path consumers

1. Replace `vault_watch`'s second-segment ID parser with catalog-backed physical-path lookup.
2. Add a structural action, such as `RebuildSessions`, for unknown/new/renamed session directories and the bare `sessions` root.
3. Coalesce structural events so one directory rename causes one rebuild, not one per artifact.
4. Update `fs-sync-core`:
   - discover sessions by `_meta.json.id`;
   - return full IDs in folder maps;
   - preserve readable basenames during moves between personal folders;
   - stop requiring a UUID basename anywhere: `folder_contains_sessions` (guards `delete_folder` — regression-test that a folder of readable sessions blocks deletion), `resolve_session_dir`, orphan-audio retention in `audio/mod.rs` (must return full IDs, not basenames), and `scan.rs` folder-versus-session classification.
5. Replace `listener-core`'s duplicate recursive finder with the shared resolver, including the `sessions_base` construction sites in `actors/root.rs`.
6. Change `apps/desktop/src/session/resource-path.ts` and `general-live.ts` to request the actual session directory rather than constructing it in TypeScript. The existing `fsSyncCommands.sessionDir` command can remain the frontend boundary once its resolver is fixed. Note this changes the `resource_dir` value passed to user hook scripts (`--resource-dir`); the code fix is in scope here, but call it out in release notes as a user-facing contract change since scripts may parse the path.
7. Update `plugins/transcription` VTT export to resolve the real directory.
8. Update CLI collision checks, imports, transcription audio lookup, and retention deletion. Route `audio_delete_orphaned_expired` in `plugins/fs-sync/src/commands.rs` through the resolver, and fix or remove the caller-less `scanAndRead`/`audioDeleteOrphanedExpired` bindings.
9. Audit “Show in Finder,” attachments, audio player/import, hooks, and export paths through integration tests. On an app-driven rename, invalidate frontend query caches holding absolute paths (`sessionDir`, `audioPath`, attachment path lists) — nothing on disk stores physical session paths (the Tantivy index stores only IDs; persisted attachments strip `src`/`path`), so transient caches are the only staleness.

Acceptance criterion: no production code outside the centralized layout modules constructs `sessions/<session-id>` directly. Enforce this with a narrow repository grep in review or a lint-style test if practical.

### Phase 4: enable readable names for newly created sessions

1. Add and test the naming/sanitization helper.
2. Change `SessionStore::write_meta` creation behavior:
   - resolve by full ID first;
   - if absent, choose a collision-free readable directory;
   - create `_meta.json` there;
   - register its location.
3. Do not let callers supply a physical folder name over IPC.
4. Verify all creation paths without special-casing them:
   - desktop blank note;
   - desktop recording;
   - onboarding welcome note;
   - audio import;
   - calendar/detection flow;
   - `loof meetings new`;
   - `loof import`.
5. Add end-to-end assertions that returned IDs remain full UUIDs even though the physical folder is readable.

Acceptance criterion: every newly created session is immediately usable through desktop, CLI, MCP, recording, transcription, attachments, delete/undo, and Finder reveal.

### Phase 5: implement provisional-title reconciliation

1. Track active recording IDs in `SessionStore` from `mark_recording_started` and `mark_recording_ended`, or add an equivalent path-lease mechanism.
2. On a title patch, detect whether the current physical basename is the app's provisional `Untitled` name.
3. If no recording is active, rename it once to the sanitized non-empty title.
4. If recording is active, leave it in place and mark it for reconciliation.
5. On `mark_recording_ended`, reconcile after recorder finalization.
6. On startup, reconcile provisional directories whose metadata already has a non-empty title. This handles crashes and title writes from older code paths.
7. Update the location catalog and write journal under the write lock during rename.
8. If the target is occupied, use the suffix expansion rules. If rename fails, keep the metadata update, report the path error, and retry during startup reconciliation; the title is user data and must not be rolled back merely because its presentation rename failed.

Acceptance criterion: typing or generating the first title during a recording never interrupts audio finalization, and the folder becomes readable after the recording stops or the app restarts.

### Phase 6: migrate legacy UUID-named directories

Run migration only after all dual-layout code has shipped and passed production validation.

1. Add an idempotent `migrate_legacy_session_directories` operation.
2. Preflight the complete source-to-target set before renaming anything:
   - only migrate a directory whose basename is exactly its full metadata ID;
   - leave custom/readable names untouched;
   - preserve the existing parent personal folder;
   - skip corrupt or duplicate identities;
   - select collision-free targets.
3. Rename within the same parent using `std::fs::rename`; do not copy/delete and do not rewrite session contents.
4. Execute at desktop startup before the store's first index rebuild and before `vault_watch` starts.
5. Make partial completion safe: the next startup skips already-renamed directories and continues the remainder.
6. Return a report containing renamed, skipped, and failed physical paths. Log it and surface failures in the existing storage diagnostics/rebuild UI if appropriate.
7. Keep CLI readers dual-layout permanently. Read-only CLI commands must never migrate as a side effect.

Do not use a “migration complete” marker as the sole guard. The physical layout is the truth, and an idempotent scan also handles a legacy session copied into the vault later.

Acceptance criterion: an existing vault is renamed without changing file contents or logical IDs, and repeated migration runs are no-ops.

## Delete and restore design

Delete and undo need dedicated regression coverage because their current algorithm is name-based.

### Delete

1. Resolve the session's current physical relative directory under the store lock.
2. Remove its live transcript buffer as today.
3. Move that exact directory through `move_to_trash` and retain the returned trash path.
4. Record `{ id, original_relative_dir, trash_path }` in the store's recent-deletions map.
5. Remove the ID from the location catalog and all logical index maps only after the move succeeds.

### Restore

1. Look up the exact recent-deletion record for the full session ID.
2. Verify that the trash directory still contains a parseable `_meta.json` with the requested full ID.
3. Use the recorded original relative directory as the destination.
4. If the destination is occupied, fail safely rather than merging.
5. Rename the directory back, remove the recent-deletion record, register its location, and call `refresh_session(id)`.
6. Return `Ok(false)` if there is no recent-deletion record or its trash path has disappeared, matching the current expired-undo behavior.

No on-disk tombstone is needed: after an app restart the undo toast no longer exists, while the trashed directory remains available for manual recovery. Tests must include two same-day delete/restore cycles, a title-containing directory, a nested personal folder, a tampered trash entry, and an occupied restore destination.

## Watcher behavior after cutover

The watcher should classify paths using physical locations:

```text
sessions/2026-03-20 — Product planning — 550e84/_memo.md
        └──────────────── session directory ───────────────┘
```

Expected outcomes:

- Known artifact path + journal hash matches: ignore as an own write.
- Known artifact path + hash differs: refresh the full logical ID from the catalog.
- Unknown direct/nested session path: rebuild session discovery/index.
- Session directory old/new rename endpoint: rebuild session discovery/index.
- Bare `sessions` event: rebuild session discovery/index.
- `.trash/**`: continue ignoring.
- Temp siblings: continue ignoring.

A rebuild remains read-only on the vault. An incorrectly classified event may cause extra work but must never delete or rewrite files.

## Test plan

### Naming unit tests

Cover:

- normal ASCII title;
- Unicode accents and emoji;
- an NFD-composed input title produces an NFC directory name, and NFD/NFC variants of the same name compare equal in discovery;
- slash, backslash, colon, wildcard, quotes, and control characters;
- multiline and repeated whitespace;
- empty/punctuation-only title;
- title containing ` — `;
- a multi-byte title near the byte limit;
- UUID and legacy non-UUID IDs;
- six/eight/twelve/full suffix collision fallback;
- local date supplied explicitly;
- stable provisional-to-final rename.

### Layout/discovery tests

Cover:

- old and new root layouts;
- old and new nested personal-folder layouts;
- a custom manually renamed session;
- metadata ID differing from the basename;
- corrupt `_meta.json` next to healthy sessions;
- duplicate IDs in separate directories;
- hidden and atomic-temp directories;
- a session directory that must not be recursively scanned as a parent folder.

### Store integration tests

Run the existing content, enhanced document, transcript, task, audio, index, rebuild, delete, and restore cases against readable physical paths. Add regressions for:

- write after directory rename lands only in the new directory;
- title update racing note/transcript writes cannot recreate the old directory;
- external directory rename followed by rebuild preserves the logical index entry;
- active recording defers provisional rename;
- stop reconciles the deferred rename;
- startup reconciles after a simulated crash;
- delete and restore preserve the readable name;
- duplicate IDs block writes rather than selecting a random directory;
- write journal behavior remains silent for normal repeated writes after rename.

### CLI and agent-access tests

Update CLI tests that enumerate direct `sessions/` children or hard-code `sessions/<id>`. Verify:

- `meetings new` prints a full UUID and creates a readable directory;
- `import` and `transcribe` find audio in that directory;
- `meetings get/list/search/export`, MCP, and doctor read both layouts;
- file-derived `updated_at` comes from the real physical artifact;
- read-only commands do not trigger migration.

### Desktop integration tests

Verify:

- create/open/edit;
- record/stop and post-capture batch transcription;
- first title entered before, during, and after recording;
- AI-generated first title;
- audio upload and transcript upload;
- attachment save/read/remove;
- Show in Finder;
- hook `resource_dir` before and after recording;
- delete/undo;
- external Finder rename followed by focus/watcher rebuild;
- Tantivy search keeps the same full-ID document key.

## Verification commands

After each Rust phase:

```sh
cargo test -p vault-read -p vault-write -p fs-sync-core -p listener-core -p loof-cli
cargo check
```

After desktop TypeScript changes:

```sh
pnpm -F desktop typecheck
```

After all edits:

```sh
pnpm exec dprint fmt
```

Also perform a manual round trip with a copied vault:

1. Start with UUID-named sessions, including one with audio and enhanced notes.
2. Run migration.
3. Open and edit the note.
4. Record, stop, and transcribe.
5. Run `loof meetings get <full-id>` and MCP lookup.
6. Reveal in Finder.
7. Delete and undo.
8. Restart and rebuild the index.
9. Confirm no UUID-named directory was recreated and no content bytes changed merely because its parent directory was renamed.

## Non-goals

- Shared/team spaces or provider integrations.
- Real-time collaborative editing.
- Renaming `_memo.md`, `_meta.json`, `transcript.json`, or enhanced-document files.
- Replacing full UUIDs in APIs, routes, links, search IDs, or metadata.
- Automatically renaming an established directory on every title or date edit.
- Flattening existing personal folder organization.
- Creating symlinks or duplicate compatibility copies.
- Inferring identity from the readable directory name.

## Definition of done

- Every new personal session directory follows `YYYY-MM-DD — title — short-id`.
- Existing UUID-named personal session directories can be migrated idempotently.
- Full IDs remain stable and are read from `_meta.json`.
- Desktop, CLI, MCP, recording, transcription, search, tasks, audio, attachments, Finder reveal, delete, and undo work in both layouts.
- A first title received during recording is reconciled without disrupting audio finalization.
- External/manual folder renames are discoverable and do not change logical identity.
- No production code outside the central layout implementation assumes `session directory basename == session ID`.
- Migration never merges, overwrites, or deletes session content.

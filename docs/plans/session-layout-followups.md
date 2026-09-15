# Plan: finish the session-layout work

> Superseded by the flat, immutable `sessions/<id>/` cutover. This document records
> the earlier readable-layout design; it is not current implementation guidance.


Status: implemented (branch `refactor/session-layout-followups`)

Implementation notes (deviations from the text below, found during verification):

- The stop-side race description was inaccurate: `AfterListeningStopped` was
  driven by `stopCapture()` resolving, not by the `Stopped` lifecycle event. The
  `RecordingMetaSettled` design fixes the actual (earlier, wider) race, and the
  same settlement also gates the `onStopped` audio-path consumers (batch repair,
  audio cataloging), which the plan text had left ungated.
- The `locations` frontend subscriber is mounted per window (not in the
  main-window-gated listener host): standalone session windows cache paths too.
- Discovery still descends past a ghost boundary so a healthy session nested
  under a ghost stays indexed (the boundary is only reported once); "is not
  descended into" applies to ghost *reporting*, preserving CLI listing
  semantics exactly.
- Pre-existing dot-named folders keep an escape hatch: rename-to-visible and
  delete accept a hidden source; creating or renaming into hidden paths is
  rejected as planned.
- Accepted trade-off: fs-sync duplicate-id blocking is as-of-last-rebuild
  rather than per-call, inherent to catalog-first resolution and bounded by
  the watcher rebuild.

## Delivery rule

Implement this plan as one complete effort. The work may be split into reviewable commits in the order below, but nothing in the plan is optional and the change is not complete until every workstream and acceptance test has landed.

The scope is intentionally limited to problems created or exposed by the readable-session-folder layout. It does not include unrelated cleanup.

## Context

The readable-session-folders feature shipped in PR #29 (`v0.16.0`; see `docs/plans/human-readable-personal-session-folders.md`). The implementation is fundamentally sound: `_meta.json.id` is identity, the store has an ID-to-physical-directory catalog, reads support legacy and readable layouts, and migration is idempotent.

The remaining work falls into four categories:

1. Desktop fs-sync commands ignore the warm store catalog and recursively scan the whole vault for routine path lookups.
2. Physical-path caches and recording hooks can observe stale paths when a provisional directory is renamed.
3. A few layout boundary, traversal, lookup, and naming rules remain duplicated.
4. Startup and rebuild repeat layout scans that can be shared safely.

These are not known data-loss bugs, but the stale-path cases are correctness issues and the repeated full scans are a user-facing performance regression. The plan below fixes all of them without changing naming, identity, migration, or delete/undo behavior.

## Current behavior and verified findings

### Routine fs-sync lookups perform full vault discovery

`plugins/fs-sync/src/commands.rs::resolve_session_dir` calls `hypr_fs_sync_core::session::find_session_dir` for audio existence, metadata, peaks, path, import, `sessionDir`, session-content loading, and direct deletion.

After migration, the resolver's `sessions/<id>` fast path always misses. It then calls `hypr_vault_read::find_session`, which recursively walks `sessions/` and parses every `_meta.json`. A mounted note normally requests at least audio existence, audio path, and audio peaks, so one note open can pay for several full scans.

Attachment commands and folder operations go through `FsSyncCore::resolve_session_dir` and pay the same cost. `FsSyncCore` is reconstructed per plugin call, so caching inside it would not help.

`SessionStore` already has a warm location catalog populated by startup rebuild and maintained by store writes, app-driven renames, delete/restore, fs-sync move-triggered rebuilds, watcher rebuilds, and focus rebuilds.

### The current core fallback is more tolerant than `SessionStore::session_dir`

The optimization must not blindly replace the core resolver with `SessionStore::session_dir`.

`fs-sync-core::find_session_dir` has a hardening fallback that recursively finds a UUID-named directory whose `_meta.json` is corrupt or missing. This keeps audio and attachments with a nested legacy recorder ghost instead of forking them into a new root `sessions/<id>` directory.

`SessionStore::session_dir` does not perform that `find_unclaimed_dir_named` fallback. Therefore the safe optimization is:

- use the store only for a validated catalog hit;
- fall back to the existing fs-sync resolver on a cache miss or stale entry.

This preserves all current corrupt/ghost behavior.

### The catalog is warm but not literally always current

An external Finder or sync-provider rename can make the catalog stale until the watcher/focus rebuild runs. A cached resolver must validate that the cataloged directory still exists before using it. If parseable metadata at that path now claims a different ID, it is stale and must fall through. If metadata became corrupt, the existing catalog claim remains usable, matching current corruption tolerance.

### Absolute-path query data can become stale

A provisional directory can rename after a title write. The frontend caches:

- audio URL/path under `['audio', sessionId, 'url']`;
- audio existence and peaks under the same `['audio', sessionId]` prefix;
- attachment absolute paths and converted asset URLs under `['session', sessionId, 'attachment-paths']`.

Invalidating only one title mutation site is insufficient: titles can be persisted through `queries.ts`, `content-mutations.ts`, enhanced-note title propagation, onboarding, and AI title generation. It would also miss manual external directory renames.

The backend catalog knows when a physical location actually changes, so location changes need their own index event and one centralized frontend invalidation path.

### Recording has start-side and stop-side path races

The existing active-recording guard is registered from `CaptureLifecycleEvent::Started`. That event fires only after the recorder has resolved and opened its directory. A title write can therefore race in the interval between capture startup and the Started event.

`BeforeListeningStarted` resolves `resource_dir` before starting capture. A long-running hook widens the same window: the provisional directory can rename while the hook is using it.

At stop, `listenerCommands.stopCapture()` returns after requesting shutdown, not after recorder finalization. `CaptureLifecycleEvent::Stopped` then causes both of these independently:

- the frontend starts resolving `resource_dir` for `AfterListeningStopped`;
- `recording_meta.rs` asynchronously calls `mark_recording_ended`, which may rename the provisional directory.

The post-stop hook can consequently receive a path that disappears immediately afterward.

### Layout invariants remain duplicated

The remaining meaningful duplication is:

- session-directory classification in `vault-read/layout.rs` and `fs-sync-core/session.rs`;
- recursive traversal rules across vault discovery, folder listing, generic scan, orphan-audio retention, and folder-update collection;
- readable-name candidate construction in create, provisional reconciliation, and migration;
- existing-location lookup in `SessionStore::resolve_session_dir` and `creation_dir_locked`.

Low-value style cleanup and cross-crate test-fixture frameworks are deliberately not part of this plan.

### Startup and rebuild repeat work

Desktop startup currently performs:

1. `migrate_legacy_session_directories` → full discovery;
2. `reconcile_provisional_names` → full discovery;
3. `rebuild_index` → full discovery plus a second ghost-directory walk.

Every focus rebuild also runs discovery and then the separate ghost walk.

The discovery pass already visits every relevant directory. It can report ghost directories during that walk. Startup migration and reconciliation can share one mutable discovery snapshot, update physical paths after successful renames, and hand the final snapshot to rebuild.

`refresh_one` should continue rereading `_meta.json`. Reusing discovery's parsed metadata after dropping the store lock would widen the race with external editors during startup/focus rebuild and could index stale metadata without a later watcher event. Saving that one read is not worth weakening consistency.

## Workstreams

### 1. Catalog-first fs-sync resolution with exact fallback semantics

### Store API

Add a synchronous cache-only method in `crates/vault-write/src/locations.rs`:

```rust
pub fn session_dir_cached(&self, id: &str) -> Result<Option<PathBuf>, StoreError>
```

It must:

1. validate the logical session ID;
2. reject IDs in `known_duplicates`;
3. read the location catalog without running discovery;
4. return `None` when there is no catalog entry;
5. validate a hit against the filesystem:
   - missing directory → `None`;
   - missing `_meta.json` → `None`;
   - parseable `_meta.json` with a different NFC-normalized ID → `None`;
   - parseable matching metadata → `Some(relative_dir)`;
   - unreadable/corrupt `_meta.json` → retain `Some(relative_dir)`, because the warm catalog is still the best known home and current artifact access intentionally tolerates corruption.

This remains O(1) in session count: at most one catalog lookup and one metadata read.

Also change the async cold-miss path in `SessionStore` to warm all healthy, non-duplicate locations from the discovery it already paid for, instead of caching only the requested ID.

### FsSyncCore resolver override

Add an optional resolver override to `FsSyncCore`:

```rust
type SessionDirResolver =
    Arc<dyn Fn(&str) -> Result<Option<PathBuf>> + Send + Sync>;
```

Provide constructors equivalent to:

```rust
FsSyncCore::new(base_dir)
FsSyncCore::with_resolver(base_dir, resolver)
```

`FsSyncCore::resolve_session_dir` must preserve its current UUID validation before consulting the override, then:

1. return a resolver hit verbatim;
2. propagate a resolver error, including duplicate-ID errors;
3. on `Ok(None)`, call the current `find_session_dir` unchanged.

Keeping validation outside the override prevents a warm store catalog from making fs-sync accept non-UUID IDs that it rejects today.

The plugin's `ext.rs::core()` installs an override backed by the managed `Arc<SessionStore>` when present. It converts the store's vault-relative hit to an absolute path. Plugin tests and standalone/core users continue to use `FsSyncCore::new` and retain current behavior.

The override is also used by `move_session`. This is safe because stale entries return `None`, source semantics still fall back to fs-sync discovery, and the command continues rebuilding the store catalog immediately after a successful move.

### Direct fs-sync commands

Keep `plugins/fs-sync/src/commands.rs::resolve_session_dir` synchronous. Preserve the current UUID validation, then use the same cache-only store lookup and fall back to the existing `find_session_dir` implementation. Do not call async `SessionStore::session_dir` here.

No Tauri command signature or generated TypeScript binding changes are needed for this resolver work.

### Tests

Add tests proving:

- a valid cached readable directory is returned without invoking fallback;
- a missing cached directory falls through;
- a cached path whose parseable metadata claims another ID falls through;
- a cached path whose metadata became corrupt remains usable;
- a duplicate ID returns an error;
- `FsSyncCore` uses an injected resolver hit for attachments and moves;
- resolver `None` preserves nested corrupt/meta-less UUID-directory adoption;
- resolver errors are not silently converted into fallback paths.

Instrument one integration test with a counting fallback closure so the warmed command path proves it performs zero full discovery calls.

### 2. Make physical-location changes observable to frontend caches

Add `Locations` to `crates/vault-write/src/index.rs::IndexEntity` and to the stable coalescing order.

Emit `IndexEntity::Locations` only when an ID's physical directory changes:

- `catalog_insert` compares old and new NFC-normalized paths before notifying;
- `catalog_remove` notifies when an entry existed;
- rebuild compares the previous and replacement catalogs and notifies changed/added/removed IDs;
- app-driven rename, migration, provisional reconciliation, delete/restore, fs-sync moves, and external watcher/focus rebuilds consequently converge on the same event.

Search projection continues ignoring this entity because search documents contain logical IDs and content, not physical paths.

In the desktop frontend, add one mounted subscriber for `locations` events. For every changed session ID, invalidate:

```text
["audio", sessionId]
["session", sessionId, "attachment-paths"]
```

This replaces mutation-site-specific invalidation. It covers manual titles, generated titles, onboarding, external Finder renames, personal-folder moves, delete/restore, and any future rename path.

Tests must verify event coalescing and exact query-prefix invalidation. Manual QA must cover an untitled session with both imported audio and an image attachment: title it and confirm playback, waveform, and image rendering continue using the renamed directory.

### 3. Sequence recording path leases and hook paths

### Prepare before the pre-start hook

Replace the boolean `active_recordings` set with a small per-session lease count. A plain set is insufficient once both the frontend and the transcription command can reserve the path: a failed duplicate start must release only its own reservation, not clear the marker protecting an already-active recording.

Add store operations and desktop IPC commands:

```rust
prepare_recording(session_id) -> Result<PathBuf, StoreError>
release_recording_prepare(session_id) -> Result<(), StoreError>
```

`prepare_recording` acquires the store write lock, resolves the current session directory, and increments that session's lease count before releasing the lock. The Tauri command returns the absolute directory path.

`release_recording_prepare` acquires the write lock, decrements only one lease, and retries provisional-name reconciliation only when the count reaches zero. It is safe to call from paired failure cleanup. The Started lifecycle ensures the count is at least one without incrementing an existing lease; the Stopped lifecycle clears the session's leases after recorder finalization.

Change `startLiveSession` to:

1. prepare the recording and receive the stable physical path;
2. pass that path to `BeforeListeningStarted`;
3. call `startCapture`;
4. on any failure before capture starts, release the frontend's recording preparation.

Keep the existing synchronous `note_recording_active` call on the Started lifecycle event as an idempotent defense for non-frontend capture callers.

Also have the transcription plugin's `start_capture` command acquire its own lease when a managed store is available, before it asks listener-core to start. On startup failure it releases only that lease. The frontend releases its separate lease through its own failure cleanup; successful starts retain their leases until Stopped clears the session. This closes the recorder-open-to-Started race for callers that bypass `startLiveSession` without allowing a failed duplicate start to unprotect a real recording. Add `hypr-vault-write` as a workspace dependency of `tauri-plugin-transcription`; it introduces no dependency cycle, and `try_state` preserves standalone plugin behavior when no store is managed.

### Settle metadata before the post-stop hook

Add a desktop Tauri event:

```rust
RecordingMetaSettled {
    session_id: String,
    succeeded: bool,
}
```

`recording_meta.rs` emits it after every Stopped-event attempt, including missing-store and failed-meta-write branches. The event means the rename attempt has finished, not that it necessarily succeeded.

Before calling `stopCapture`, the frontend installs a one-shot waiter for the matching session ID. After requesting stop, it waits for `RecordingMetaSettled` before resolving `resource_dir` and invoking `AfterListeningStopped`. Registering the listener first prevents an instant finalization from being missed.

Use a bounded timeout as a crash/fault safeguard, not as ordering logic. A normal path must always complete by event. On timeout, log a warning, resolve the directory afresh, and continue so a broken event listener cannot permanently wedge stop handling.

Tests must cover:

- prepare blocks a first-title rename;
- releasing the final lease reconciles the pending title;
- releasing one of multiple leases does not unprotect the session;
- successful stop clears all leases and reconciles;
- start failure does not leave a permanent lease;
- a failed duplicate start does not clear an existing recording's protection;
- the waiter is installed before `stopCapture`;
- matching event resolves it;
- unrelated session events are ignored;
- timeout continues with a warning;
- both successful and failed metadata stamping emit `RecordingMetaSettled`.

Manual QA: configure both before/after hooks, title the session while each lifecycle is in progress, and confirm each hook sees a directory that is not moved by the app during its phase. An unrelated external process can still rename a folder at any time and is outside this lease guarantee.

### 4. Centralize the layout invariants

### Shared directory classification

Expose a stable classifier from `crates/vault-read/src/layout.rs` rather than making the current private implementation details public verbatim.

The shared result must preserve the key boundary rule:

- `_meta.json` absent → personal folder;
- `_meta.json` present and parseable → session with `SessionMeta`;
- `_meta.json` present but unreadable/corrupt → corrupt session boundary, never a folder.

Use it from vault discovery and map it in fs-sync-core. Remove fs-sync-core's duplicate metadata reader.

Also expose a cheap shared `has_session_boundary(dir)` helper for callers that only need “may recurse” versus “must stop.” It probes `_meta.json` without parsing: present is a boundary, `NotFound` is a folder, and any other stat/read error is conservatively a boundary. Do not use `Path::exists()`, which hides permission errors as absence. Use the full classifier only where the metadata ID is needed. This avoids both duplicated rules and needless JSON parsing in generic scan/folder-delete paths.

### One hidden-directory rule

Every recursive traversal rooted at `sessions/` must skip dot-prefixed directories before classification or recursion:

- `vault-read` discovery;
- fs-sync folder listing;
- folder-update collection;
- `folder_contains_sessions`;
- generic `scan_and_read` traversal;
- orphan-audio retention traversal;
- unclaimed UUID-directory fallback;
- ghost detection.

Reject dot-prefixed segments in `normalize_folder_path` so the app cannot create or rename a personal folder into a part of the tree that every layout reader intentionally ignores.

Add regression fixtures containing `.stversions`, `.trash`, and `.tmp-*` directories. Their contents must not appear as sessions/folders, be scanned as artifacts, block ordinary folder operations, or be considered orphan audio.

### One candidate generator

Add one pure helper that returns readable directory candidates:

```rust
session_dir_candidates(parent, date, title, id) -> Vec<PathBuf>
```

It owns the invariant:

```text
6-character suffix → 8 → 12 → full hex
```

Creation, provisional reconciliation, and migration each call this helper and retain only their genuinely different occupancy rules:

- creation's legacy ghost adoption;
- reconciliation's current-directory NFC self-filter;
- migration's preflight `claimed` set.

Do not hide those differences behind a generic callback abstraction.

Simplify `short_id_candidates` with `Vec::dedup`. Keep the current permissive UUID-shaped check and document why: changing to `Uuid::try_parse` would alter the stable hashed fallback for legacy IDs with noncanonical dash placement without providing a correctness benefit.

### One existing-location lookup

Extract a private `lookup_existing_dir(id) -> Result<Option<PathBuf>, StoreError>` used by both `resolve_session_dir` and `creation_dir_locked`. It owns:

- validation and duplicate rejection;
- catalog lookup;
- full discovery on a cold miss;
- catalog warming;
- corrupt and ambiguous error mapping.

The callers differ only for `None`:

- normal artifact lookup → legacy `sessions/<id>` fallback;
- creation → collision-free readable candidate selection.

All existing dual-layout and corruption tests remain behaviorally unchanged.

### 5. Perform one layout traversal per rebuild/startup normalization

### Report ghosts during discovery

Extend `SessionDiscovery` with ghost-directory diagnostics gathered during its existing traversal. A meta-less directory that directly contains a recognized session artifact (`*.md` or `transcript.json`) is reported as one ghost boundary and is not descended into.

Remove `vault-write::rebuild::scan_ghost_dirs`. `scan_session_locations` derives healthy locations, duplicate IDs, broken directories, and ghosts from one discovery result.

This does not alter CLI listing semantics; consumers that do not care about ghosts ignore the additional field.

### Share one startup snapshot

Add a store-level startup operation, for example:

```rust
normalize_startup_layout() -> Result<StartupLayoutSnapshot, StoreError>
```

It:

1. acquires the store write lock;
2. runs discovery once;
3. preflights and performs legacy migration from that snapshot;
4. performs provisional-name reconciliation from the updated snapshot;
5. updates snapshot paths after each successful rename;
6. refreshes the location and duplicate catalogs;
7. returns the final snapshot plus migration/reconciliation diagnostics.

Add an internal `rebuild_index_from_layout(snapshot)` used immediately afterward. Normal watcher/focus/manual rebuild calls discovery once and delegates to the same internal reconciliation code.

Do not hold the store write lock while reading every note, transcript, enhanced document, and task. Preserve the current lock boundary: protect layout scan/catalog swap/renames, then release before content refresh.

Keep per-session `_meta.json` rereads in `refresh_one` for consistency with external filesystem edits.

### Warm the catalog on a paid cold scan

The shared existing-location lookup must insert every healthy non-duplicate location from a cold discovery, not only the requested ID. A second lookup then becomes a validated cache hit.

### Tests and measurement

Add tests proving:

- startup migration + provisional reconciliation call discovery once;
- a normal rebuild calls discovery once and has no ghost second walk;
- renamed paths in the startup snapshot are the paths rebuild uses;
- migration failures retain source paths in the snapshot;
- corrupt/duplicate protection and stale-index pruning remain unchanged;
- unchanged rebuild still emits no content-index events;
- location changes emit only `Locations` events when file content is unchanged.

Add lightweight tracing around discovery with duration and healthy/corrupt/duplicate/ghost counts. Compare startup and focus-rescan traces on fixtures with 100, 1,000, and 5,000 sessions. This is measurement, not a benchmark gate.

## Explicitly unchanged

- Session identity remains `_meta.json.id`.
- Directory naming and one-shot provisional rename policy do not change.
- Migration remains idempotent and never merges occupied directories.
- Delete/restore and trash behavior do not change.
- Legacy UUID layouts, nested personal folders, corrupt metadata tolerance, duplicate blocking, and recorder ghost fallback remain supported.
- `FsSyncCore` remains usable without Tauri or `SessionStore`.
- No persistent cache or database is introduced.
- No frontend component derives a physical path from a session ID.

## Implementation order inside the single effort

1. Add shared classification/candidate/lookup primitives with behavior-preserving tests.
2. Add validated catalog-only resolution and wire fs-sync direct/internal paths.
3. Add `Locations` events and centralized path-query invalidation.
4. Add recording prepare/cancel and post-stop settlement ordering.
5. Fold ghost reporting and startup normalization into one discovery snapshot.
6. Run all automated and manual verification before merging the complete effort.

Each step should be a reviewable commit, but partial completion is not the intended shipped state.

## Verification

Run after Rust changes:

```sh
cargo test -p vault-read -p vault-write -p fs-sync-core -p tauri-plugin-fs-sync -p listener-core -p tauri-plugin-transcription -p loof-cli
cargo test -p desktop --lib
cargo check
```

Run after desktop TypeScript changes:

```sh
pnpm -F desktop typecheck
```

Run after all edits:

```sh
pnpm exec dprint fmt
```

Manual verification:

1. Open a migrated note with audio and attachments; confirm traces show catalog hits and no repeated full discovery.
2. Title an untitled session containing imported audio and an image; confirm the player, waveform, and image survive the rename.
3. Move and manually rename a session folder while the app is open; confirm catalog rebuild plus `Locations` invalidation refreshes all path-backed UI.
4. Configure before/after hooks, title during start and stop, and confirm hook paths remain valid.
5. Exercise nested legacy ghost/corrupt directories and confirm fs-sync still adopts the existing location instead of creating a root duplicate.
6. Restart with legacy, provisional, healthy, corrupt, duplicate, hidden, and ghost fixtures; confirm one startup discovery and unchanged indexing semantics.

## Definition of done

- Routine desktop audio, attachment, Finder, hook, and session path lookups use validated O(1) catalog hits.
- Cache misses retain the exact current fs-sync corruption/ghost fallback semantics.
- Physical directory changes invalidate every frontend cache that stores an absolute session path.
- Pre-start and post-stop hooks resolve paths under deterministic lifecycle ordering.
- Session-boundary, hidden-directory, readable-candidate, and existing-location rules each have one implementation.
- Startup normalization performs one discovery; normal rebuild performs one discovery and no second ghost walk.
- All legacy/readable/nested/corrupt/duplicate/ghost behaviors remain covered.
- No optional or deferred item remains in this plan.

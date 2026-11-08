# Frontend/backend separation continuation

## Goal and constraints

Fully implement, test and validate [the complete plan](./frontend-backend-separation.md).
The goal is active and incomplete. Status questions do not replace it. Local stays
standalone, offline and login-free by default. Explicit Remote never falls back to
a Local vault. Preserve macOS and Windows 11 x64/ARM64; Linux is headless only.
Exclude bases, CRDTs, mobile, real-time collaboration and general remote offline
editing. Do not merge or release yet.

Use the existing isolated worktrees. All build/cache/temp/log/model/test artifacts
must physically use T7. The original `/Volumes/T7/src/free-meeting-transcriber`
tracked main is untouched (`?? docs/plans/` only). Do not restore old stashes,
create replacement trees/caches, or cancel healthy builds/CI. Source stays frozen
through each dependent build. Use Cargo's environment for Native runtime tests;
direct Mach-O execution previously failed two Codex probes that passed through
Cargo with the same binary. Never exclude those probes to claim a passing suite.

## Authoritative source and publication

- Integration: `/Volumes/T7/src/loofah-backend-separation`, branch
  `refactor/frontend-backend-separation`.
- Current implementation checkpoint: `c6df7ad9a5a494151da982c5bf9367191c3c4247`.
- Latest integrations before it: portable ownership `e1a7b460b`, pinned Store/Core
  ownership `a89e6190d`, checked session delete/restore `f2d9de8b4`.
- Published head remains `f5a30b2ad78c37b81de82e53634649c0e464bf70`, draft
  PR <https://github.com/bart6114/loofah/pull/123>. No merge/release. Do not push
  the next batch over its healthy Windows run.
- At the last live check, all f5 workflows except Windows completed successfully,
  including backend, CLI, full desktop and general CI. Windows run `42480766961`
  remains in progress. Earlier x64 packaging HTTP504 failures are historical;
  inspect the current run before acting. The current unpublished source still
  needs fresh hosted Windows validation on both architectures.

## Current behavior

The default Local app still opens its existing vault without sign-in. The settings
plugin now acquires the canonical vault lease BEFORE config migration. Its
`LocalVaultLease` is shared with the Native Context and Store; ConfigInner and
StartupSnapshot blocking writes retain the original Arc<File> through actual I/O.
Remote startup acquires no Local vault lease and reads device preferences only.
The old CLI is not yet a safe concurrent writer: the owner bridge below remains
required before release, and pre-bridge desktops require a stopped upgrade.

Remote startup still selects the experimental notes workspace. The real main
workspace adapter/editor and bounded main timeline are implemented/tested, but
activation still waits for the remaining actions/settings/processing, recording,
imports/exports and acceptance gates. Unsupported legacy actions cannot load
Local credentials/models or write a Local vault.

The device recording host lives outside the authenticated vault view. Existing
unrecorded notes can capture audio, and **Record a new note** can start a pending
capture before sign-in when the profile has a saved member/vault destination.
This creates raw device audio, not an editable offline note. Native capture does
not depend on a stale connection; only a matching available window/profile client
may attach an uploader. Stop and private recovery inventory work signed out.
Unknown start replies retain both capture and pending-create request UUIDs.
Destination and raw capture identity never retarget after switching accounts or
vaults. The queue displays the original server/vault and retains all device audio.
Explicit raw WAV export and interrupted-prefix finalization/recovery controls are
still required. No automatic device audio deletion is enabled.

The server has authenticated live HTTP/SSE operations backed by the same durable
capture chunks and configured CPU host. `transcription_policy` exposes only the
public model alias/advisory availability without opening a cold vault. Current
source advertises `capture_pending_publication` separately, and configured hosts
advertise `capture_live_transcription`; notes-only hosts omit live. These changes
are not yet in published f5. Pending publication verifies raw bytes, creates one
checked note using producer time, pins its binding and publishes recording once.
Replay and lost ACK never create another note. Existing transcript replacement
and append remain unavailable.

Automatic live/batch-repair/summary orchestration remains incomplete. It belongs
in shared Core, with explicit durable intent, existing jobs, stable IDs and fresh
membership through canonical commits, independent of a frontend callback.

## Fixed worktrees and current assignments

- **A `frontend_distribution`**, `/Volumes/T7/src/loofah-separation-document-editor-ui`:
  checked lifecycle checkpoint `b6ae926170ecd45f459f71711ac8bc08b06cd0bd` is integrated
  as `f2d9de8b4`. Next: thin HTTPS/SDK/VaultClient and shared frontend delete/restore
  and undo, preserving Local path/timestamp behavior through trusted details.
  A owns new adapter leaves/session-store command wrappers; Root owns lib.rs
  registration and actual exports. Proposed capability `session_lifecycle` must
  not be advertised until its full read/delete/restore surface is wired. A has
  explicitly released target-root after its tested checkpoint and is coordinating
  the next source cohort with C. No Native/generated edits without agreement.
- **B `backend_distribution`**, `/Volumes/T7/src/loofah-separation-batch-transcription`:
  portable lease `602d97239` and pin `e9a21efbc4f79836c78148cf152618e9cb8e8525` are
  integrated. Next: secure Local owner bridge/typed Core dispatcher and CLI/MCP
  resolution, retaining all existing command flags/output and mutations. Same-user
  Unix socket on macOS; protected same-SID named pipe on Windows. No TCP/login.
  Resolve ownership before cache/config migration; no independent fallback while
  another owner holds the lock; idle MCP holds no lease/Context. Stream large
  payloads with bounded frames; no new hard total Local note/import limit. Use
  `ConfigState::load_with_vault_lease` for owned transient contexts. Root Native
  bridge hooks can reuse managed `tauri_plugin_settings::LocalVaultLease`.
- **C `capture_distribution`**, `/Volumes/T7/src/loofah-separation-transcription-cpu`:
  all prior pending/live/lifecycle changes are integrated. Rich internal source checkpoint
  `6aeaa8d8c2f1a750a673303b8647de18330eebd9` is clean but NOT yet compiled
  (11 focused tests authored). It implements checked creation for existing CLI metadata flags, preserving old default v1
  receipts and producer-timed pending creation. New private v2 receipt pins initial
  metadata and files-created proof, with honest partial-persistence/recovery and
  no post-create metadata patch. Coordinate its known-path busy check with A.
  Next: server-owned capture processing intent/orchestration in dedicated Core,
  protocol and route leaves. Reuse existing live/batch/summary jobs; no generic
  workflow engine. Do not auto-process historical captures without an explicit
  intent. Pending capture uses batch after binding/publication. Root wires SDK
  upload triggering and Native/front controls after C supplies its contract.
- **Root**: integration, Native ownership/startup/quit, actual generated bindings,
  recording UI/activity, raw local capture recovery/export, main activation and
  remaining acceptance. Next Native issue: legacy Local recording/STT is outside
  Core capture journals; protect shared deletion/CLI with a real session-work
  lease through actual capture/transcript persistence, not sampled React state.
  A identified the seams: recording_meta.rs Started is too late and Stopped is
  not a persistence barrier; Finalizing must remain owned. Acquire before Native
  capture/start_transcription admission and retain through actual terminal plus
  final queued transcript persistence/session_finish_transcript flush. Batch JS
  awaits lastTranscriptWrite after the Native terminal event. Never release from
  sampled Started/Stopped or React activity alone. Shared explicit delete may use
  cap/role/connected only after this guard is installed; automatic cleanup stays
  activity-gated. Do not advertise Local session_lifecycle early.

## Build state

Fixed Native runner: `/Volumes/T7/src/loofah-separation-desktop-bootstrap`, branch
`refactor/backend-desktop-bootstrap`, clean source `3b0ac97f5` (Root Native source
checkpoint `2ba85c1a5` plus A lifecycle). Native cache is
`/Volumes/T7/loofah-search-build/target-tauri`. Do not compile Native from integration.
Portable cache is `/Volumes/T7/loofah-search-build/target-root`; agents coordinate
its reservation directly. Do not assume a stale reservation means a live build.

Root combined verification **passed** on frozen runner source; exec session78210
is terminal. Configured CPU-host unavailable/policy regression passed1 test; the
combined all-target check for desktop/settings/Core/server with server CPU feature
passed in63.85s. Native cache is released; no Root Cargo/test process remains.
The317 selected Rust/TOML/lock source files match integration byte-for-byte. Logs:

- `logs/pending-lifecycle-combined-cpu-policy.log`
- `logs/pending-lifecycle-combined-check.log`
- `logs/pending-lifecycle-combined-results.json`
- `logs/pending-lifecycle-combined-verification.json`

All logs mentioned here are under `/Volumes/T7/loofah-backend-build/`. Actual
exports run from `apps/desktop/src-tauri` with filter `export_types` (module is
`test`, not `tests`; a mistaken `tests::export_types` ran zero tests and is NOT the
export evidence). Oxfmt generated Native TS from repo root. Protocol export remains
generator-owned. Register newly exposed lifecycle types when its adapters land.

## Verified checkpoints and evidence

Root c6 (Native source on B ownership base, before A combined verification):

- Whole frontend **2,513 tests /277 files**, typecheck, all109 locale extraction
  and compilation, full dprint and diff check pass.
- Native Cargo **132 passed, one existing opt-in ignored**; actual export separately
  **1 passed**. Settings plugin16 and Core settings27 pass, including a cancelled
  write retaining the original kernel lease through actual I/O.
- Server capture/live9 and policy1 pass. Real SDK publication covers Existing and
  Pending at50/150/500ms, lost ACK, exact replay, one note, producer time and an
  independent canonical audio hash/byte read. Earlier stale pending-refusal test
  was corrected to the implemented creation behavior.
- Four-package all-target check and protocol export --check pass. Twelve Root-owned
  source files match the fixed runner byte-for-byte. Proof
  `logs/pending-owner-verification.json`, SHA256
  `863769123f791bfc0d2ce9206ea03c548f8f9749be331afb459357ad9f21cc5c`.
  First wrong-filter zero-test export is superseded by the actual export above.

B ownership checkpoint: storage62, Store555 (five existing ignored), Core56 focused,
all-target check and library Clippy pass. Original lease stays pinned through real
blocked filesystem work after waiter/Store drop. Proof
`agents/batch-transcription/logs/owner-pin-verification.json`, SHA256
`a2a93621c824ca9366e1d052216a80402da2f6ef76ef14477f71646fafbf9998`.

A lifecycle checkpoint:55 selected tests (Store15, Core lifecycle18, jobs/access4,
batch16, gate overlap counted once), all-target check and normal library Clippy
pass, zero owned warnings. Strict -D warnings stopped on existing dependencies;
no unrelated cleanup. Proof
`agents/document-editor/logs/session-lifecycle-verification.json`, SHA256
`32a675b9463fb3baa8bf7ceb1e9efda82cbb2a6b8dd2d5f4dea262e1bce82703`.

Earlier actual configured CPU live20s case produced first words1.21s before seal,
verified real drain, canonical audio/transcript and unknown-file preservation.
This is NOT physical device, two-hour, fresh Windows or complete production
acceptance evidence. Main activation, complete import/export/provider settings,
CLI ownership/remote mutations, automatic processing, append, retention,
upgrade/backup validation and remaining plan gates remain required.

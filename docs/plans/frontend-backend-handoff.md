# Frontend/backend separation continuation

## Goal and invariants

Fully implement, test and validate [the complete plan](./frontend-backend-separation.md).
The goal remains active and incomplete. Local is standalone, offline and login-free
by default, including one-person installations. Explicit Remote never falls back
to a Local vault. Preserve macOS and Windows 11 x64/ARM64. Linux is headless only.
No bases, CRDTs, mobile, collaboration or general remote offline editing.
Do not merge or release yet.

Use the existing isolated worktrees. All local build, cache, temporary, model,
test and log artifacts must physically reside on T7. The original tracked main
at `/Volumes/T7/src/free-meeting-transcriber` is untouched. Do not restore old
stashes, replace worktrees/caches, cancel healthy builds, or push over a healthy
Windows run. Freeze each runner's source during its dependent commands.

## Source and publication

- Integration: `/Volumes/T7/src/loofah-backend-separation`, branch
  `refactor/frontend-backend-separation`.
- Published: `ade7bb6e51b45beb92d9296f6eb986657694afe2`, draft
  <https://github.com/bart6114/loofah/pull/123>. Publication and draft status were
  read back. No merge or release.
- Current committed integration: `c7e311b54`; preceding commits are
  `945228d3b` (Windows lifecycle fixture), `1912a62ce` (checked UI deletion/undo),
  and `87b3eeb1d` (owned Core Local CLI dispatcher/startup).
- Uncommitted Root source: Local journal selection/startup integration; narrowly
  lease-pinned before-ready recording recovery; Local session work barriers in
  Core, Native and transcription plugin; actual export path robustness; macOS
  canonical temporary-path test correction; this handoff update. Inspect
  `git status` before editing. None of this batch is published yet.

## Live CI

All previous `f5a30b2ad` hosted workflows completed successfully, including both
Windows installers/installed-app checks. New published `ade7bb6e5` has green
backend, CLI, general/frontend and zizmor workflows. Desktop Rust and Windows
x64 failed new fixtures; ARM64 Windows run `42490673498` remains active at the
last readback. Do not cancel it.

The failures and corrections are concrete:

- macOS Core lifecycle inventory fixture passed `/var` alias to strict storage.
  Root canonicalized its temporary root before journal creation.
- Windows traversal fixture used `PathBuf::join` on a verbatim path, which
  normalized away the traversal before calling production validation. B's
  `fbef4e796` is integrated as `c7e311b54`; raw OsString construction now asserts
  ParentDir is present before testing rejection. Production already rejected it.
- Windows lifecycle external-editor fixture used `std::fs::write` against a
  shared-locked file, truncating it before WriteFile failed. A's `968a64960` is
  integrated as `945228d3b`, using existing atomic replacement. No production
  safety check was weakened. These two fixture fixes still need execution on the
  new combined source and fresh hosted native CI.

## Implemented and verified before this batch

Default Local takes the canonical vault lease before settings migration and
pins the original descriptor in Context/Store/settings through actual I/O.
Remote startup takes no Local lease and uses profile-separated OS credentials.
Remote activation remains the experimental notes workspace; main-shell feature
parity and activation remain gated.

Published retained-audio recovery `f374d7cda` allows signed-out WAV export and
explicit preparation of an interrupted timed prefix for later upload. Original
identity, checksum chain, float sample bits, no-clobber export and raw bytes are
preserved. No automatic source cleanup or general offline editing is enabled.
Pending recordings can start before sign-in with an already-saved member/vault
destination; only the original authenticated destination can upload/publish.

Published shared lifecycle adapters cover checked read/delete/restore through
Core, Store, IPC, HTTPS and Rust SDK; actual bindings/capability are exported.
UI integration `1912a62ce` is now present, but Local shared deletion remains
DEFAULT OFF pending legacy producer/persistence barriers. Trusted old Local UI
behavior remains available; no Remote fallback.

Published basic Local owner foundation includes canonical journal selection,
same-user Unix sockets/macOS and protected same-SID Windows named pipes,
bounded control/data framing with large payload spooling, lock-first resolution,
and no independent fallback while another process owns the vault. CLI/Native
activation and full import/transcription parity are not complete.

Evidence lives under `/Volumes/T7/loofah-backend-build/`:

- `logs/capture-recovery-verification.json`, SHA256
  `56537c3b61ad8d53491352e374e82857738a4daa8ce0567b0fbbd7823614f3d6`:
  104 unique focused Rust cases, actual Native/protocol exports, CPU-feature
  all-target check and Clippy; full frontend 2,518 tests/277 files, typecheck and
  locale checks passed.
- `agents/batch-transcription/logs/owner-transport-verification.json`, SHA256
  `45e423d0b732eb62f8846855bad6a510531cc2b7ee692bb3b741e29c617a789b`:
  storage70, protocol28, owner11 plus actual child owner-death; checks/Clippy.
- `agents/document-editor/logs/session-lifecycle-ui-verification.json`, SHA256
  `2db9ed9ba0e5f55e01c8ee448e98620417c6b35e858ab84e52a7261cd3e2de41`:
  170 unique affected frontend cases and whole desktop typecheck.
- Rich creation C proof SHA256
  `6166b0e5dd4cf3dfb0150f8bbe02f348d315d504ae15b2ff3e1003b63d9eb409`:
  39 focused cases plus checks/Clippy; integrated `6b7b8f9b4`.

## Current Root runner and immediate continuation

Fixed Native runner: `/Volumes/T7/src/loofah-separation-desktop-bootstrap`, branch
`refactor/backend-desktop-bootstrap`. It retains the previous verified recovery
checkpoint via `refactor/backend-native-recovery-verified-01e05f2c1`, then uses
published ade plus B's df283 Core checkpoint (`e89625d90`) and copied Root WIP.
Targets physically resolve to `/Volumes/T7/loofah-search-build/target-{root,tauri}`.
Never build Native from another absolute path.

Native compile `68960` completed successfully in132.63s. It is superseded for
execution by the final Local drain correction and regression now in the runner.
Current Cargo test handle is **59891**. Poll that handle and its logs before any
runner edits or new build. Inputs/hashes: `logs/local-work-source-freeze.json`.
Logs: `logs/local-work-test-*.log`; results written incrementally to
`logs/local-work-test-results.json`. Packages are desktop, Core, local-owner,
server, client, capture-store, vault-write, listener-core and transcription
plugin with server CPU feature. Keep this feature graph stable.

Earlier bootstrap tests exposed a real ordering bug: pending Store refused
recovery operations requiring readiness, globally hiding recordings. Root added
`RecordingPublicationRecovery`, a narrow original-lease-validated recovery view
that can only discover/prepare already-admitted publications while the managed
Store remains pending. New Store tests exercise both durable publication
boundaries, preserve note/audio bytes and reject foreign/unpinned descriptors.
A second fixture had used an unrecognized receipt name and was corrected to the
real recording-publication prefix. Do not weaken discovery to pass that fixture.

Local work barriers retain the shared Core lifecycle admission for an immutable
request UUID until BOTH native producer/resource completion and all originating
frontend persistence are acknowledged. Unknown starts/writes stay protected;
late acknowledgements cannot release another generation. Tracked Local capture
requires actual native stream drain and stopped recorder/listener/source before
proof; batch runtime ownership follows the existing PreparedAudio/native worker
Arc chain. Validate the new actual mocked-capture case and cancellation paths
before enabling Local shared deletion. New Native ack is
`application_finish_local_session_work(session_id,request_id)`; plugin starts gain
`request_id` after params.

Actual export tests now resolve output from CARGO_MANIFEST_DIR, so both Native
and plugin can run via Cargo in the same frozen package/feature cohort. Generated
files remain generator-owned. Copy exports to integration/A and format them,
then A can finish typecheck. Do not hand-edit bindings or execute Mach-O binaries
directly: earlier direct probes differed from Cargo runtime behavior.

## Existing agents and ownership

- A `frontend_distribution`: `/Volumes/T7/src/loofah-separation-document-editor-ui`.
  Lifecycle UI `a29578588` integrated as1912. Current frozen10-file frontend WIP
  forwards stable Local work UUIDs through listener starts and waits all original
  writes before ack, including retired UI owners. 190 affected tests pass;
  actual generated plugin/Native signatures and final whole typecheck/commit are
  pending. Source proof `agents/document-editor/logs/local-session-work-frozen-source.json`,
  SHA256 `6435a91861f8dc6a56dd01cca7519e4c07611fafdcc16b220e2ada790db6a6fb`.
- B `backend_distribution`: `/Volumes/T7/src/loofah-separation-batch-transcription`.
  Core bootstrap/dispatcher df283 integrated87. Current source-frozen basic CLI
  gateway covers reads/MCP, rich create, note/meta/tag CAS, deletion details,
  attachment/PDF selection and maintenance without Doctor migrations or lost
  first-init counts. Waiting for C's root-cache release for bounded tests/checks.
  Then implement checked MP3 publication and recording-hash/transcript-revision
  replacement; old import/transcribe acceptance remains explicitly incomplete.
- C `capture_distribution`: `/Volumes/T7/src/loofah-separation-transcription-cpu`.
  Server-owned explicit capture processing pipeline source is not yet integrated.
  Existing fake/legacy cohort88 passed; configured real20s CPU fixture adapted
  without duplicate inference scenario. Latest approved Empty+saved-notes summary
  changes compiled; C now adds atomic private child-origin markers to prevent
  missing-parent/older-binary queued work from looking like unrelated manual AI.
  No auto-processing historical captures and no paid retry after unknown output.
  C coordinates target-root handoff directly to B.

## Remaining full-plan gates

Automatic pipeline integration plus explicit durable device declaration/SDK/UI;
CLI/Native Local owner activation; checked MP3 imports and transcript replacement;
provider/settings parity and credentials; remaining content/export/device actions;
main workspace activation; retention/verified cleanup; backups/upgrades and
compatibility; production macOS performance with realistic vault/recording/AI;
long recordings and network/restart acceptance; fresh native Windows x64/ARM64.
Prior actual20s CPU first-caption evidence1.21s is not physical device or two-hour
acceptance. Do not claim the full plan complete or ship the preview as parity.

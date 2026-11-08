# Frontend/backend separation continuation

## Goal and constraints

Fully implement, test and validate [the plan](./frontend-backend-separation.md).
Implementation is authorized while the user is away. Status questions do not stop
it. Keep standalone use offline, login-free and the default. Preserve macOS and
Windows 11 x64/ARM64; Linux is headless only. Exclude bases, CRDTs, mobile,
collaboration and general offline remote-vault editing. No merge or release yet.

All worktrees, build/cache/temp/log/model/test artifacts must physically use T7.
Original `/Volumes/T7/src/free-meeting-transcriber` tracked main stays untouched.
Do not restore old stashes or cancel healthy builds/CI.

## Current source

- Integration: `/Volumes/T7/src/loofah-backend-separation`,
  `refactor/frontend-backend-separation`.
- Previous published checkpoint: `09495af6fe2ec34164fd38be177b4849efe5d485`.
  Verify the current published head through origin or PR metadata.
- Draft PR: <https://github.com/bart6114/loofah/pull/123>.
- Integrated source now includes startup `0f44dadda`, active-profile save fix
  `b80001d47`, main attachments `5b27c9855`, captured main host `77ead7fd3` and CPU
  deployment `7cd59564f`.
- Shared batch Core/CPU/HTTP operations are integrated through `013580cf3`;
  model-to-canonical CPU proof is from the actual configured Service on macOS.
- Root's fixed native runner is
  `/Volumes/T7/src/loofah-separation-desktop-bootstrap`,
  `refactor/backend-desktop-bootstrap`. Rebase it onto the integration head only
  while clean and no dependent Rust build is running; it currently has equivalent
  startup save fix `98e0e8ab2` and must skip that already-applied patch.

Earlier implementation and evidence are historical sections in the main plan.
Do not append another large chronological checkpoint there. Update its current
summary and this handoff instead.

## Startup cohort

The SDK stores a bounded, revision-checked `desktop-startup.json` next to saved
connection metadata. Absent means Local without creating profiles or contacting
an identity provider. Unknown/damaged records and removed/replaced profiles fail
closed. Settings resolve this choice before resolving a local vault. Remote
preferences have their own device directory and no legacy local-settings fallback.
Owned settings I/O retains its serialization guard through actual completion.

Native startup omits the local Store/Context/watcher/index and local STT supervisor
in remote mode. The existing device transcription root remains available. A
cached immutable active ProfileLease lives in RemoteState for the app lifetime;
reconnects/offline captures must reuse it instead of opening the profile again.
Each profile uses its own OS credential namespace. Window teardown closes its
network client without releasing or retargeting capture ownership.

Frontend bootstrap keeps the original local tree intact. Explicit remote startup
mounts the existing experimental notes workspace, with saved member/vault checks.
Startup preferences apply next process start. Local-vault failure also exposes
startup options. No general in-process transport switch exists.

Checks under `/Volumes/T7/loofah-backend-build/logs/desktop-bootstrap-*`:

- Native build and all-target desktop/settings/SDK/CLI Rust checks passed.
- 122 desktop tests passed, one existing ignored; actual native binding export
  passed separately. Settings 15 and SDK profile/startup 18 passed.
- Whole desktop frontend: 2,367 tests across 262 files passed; typecheck passed.
- Focused startup/transport checks: 46 passed. New owned UI lint has zero warnings;
  global lint retains 86 baseline warnings, zero errors.
- All 109 locale catalogs extracted and strictly compiled. Global formatting and
  changed-source checks passed; startup locale drift passed after commit.
- Native provenance JSON records own source/executable/.d, hashes and exact tests.
  The first native attempt lacked generated Codex catalog resources; running the
  existing preparation script fixed that. Successful second build took 1m21s;
  all-target check took 2m26s. Do not repeat a full native build for small UI edits.

Startup follow-up: the native command now reuses the immutable active profile
lease when changing the next-start selection back from Local to that profile.
The same shared SDK CAS/atomic-write operation is used; foreign-store/profile
leases are rejected. All 20 SDK profile tests and the actual native command
round trip pass; desktop/CLI all-target checks pass and actual binding export has
no contract drift. See `logs/desktop-startup-leased-save-*` for proof.
Title-only draft previews are memory-backed, not crash/restart durable.

## Latest attachment and deployment integration

The actual main attachment list, upload hook, raw/output resources, image strip
and navigation now use scoped APIs and canonical IDs. Runtime URLs and local paths
do not determine upload success or enter new saved references. Lists are paged;
resource/copy actions are lazy. Local offline upload and Finder/Explorer behavior
remain available through the local adapter. Unknown replies retain the same UUID
and staged source for explicit retry. Root reviewed callbacks/ownership and the
combined startup/attachment/main-host run passes 332 checks across 31 files;
whole desktop typecheck passes. Worker proof covers 142 cases and 50/150/500 ms retirement.

CPU deployment adds `Dockerfile.cpu` (Ubuntu 24.04 x64), a shared pinned Linux native
setup and precise configured-Service runtime CI. Default notes-only image/job and
macOS setup remain unchanged. Local capability checks and 23 workflow helper tests
pass; exact model-to-canonical execution on Linux and full standalone Docker build
remain pending. No local Docker daemon/VM was started.

## Active agents and next dependencies

Use the existing fixed trees; no new worktree per subtask.

- A `frontend_distribution`: `/Volumes/T7/src/loofah-separation-document-editor-ui`.
  Main attachments and captured host `52303dd95` are integrated. The generic host
  takes explicit Scope/AI/listener state and exact-Scope activity; Local preserves
  legacy services, and unknown activity skips cleanup. Next: captured activity and
  unsafe legacy capture/delete/window affordance guards. No Remote main activation
  until bounded timeline browsing and the remaining gates are migrated.
- B `backend_distribution`: `/Volumes/T7/src/loofah-separation-batch-transcription`.
  Portable uploader `15b36b838` and native coordinator source `d18e40922` are ready
  for Root review. Portable checks passed; native fixtures are SOURCE ONLY.
  Next: stable caller start UUID, exact bounded replay, changed declaration refusal
  and existing-spool recovery-only behavior. Root owns command/quit/UI integration
  and automatic empty-session cleanup guards for active/queued captures.
- C `capture_distribution`: `/Volumes/T7/src/loofah-separation-transcription-cpu`.
  Deployment `1f4eedaa6` is integrated. Next: internal Core/CPU live vertical slice,
  protocol leaf, pure processor mapping and actual drain/promotion proof. Reuse
  durable HTTPS chunks; SSE delivery/SDK will follow after its API freezes. No
  public live capability yet; no second model pool/spool or canonical format.
- Root: integration, timeline contract, actual bindings, public recording commands,
  safe quit and subsequent real-main activation, remaining workflows and acceptance.

Both cache windows were released by B/C; Root owns target-tauri for the next
consolidated native capture cohort. Confirm reservations before another Cargo
invocation. Physical shared roots are
`/Volumes/T7/loofah-search-build/{target-root,target-tauri}`. Keep source frozen
through dependent builds. Use Root's fixed runner for native checks rather than
compiling identical integration source from a second absolute worktree path.
Generate native bindings with cwd `apps/desktop/src-tauri`, then oxfmt the resulting
file from the repo root (do not pass paths containing `..`). Keep protocol export
format unchanged.

## Hosted checks and release gates

Pushed `09495af6f`: frontend, CLI, formatting/security and default container passed.
Both Windows architectures completed native checks/tests, installers and installed
app/uninstall checks successfully. Windows speech-model dispatch was skipped.
macOS and headless server suites failed the same attachment-removal fixture:
its held reader outlived shutdown. Root `276483edf` drops the reader after its last
assertion. Linux typed-live CPU fixture hit its 10-second tail collector deadline;
Root `4dc982502` increases only the real CPU drain budget with diagnostics. Both
corrections need the next hosted run. Last entirely green head is `93dada10bb`.

The current startup/attachments/host/CPU-deployment batch has passed its affected
checks. Push it once and update/read back PR head/body; fresh hosted results remain
required. Subsequent capture/live/main activation work continues in parallel.
The previous Windows run is finished, so there is no old-run wait requirement now.
Do not claim completed platform QA from macOS tests or skipped speech-model jobs.

Still open: complete main remote UI, remaining processing/streaming/import/export,
provider secrets and CLI owner/writes; capture/public lifecycle/pending/append,
live STT/diarization and repair; retention/cleanup and production compatibility,
upgrade, long-recording and macOS/Windows performance/UX acceptance. The full plan
remains active, even when a useful intermediate feature is committed or pushed.

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
- Last pushed: `09495af6fe2ec34164fd38be177b4849efe5d485`.
- Draft PR: <https://github.com/bart6114/loofah/pull/123>.
- New Core/CPU/HTTP source is integrated through `013580cf3` (configured capability),
  including `0ec0d4923` (actual CPU host activation), `3ac17c716` (authenticated jobs),
  `27c765c1e` (durable Core jobs), `13a17539f` (result binding/interrupted distinction),
  `9d3a4a258`/`2b49e78bc` (main source/title editing), and the CPU adapter/pure prep.
- Root startup source lives in the fixed
  `/Volumes/T7/src/loofah-separation-desktop-bootstrap`,
  `refactor/backend-desktop-bootstrap`. SDK startup selection is `c34a62db8`;
  the validated native/frontend cohort is committed with this handoff.

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
  changed-source formatting checks passed; verify locale drift after commit.
- Native provenance JSON records own source/executable/.d, hashes and exact tests.
  The first native attempt lacked generated Codex catalog resources; running the
  existing preparation script fixed that. Successful second build took 1m21s;
  all-target check took 2m26s. Do not repeat a full native build for small UI edits.

Known startup follow-up before production: saving back to the active remote
profile after selecting Local for the next launch needs a borrowed-profile-lease
save path. Identical selection saves and choosing Local already work. The current
exclusive SDK profile lock can otherwise refuse that reverse selection until
restart. Keep the active profile immutable; do not solve this by retargeting it.
Title-only draft previews are memory-backed, not crash/restart durable.

## Active agents and next dependencies

Use the existing fixed trees; no new worktree per subtask.

- A `frontend_distribution`: `/Volumes/T7/src/loofah-separation-document-editor-ui`.
  Migrating main attachment presentation/resolution/upload and canonical image
  navigation to existing scoped APIs. Owns optional adapter-only directory open.
  No native/export/locale/bootstrap changes.
- B `backend_distribution`: `/Volumes/T7/src/loofah-separation-batch-transcription`.
  Implementing bounded durable uploader and native RemoteCaptureCoordinator,
  reusing existing capture host/spool. Actual lost ACK/offline/reopen/identity/hash
  checks; no new spool format. Initial publication only existing sessions without
  audio. Pending/append remain explicit recovery states. Root owns public commands,
  bootstrap, subscriptions/DTO generation, UI and safe quit integration.
- C `capture_distribution`: `/Volumes/T7/src/loofah-separation-transcription-cpu`.
  Owns Ubuntu 24.04 CPU Docker variant, shared Linux native setup script, CI and
  deployment README plus capability checks. Default notes-only image stays intact.
  Local Docker daemon is unavailable; actual Linux/runtime and standalone image
  qualification remain distinct gates.
- Root: integration, actual contracts, startup, remaining workflows and acceptance.

Build reservations: B owns target-root for frozen portable uploader/store checks.
C owns target-tauri for warm CPU capability-test relink after Root released it.
Confirm explicit release before another Cargo invocation. Physical shared roots:
`/Volumes/T7/loofah-search-build/{target-root,target-tauri}`. Keep Rust source frozen
through its dependent build/tests. Use Root's fixed bootstrap tree for consolidated
native checks going forward; avoid rebuilding the same integration source from a
second absolute worktree path. Generate bindings only from the actual binary,
with cwd `apps/desktop/src-tauri`, then oxfmt the native generated file. Keep the
protocol exporter format unchanged.

## Hosted checks and release gates

Pushed `09495af6f`: frontend, CLI, formatting/security and default container passed.
Both Windows architectures completed native checks/tests, installers and installed
app/uninstall checks successfully. Windows speech-model dispatch was skipped.
macOS and headless server suites failed the same attachment-removal fixture:
its held reader outlived shutdown. Root `276483edf` drops the reader after its last
assertion. Linux typed-live CPU fixture hit its 10-second tail collector deadline;
Root `4dc982502` increases only the real CPU drain budget with diagnostics. Both
corrections need the next hosted run. Last entirely green head is `93dada10bb`.

After finishing this coherent batch: integrate agent commits, regenerate only
changed contracts, run affected checks, push once and update/read back PR head/body.
The previous Windows run is finished, so there is no old-run wait requirement now.
Do not claim completed platform QA from macOS tests or skipped speech-model jobs.

Still open: complete main remote UI, remaining processing/streaming/import/export,
provider secrets and CLI owner/writes; capture/public lifecycle/pending/append,
live STT/diarization and repair; retention/cleanup and production compatibility,
upgrade, long-recording and macOS/Windows performance/UX acceptance. The full plan
remains active, even when a useful intermediate feature is committed or pushed.

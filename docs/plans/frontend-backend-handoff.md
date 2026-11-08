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
- Published checkpoint: `c32f8ae172345de16fc0a963584bb0178c043084`.
  Later integrated source includes the uploader, captured Local control guards and
  timeline contracts/Store/Core/HTTP/SDK source; these are not yet pushed.
  `0e87fe212` adds actual typed timeline client/query seams and the device recording
  client. Its 81 adapter tests and whole desktop typecheck pass.
- Draft PR: <https://github.com/bart6114/loofah/pull/123>.
- Integrated source now includes startup `0f44dadda`, active-profile save fix
  `b80001d47`, main attachments `5b27c9855`, captured main host `77ead7fd3` and CPU
  deployment `7cd59564f`.
- Shared batch Core/CPU/HTTP operations are integrated through `013580cf3`;
  model-to-canonical CPU proof is from the actual configured Service on macOS.
- Root's fixed native runner is
  `/Volumes/T7/src/loofah-separation-desktop-bootstrap`,
  `refactor/backend-desktop-bootstrap`. Rebase it onto the integration head only
  while clean and no dependent Rust build is running. It already contains the
  startup fix, uploader/coordinator and timeline source. Root checkpoint
  `d16e32a3f` added public device recording commands, scoped status/subscriptions, safe quit,
  Local timeline IPC and actual protocol exports (integrated as `516385f18`). The runner is clean; current Native source/checks are verified.

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
pass. Hosted `linux-cpu-stt` and the default container are green on `c32f8ae17`;
full standalone CPU Docker build qualification remains pending. No local Docker daemon/VM was started.

## Active agents and next dependencies

Use the existing fixed trees; no new worktree per subtask.

- A `frontend_distribution`: `/Volumes/T7/src/loofah-separation-document-editor-ui`.
  Main host, attachments and Local legacy-control admission guards are integrated
  (`6ce2d8096` as `ee7436ba0`). Guards passed 543 tests/32 files and typecheck.
  Scoped undo/delete recovery `83d46778f` is integrated as `873d24043` (82 tests,
  typecheck, lint and format passed). Now consumes actual generated
  timeline contracts for bounded Date/Upcoming/Undated/tag browsing. No Remote main
  activation until remaining gates pass.
- B `backend_distribution`: `/Volumes/T7/src/loofah-separation-batch-transcription`.
  Uploader/coordinator/stable start UUID are integrated. Timeline protocol `6a8874df4`
  and Store/Core/HTTP/SDK `0673b7dab` are integrated SOURCE ONLY, pending B's focused
  backend validation. Native wrappers use `VaultContext::timeline_headers/facets`.
  Next live HTTP/SSE/SDK against the C API handed off directly. Source work can
  proceed while C finishes validation; no public live capability yet.
- C `capture_distribution`: `/Volumes/T7/src/loofah-separation-transcription-cpu`.
  Pure live protocol `359e445ed` is integrated and passed its three own-source tests.
  Internal live Core/CPU vertical remains WIP/unadvertised. Its combined CPU graph
  compiled successfully; fake/legacy cases, exact consumed-source hash proof and a
  configured real CPU caption-before-seal case are next.
- Root: integration, actual bindings, timeline client/query factories, public
  recording controls/quit and device UI, remaining workflows and acceptance.

C owns target-root through its live validation, then explicitly hands it to B.
Root finished the native/export/check cohort; no Cargo currently uses target-tauri. Do not launch
competing Cargo work. Physical roots are
`/Volumes/T7/loofah-search-build/{target-root,target-tauri}`. Keep source frozen
through dependent builds. Root's fixed runner avoids compiling identical source
from another absolute worktree path.
Generate native bindings with cwd `apps/desktop/src-tauri`, then oxfmt the resulting
file from the repo root (do not pass paths containing `..`). Keep protocol export
format unchanged.

## Hosted checks and release gates

Pushed `c32f8ae17`: full frontend/typecheck/locales, CLI, backend (including CPU),
formatting and security passed. macOS failed an external Soniqo dependency download
(SpeechCore v0.0.14 HTTP 504), including one failed-job retry; do not repeatedly
rerun while that upstream asset is unavailable. Windows x64 native checks/tests
passed but packaging failed from an external HTTP 504 (including its diagnostic
retry), not a Rust compilation failure.
Windows ARM64 remains active. Do not cancel it or push another batch over it.
Last wholly green head remains `93dada10bb`.

Root's first new native recording cohort compiled and passed all recording,
coordinator and status tests. Full desktop run: 130 passed, one existing ignored,
two existing Codex executable-version probe tests failed their five-second runtime
budgets on T7 (one focused retry also failed). Do not claim a fully green native
suite. SDK uploader six cases and capture Store 33/three ignored passed. Proof:
`logs/device-recordings-v2-test-provenance.json` and its per-executable logs.
The consolidated native cohort now passes 131 tests (one ignored, the same two
Codex probes excluded), actual binding export, and `cargo check -p desktop
--all-targets`. Proof: `logs/device-recordings-final-native-provenance.json`; own
source depfile and hashes are recorded. Its warm native rebuild took38s, all-target
check82s. Root wrapper await errors and five B SDK test-only compilation errors
were corrected before that final result. Both actual generated contract files
are integrated. A rebases the same fixed tree onto `0e87fe212` before restoring
only its owned timeline controller WIP.

The native recording client has no constructor I/O. UI retirement or a lost
start reply preserves the original request UUID as an unknown outcome and does
not stop device capture. Explicit Stop remains independent of network-client
retirement. Closing subscriptions fences callbacks and cleans late admission.
This adapter is not yet a complete recording UI. Before enabling Remote main
cleanup, invalidate/rescan historical inventory at new-start admission so a
retired in-memory ticket cannot turn an old queued capture into false idle. Keep
unknown inventory protective. Root ran already-built timeline tests by pinned executable while C retained its
cache: protocol1, Store9 and Core2 all passed. Proof:
`logs/timeline-pinned-provenance.json`. SDK/HTTP timeline cases remain pending B.

Still open: complete main remote UI, remaining processing/streaming/import/export,
provider secrets and CLI owner/writes; capture/public lifecycle/pending/append,
live STT/diarization and repair; retention/cleanup and production compatibility,
upgrade, long-recording and macOS/Windows performance/UX acceptance. The full plan
remains active, even when a useful intermediate feature is committed or pushed.

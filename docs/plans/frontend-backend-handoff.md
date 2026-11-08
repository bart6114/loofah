# Frontend/backend separation continuation

## Current state

The user resumed **all work**. Goal: fully implement, test and validate
`docs/plans/frontend-backend-separation.md`. This is a continuation checkpoint,
not a pause or a completion claim.

- Root worktree: `/Volumes/T7/src/loofah-backend-separation`.
- Branch: `refactor/frontend-backend-separation`.
- Source: `0818a893e7af829b3a70425b359e7aa8ed6c9597`.
- Compiled locale checkpoint: `3306a0988a9083766328c5cc8f847aa18585d48a`.
- Last push: `aade2e309e08c93249c6d440e64b3e9485457b86`.
- Draft PR: <https://github.com/bart6114/loofah/pull/123>; no merge or release.
- Original `/Volumes/T7/src/free-meeting-transcriber`, `main`: tracked files
  remain clean and untouched; pre-existing untracked docs are preserved.

Default local use remains standalone, offline and **login-free**, including solo
use. Preserve macOS and Windows 11 x64/ARM64. Linux is headless only. No bases,
CRDTs, general remote-vault offline editing, mobile or collaborative editing.
Every local artifact/cache/temp/log/model must resolve physically to T7.

Read live Git status, HEAD, agent/process state and CI before continuing. Source
changes below are committed; Root's Cargo/native/frontend pipelines have finished.
C owns the exclusive shared Cargo target-root build window. B is writing Core
publication without Cargo. A is starting slim main-app reads. Do not cancel
healthy work or run another shared-cache build without coordination.

## Latest integrated work and evidence

Earlier shared operations, authenticated HTTPS/SSE, scoped clients/drafts, opt-in
notes workspace, checked editing/tasks/metadata/tags, media/attachments, processing
settings/jobs, CLI/MCP remote reads and internal native capture are described in
the main plan. None establishes full main-shell or remote capture completion.

New Root checkpoints after the last pushed batch:

| Root commit | Change                                                          |
| ----------- | --------------------------------------------------------------- |
| `ca7f9af3e` | Verified absent-only WAV publication primitive in `vault-write` |
| `b700e51a8` | Preserve/default and optional generated-title job contracts     |
| `338c1f1e7` | Durable body confirmation and optional Core title stage         |
| `7a3da095d` | Plain main New Note through captured client/tabs scope          |
| `b7af0fe3c` | Lossless raw title metadata patch and actual-byte preflight     |
| `67e4e06f3` | Capture-fixture bounded lease reopen, test only                 |
| `9fa1fa50f` | Verified streaming WAV source, four new tests                   |
| `77f4b5b43` | Scoped title opt-in, exact-policy recovery and result UI        |
| `0818a893e` | SDK/capability gates, real network tests, actual exports/docs   |
| `3306a0988` | All 109 locale catalogs extracted and strictly compiled         |

Title requests omit Preserve exactly as before. Opt-in requires `generated_titles`
before credentials/dispatch. The worker confirms the body, then uses its pinned
model/settings for the optional title. Body success survives title failure,
cancellation, conflict or uncertainty. Canonical title application preserves the
existing heading policy; retained generated text remains the original output.
Newer device title drafts survive. Private opt-in jobs use schema 2 and bounded
title sidecars; canonical title transactions use existing schema 4. Do not reset
unreadable records or repeat paid work on restart/downgrade. Review/drain with the
current version or restore matching binary plus complete storage backup.

Important validation:

- Title metadata worker: 101 focused cases/nine new regressions, actual crash
  fixture, default all-target check, source/freshness/list/hash proofs:
  `/Volumes/T7/loofah-backend-build/agents/title-metadata/logs/`.
- Title stage: worker full Core 277 passed/two helpers ignored before equivalent
  lint simplifications; final jobs 128 passed/two helpers ignored, eight actual
  crash subprocesses, checks/Clippy. Final proof files:
  `/Volumes/T7/loofah-backend-build/agents/title-stage/logs/core-stage-test-provenance-final.json`,
  `core-stage-sources-final.sha256`, `core-jobs-tests-final.log`.
- Original publication primitive: worker 479 passed/five helpers ignored,
  26 publication cases/six actual crash subprocesses:
  `/Volumes/T7/loofah-backend-build/agents/capture/recording-publication-verification.json`.
- Root WAV: four passed, exact samples/channel order, corrupted second pass,
  retained context on cancellation, two-hour header geometry (not two-hour capture
  acceptance). SDK missing-title-capability test passed before credential access.
  Proof: `/Volumes/T7/loofah-backend-build/logs/capture-wav-sdk-provenance.json`.
- Root protocol: four job-contract tests, including actual optional TypeScript
  export and exact old request bytes. Network title replay at 50/150/500 ms RTT
  loses the first reply and repeats neither model call. The 150 ms case proves
  invalid title output retains successful unchanged body. Eight existing API
  tests passed in the first run; a new test incorrectly expected no canonical
  heading stamp, was corrected, then passed. Proof:
  `/Volumes/T7/loofah-backend-build/logs/generated-title-api-provenance.json`.
- Native 28 capture cases/default parallel, both targets/policies receipt
  round-trip without login/vault session, actual export, normal desktop check.
  Logs `capture-reopen-native-tests.log`, `title-policy-native-receipt-tests.log`,
  `generated-title-final-native-export.log`, `generated-title-native-check.log`.
- Root Core/client/server all-target check passed. Unwired WAV/internal capture
  methods still emit unsuppressed warnings; do not suppress them for clean logs.
- Frontend worker 117 unique focused tests; Root integrated 165 cases/six files,
  actual-binding typecheck, owned format, strict catalogs and committed drift
  check pass. Logs `generated-title-integrated-{ui-tests,typecheck}.log` and
  `generated-title-i18n-{extract,compile,drift-check}.log` under the same T7 logs.

Actual generated files:

- `apps/desktop/src/types/tauri.gen.ts`:
  `37ac8b22422c84fc951284a49c7e4061277d004800614dcd31d7e657aebd1cdd`.
- `apps/desktop/src/shared/application/protocol.gen.ts`:
  `4c9d0800a69ffbdc1d74a93fe6647b07cfe825e1151861491a7965b975ad7e4a`.

Specta's pinned macro parses custom serde skip after Specta attributes, resetting
optional=false. `SummaryJobRequest` deliberately puts serde `default` last; the
export golden test guards that behavior. Never hand-edit generated contracts.

## Active worker ownership

Reuse the three authorized agents. Do not spawn a new team.

**A — `/root/frontend_distribution`**: slim actual-main summary reads, fresh base
`3306a0988`. Own only `session/queries.ts::useSessionSummary`, linked-row
`editor-bridge/session-view.tsx` and focused tests. Use existing bounded
`VaultScope.sessionSummariesOptions`, avoid downloading/parsing referenced note
bodies for dates. Test dedupe, immutable host/cache identity, replacement/late
results, terminal access loss and offline login-free local behavior. No profile
activation, legacy AI/capture, new backend API or full repeated frontend suite.
Prior clean feature commit `1ac2238c` is already integrated as `77f4b5b43`; two
Root-generated validation inputs in that old worker tree are intentionally dirty
and excluded.

**B — `/root/backend_distribution`**: `/Volumes/T7/src/loofah-capture-publication`,
`refactor/backend-capture-publication`, base `9fa1fa50f`. Core publication code,
Context gates, runtime recovery and recording read/delete gates; no Cargo while
C owns target-root. Approved API `publish_capture_authorized(member, capture_id,
expected_seal, repeatable_authorize) -> CaptureIngestStatus`. ExistingSession with
expected audio absent only; reject PendingSession/append for this checkpoint.
Derive scope, original UUID, encoder-domain source binding, layout and timing
from authenticated sealed storage. Verify full PCM, stream deterministic WAV,
join actual producer/prepare I/O on all outcomes, recheck, then reacquire fresh
authorization through commit and spool observation. Keep raw sources. No endpoint
or capability before integration. Successful response alone cannot authorize
device cleanup; require independent canonical read verification.

Install per-session/per-operation active+unresolved gates before preparation.
Retain them after ambiguous commit; one ticket cannot clear another. Gate media
before and after awaited recording opens and delete preparation/recheck. Unknown
unscoped inventory issues deny recording vault-wide while notes remain usable.
C supplies opaque recovery plans. Bootstrap trusted service-owner recovery needs
no JWT or model work, holds the actual vault lease through new recovery I/O, and
runs before context exposure. Existing subsequent document/startup cancellation
ownership gaps are not claimed solved. No STT endpoint exists yet; future STT
must use the guarded source.

**C — `/root/capture_distribution`**:
`/Volumes/T7/src/loofah-separation-publication-recovery`,
`refactor/backend-publication-recovery`, base `b7af0fe3c`. Own recording publication
module/tests only. Approved bounded complete pending/review inventory, strict
receipt/path/vault validation and opaque prepared recovery handles; Committed
receipts omitted and never resurrected after later deletion. Overflow/incomplete
scan fails closed, no truncated success. Return prepared plan so Core can hash,
reauthorize and commit separately. Preserve uncertainty/stages, never infer or
remove orphan sources/files. Also bound inherited ended_at before intent and
check known transcript.json absence under the existing transcript lease for
initial publication; committed replay permits later legitimate transcription.
No global SessionStore field or canonical schema change. C currently owns the
exclusive target-root validation window; obtain explicit release before B/Root
Cargo work. Verify fresh=false, own source, exact new test inventory and SHA.

## Next integration and completion gates

1. Review C storage recovery, integrate clean commit; review B publisher/gates,
   integrate only after focused ownership/auth/crash/restart tests and default
   check. Root owns subsequent HTTPS/SDK thin adapters and real latency/lost-reply
   end-to-end validation; advertise capability only with a safe complete path.
2. Integrate A slim queries after focused verification. Main-shell/profile and
   remote-only startup remain broader work. Legacy global store commands,
   mutations, tasks/people, timeline/selection, windows, AI and capture must be
   scoped before activating remote in the actual main shell.
3. Continue trusted native source, durable uploader, lifecycle/public controls,
   pending-session creation, append/repair, live/batch transcription and Linux
   model management. Neither private staging nor WAV storage is remote recording.
4. Complete remaining mutations/imports/exports, CLI owner bridge/mutations,
   credentials, deployment restore/upgrade/downgrade, realistic network outages,
   real two-hour recording, production performance and macOS/Windows acceptance.
5. Keep all seven plan phases open until full acceptance. No merge/release based
   on a helper, opt-in workspace, synthetic fixture or previous SHA's CI.

## Hosted CI and build rules

Last fully green head: `93dada10bb592c824ae59eeff72187b5079a9558`, all six hosted
workflows including Windows x64/ARM64 installers. Speech-model dispatch was
skipped, so this is not hardware/model recording acceptance.

Last pushed `aade2e309`: backend/CLI/general/security/frontend pass; macOS and
Windows x64 failed the same capture-fixture lease-release race. Test-only fix
`67e4e06f3` is integrated and 28 native capture tests pass. ARM64 is still active
at this save. Wait for completion before pushing the validated corrective batch;
fix actual failures, do not wait forever for an unchanged failed SHA to be green.
Current run `42436692171`; x64 job `128596929979`, ARM job `128596930244`.
macOS failure run `42436692175`, job `128596929946`.

Use exclusive Cargo validation windows; Cargo command locks alone once selected
a stale wrong-worktree binary with zero new tests. Reject zero-match runs and
pin compiler source/freshness/test list/hash. Sources stay frozen through builds
and actual crash children. Root centrally owns native builds and binding export.
Use installed Cargo, `--locked --offline`, shared targets physically at
`/Volumes/T7/loofah-search-build/{target-root,target-tauri}`. Fresh worktrees run
`scripts/setup-shared-target.sh` and verify physical destinations first.

Set these paths for tools:

```sh
export PATH=/Users/bart/.cargo/bin:$PATH
export TMPDIR=/Volumes/T7/loofah-backend-build/tmp
export NPM_CONFIG_CACHE=/Volumes/T7/loofah-backend-build/npm-cache
export DPRINT_CACHE_DIR=/Volumes/T7/loofah-backend-build/dprint-cache
export CLANG_MODULE_CACHE_PATH=/Volumes/T7/loofah-backend-build/clang-module-cache
export SWIFTPM_MODULECACHE_OVERRIDE=/Volumes/T7/loofah-backend-build/swift-module-cache
```

Use targeted checks, no redundant full suites. Native export runs from
`apps/desktop/src-tauri`, then oxfmt the generated file from Root. Protocol exporter
writes its exact output; do not format it. Catalog drift check needs the generated
catalogs committed first. Markdown uses T7 tmp/markdown-dprint.json.

Preserve pause archive `/Volumes/T7/loofah-backend-build/pause-2026-11-07T093537Z/`
and existing stashes. Root stash `d13afbb1cb44a7939c3a5755458f97652e3630b1` was
restored/hash-verified and incorporated into `0868995b4`; do not reapply it.
Other old stashes are preserved, not instructions to overwrite current work.

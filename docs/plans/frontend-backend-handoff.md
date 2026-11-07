# Frontend/backend separation continuation

## Active resumption checkpoint

The user resumed **all work**. The implementation goal is active; this handoff
is not a pause. Continue in `/Volumes/T7/src/loofah-backend-separation`, branch
`refactor/frontend-backend-separation`. Default local use remains standalone,
offline and login-free. Preserve macOS and Windows; all local artifacts must
resolve physically to T7. The original checkout's tracked files remain untouched.

Last fully hosted-validated head: `a8e873130e628310f3853f6a46def3476df0e741`.
Current validated source checkpoint: `9e26192ddd43f59cf3718213a0fd13cda3abd2b5`.
Read git remote/HEAD and live checks for publication status; later documentation
and locale commits do not change this source checkpoint.
Draft PR: <https://github.com/bart6114/loofah/pull/123>. General, CLI, backend,
macOS desktop, security and both native Windows architectures passed on that
head. Windows speech-model acceptance was skipped. Linux CPU inference passed
30 default, three real kernel and two real batch/live cases with the pinned
model verified and actual `GGML_NATIVE:BOOL=OFF`. This proves the portable kernel,
not an authenticated server transcription endpoint. Evidence: backend run
`42416568822`, CPU job `128535832097`, Windows run `42416568804`.

The integration branch additionally contains:

- Atomic empty-session cleanup across storage/core, IPC, HTTP, SDK and client
  (`4ad455d95`). Canonical inspection and trash move share one continuous guard;
  durable historical receipts cannot delete restored sessions. Native IPC,
  permissions and real lost-reply tests at 50/150/500 ms passed.
- Captured editor flush/recovery guards and atomic close dispatch, with at most
  eight deduplicated optional cleanup candidates (`890b26155`, `27dd1182f`,
  `ae3303c32`). Unknown capture state refuses cleanup. Remote main-host exposure
  still waits for trusted capture/spool inventory.
- Shared capture wire types/hash rules (`30adcf94c`). Device storage v1 and old
  Rust import paths remain compatible. Golden vectors and all 18 spool tests pass.
- Generated template body/task/tag storage and owned core worker (`4fe1ed691`,
  `0771f1d8f`). Raw metadata survives; missing and legacy template targets are
  refused before model calls. Recovery schema 3 adds a distinct generated-template
  kind; older binaries require draining/recovery before downgrade.
- Native provider stop/drain and resource admission barriers (`3960359ec`,
  `f10ad7ca4`). The capture worker passed 112 tests; integrated native capture
  fixtures now pass all six with an explicit fake provider drain receipt. Native
  app check passed. This is still an internal API, not available remote capture.
- Frontend generation/flush package (`57f6769d2`, worker `13e40d59f`) and public
  template capability/admission/receipt checkpoint (`5b597f94e`). Integrated
  suites passed 2,030 tests; typecheck/strict locales passed. Three template HTTP
  cases, eight existing processing API cases, 12 device receipt tests and native
  login-free receipt commands for both targets passed. Shared/client/CLI/native
  checks and library Clippy passed. One subprocess helper is ignored directly and
  exercised by its parent. Do not expose template controls on older servers.
- Shared immutable frontend host (`0479590ff`) and per-tab-host timeline selection
  (`4a2c78585`). Their workers passed full suites of 2,038 and 2,051 tests; Root
  combined provider/workspace/client 81 and selection 63 focused tests passed,
  along with typechecking. Callback changes preserve scope/selection; old client
  or selection handlers cannot affect a replacement host.

The later integration checkpoint also contains:

- Atomic create-only generation admission (`1e4db1b1d`, `90a89876b`). Unknown
  receipts require explicit Resume, including after a stale cross-window list.
  Fourteen Rust receipt cases, native IPC for both targets, 100 combined frontend
  cases and typechecking passed. The frontend worker passed 2,059 tests. Duplicate
  known JSON fields are rejected without resetting receipts.
- Managed native capture (`0b2f61f3c`, `26f1ba11b`, `88d509ed5`, `2e2b58ed1`).
  First-audio wall-clock observation, stop/resource and supervisor proofs are
  distinct from UI events and durable finalization. Owned starts/drains survive
  caller drop and deadlines; retained states/history/waiters are bounded. Terminal
  tickets cannot regress. Twenty integrated native fixtures pass under default
  parallel test scheduling. The production library check passes after promoting
  the existing futures-util dependency from dev-only. Unwired internal APIs now
  produce 29 unsuppressed native library warnings. Public commands, app quit hooks,
  durable timing and uploads remain unfinished.
- Generated-title storage/core (`e4f318ae3`, `54c574723`). Root's three core and
  19 storage cases passed, including actual abrupt-exit recovery. Schema 4 has a
  32 MiB serialized cap; older large official schemas retain their behavior.
  Strict original-byte decoding rejects duplicates. No automatic/public title
  stage is enabled; drain schema-4 intents before downgrade.
- Neutral capture storage (`9e26192dd`, worker `b9f844f0a`). Four moved files are
  byte-identical; old client imports still work. The new library has no SDK,
  reqwest, OIDC, Tauri or device dependencies. Its 121 store/SDK/protocol cases,
  Clippy, checks and protocol drift passed. Root native/shared/server/CLI checks
  and the headless graph check passed after integration. Device schema 1 and hash
  domains remain unchanged. Backend/Windows CI include the new crate's tests.

Three authorized workers remain in isolated worktrees:

- Frontend: `/Volumes/T7/src/loofah-separation-tags`,
  `refactor/backend-tag-storage`, based on the exact source checkpoint above.
  Implementing storage-only bounded registered/in-use tag suggestions and checked
  accept/dismiss operations after reviewing the real UI semantics. No public
  contracts, adapters or UI migration yet. Preserve the prior clean UI branch
  and stashes; its incoming native blobs are not a merge/rebase instruction.
- Backend: `/Volumes/T7/src/loofah-separation-processing`,
  `refactor/backend-summary-tags`. Building the portable title helper and optional
  model sampling parity, with the supplied model retained through cancellation.
  No public policy, worker stage or journal change. Exact JavaScript trim parity
  is the last narrow refinement after healthy checks; Root must validate its tiny
  native ChatGPT fixture/guard centrally after integration.
- Capture: `/Volumes/T7/src/loofah-separation-capture-timing`,
  `refactor/backend-capture-timing`, based on exact source checkpoint above.
  Storage-only schema-2 first-admitted audio observation is in progress. Do not
  claim it is integrated. No identity/hash changes, native manager wiring or
  endpoints are authorized in that slice. Clean prior trees are preserved at
  `/Volumes/T7/src/loofah-separation-capture` (`c4d1f11f3`) and
  `/Volumes/T7/src/loofah-separation-capture-store` (`b9f844f0a`).

Avoid merging the workers' divergent cherry-picked histories. A full merge on
an old capture branch produced 12 conflicts and was safely aborted to its clean
checkpoint. New dependencies use a fresh T7 worktree from an exact Root commit.
The frontend's one incoming conflict was resolved only by copying the exact four
Root contract blobs, then restoring/hash-verifying its two saved source files.
No source was lost. Its named stash `cc0165379ce1f1d6d20f56ff1ebb0fe4995f74eb`
and T7 recovery manifests remain preserved.

Root's current named template backup stash is
`757c1aa385a2398bd8643eede0f43b8e87dca9f5`
(`root-template-transport-before-provider-drain-join`); its seven restored hashes
matched `/Volumes/T7/loofah-backend-build/template-transport-before-drain-join.json`.
Newer changes supersede it; never reapply over the current working tree. Cleanup
WIP stashes are historical because those adapters are already committed.
Historical standby archives below remain intact.

Keep relevant Rust sources frozen during dependent checks. Native Cargo commands
run from `apps/desktop/src-tauri` for the warm target; all targets/cache/temp/log
paths must remain physically on T7. Let healthy tests/builds finish, including
slow macOS binary startup. Batch pushes only after the previous healthy Windows
run completes. Do not merge or release until the full plan and acceptance gates
pass. Main remote-only startup, authenticated capture/processing, title job policy,
remaining content/CLI paths, backup/upgrade validation and real platform/device
acceptance remain substantial unfinished work.

## Historical standby snapshot

The user explicitly resumed all work on 7 November 2026. The pause instructions
below describe the historical standby checkpoint; they no longer pause the active
goal. Read the current implementation-plan audit and git status for subsequent
work. The original WIP backups remain preserved.

Saved 7 November 2026, about 10:37 Europe/Zurich, at the user's explicit request
before workstation standby. **Work is paused. Do not resume implementation or
start agents/builds until the user resumes it.** The full implementation goal is
unfinished; this is a continuation checkpoint.

## Scope and durable instructions

- Implement [the full separation plan](./frontend-backend-separation.md), using
  [the distribution plan](./frontend-backend-distribution.md).
- Default desktop use stays standalone, offline and login-free, including a
  one-person installation. Remote profiles authenticate explicitly.
- Preserve macOS and Windows 11 x64/ARM64. Linux support here is headless server
  support, not Linux desktop support.
- Exclude bases, CRDTs, general offline remote-vault synchronization and real-time
  collaborative editing. Queued device recording uploads are separate.
- Use isolated worktrees. All build/test/temp/cache/log/model artifacts must
  resolve physically to `/Volumes/T7`.
- Keep relevant Rust sources frozen during dependent builds/checks. Do not
  cancel/restart healthy checks merely because observation times out.
- The user authorized the three implementation agents. They are stopped; do not
  assume their conversations/processes survive standby.
- No merge or release until the full plan and acceptance gates pass. Do not fix
  unrelated warnings or reset/clean worktrees containing preserved changes.

## Integration checkout and review

Active integration cwd: `/Volumes/T7/src/loofah-backend-separation`.
Branch: `refactor/frontend-backend-separation`.

The validated shared-header API was committed while saving this checkpoint:
`bd8e1e52d138fa503fa808942fe60cb126b4b41b`
(`refactor: add bounded shared session header queries`). The checkout was clean
after that commit; this handoff is saved separately afterward.

Last pushed head: `4942778ce840bd6fa5c6bc5ffa236b5dbf8d0595`.
Draft PR: <https://github.com/bart6114/loofah/pull/123>. Its body still describes
head `4942778ce`; neither the new API commit nor worker checkpoints were pushed
or merged during pause preparation.

Original checkout `/Volumes/T7/src/free-meeting-transcriber` remains on `main`
at `a6a8f001d`. Continue in the integration worktree, not the original checkout.

Last observed CI on pushed head `4942778ce`: general, CLI, backend, macOS desktop
and zizmor passed; Windows remained in progress. Run IDs: Windows `42356190212`,
CLI `42356191037`, backend `42356190227`, desktop `42356190219`, general
`42356190381`, zizmor `42356190213`. Native Windows x64 tests had passed and its
installer build was running; ARM64 was still in its Rust check. Speech-model
validation was skipped. Recheck live status after resume. Let healthy previous-head
Windows CI finish before the next batch push; do not supersede it blindly.

## Verified recovery copies

`/Volumes/T7/loofah-backend-build/pause-2026-11-07T093537Z/` contains:

- `manifest.json`: branch/HEAD/file SHA-256 inventory for all four worktrees.
- `ui.patch` and `ui-working-files.tar.gz`: all seven dirty frontend files,
  including untracked files.
- `processing.patch` and `processing-working-files.tar.gz`: all three dirty
  Whisper-kernel files, including the new private test file.
- Clean integration/capture HEAD records and empty patches.

The working files remain in their worktrees as well. Archives are recovery
copies, not instructions to extract over newer work. Read the manifest and
compare hashes before restoring. Committed worker changes remain on their named
branches in the shared repository.

## Root's new API checkpoint

`bd8e1e52d` adds one shared session-header query across storage/core/IPC/HTTP/Rust
SDK/TypeScript client/scope. `SessionSummary` contains only `id`, `title` and
`created_at`. Queries are `list`, `title` and `ids`.

- List/title limits: 1–200. Title queries: trimmed Unicode lowercase substring,
  at most 4096 UTF-8 bytes, no control characters. Top-K heap selection scans the
  full index without cloning full note/metadata collections.
- Lists use existing backend `created_at` string ordering with ID tie-breaks;
  `titled_only` filters before limiting.
- ID queries: at most 100 inputs, requested-order deduplication, omit missing or
  deleted sessions, recovered canonical metadata reads with safe paths.
- Raw projected text budget: 1 MiB; encoded JSON budget: 8 MiB.
- HTTP: reader-accessible POST
  `/api/v1/vaults/{vault}/session-summaries/query`, optional capability
  `session_summaries`. IPC remains login-free.
- SDK validates IDs, response bounds, uniqueness, filters and order.
- `VaultClient.sessionSummaries(input, options)` and
  `VaultScope.sessionSummariesOptions(input)` share the operation. Key:
  `["vault", scope, "session_summaries", input]`. Header/session changes,
  confirmed note saves, reconnect and resync invalidate matching scoped caches.

Primary new files: `crates/{loofah-protocol,vault-write,loofah-core}/src/session_summaries.rs`,
`apps/server/src/routes/session_summaries.rs`; registration, requests.rs, native
application commands, generated contracts and frontend client/scope changed in
the same commit. UI consumers are still on the frontend worker below.

Final validation completed successfully immediately before pausing:

- Eight focused Rust protocol/storage/SDK/HTTP tests; one native login-free IPC
  test; 55 focused frontend adapter/scope tests.
- Real TCP SDK fixture at 50/150/500 ms RTT, including canonical title/ID reads
  and lost-submission replay without repeating generation.
- All 1,887 frontend tests across 235 files with two workers. First unrestricted
  run passed 1,886 and hit the unchanged AI window-sync five-second timeout once;
  isolated rerun (7 tests) and bounded full rerun passed.
- Desktop typecheck; shared Rust/core/server/client/protocol/write/CLI check;
  native desktop check; formatting/diff checks; generated protocol readback.
- Scoped Clippy passed with 23 existing vault-write warnings; frontend lint has
  88 existing warnings and zero errors. No new query-module lint warnings.
- Native bindings exported and formatted. Do not manually format protocol.gen.ts.

Logs: `/Volumes/T7/loofah-backend-build/logs/session-summaries-*`. A sample in
`session-summaries-server-loading.sample.txt` found a slow launch at `dyld` before
test execution; healthy processes were left running. These are debug/fixture
checks, not production/native recording or final platform acceptance.

Targeted review item for the next Rust increment: test direct-core caller
cancellation during canonical ID recovery and ensure the `VaultContext`/vault
lease stays owned until actual I/O finishes. The indexed path captures the Arc in
spawn_blocking; the ID path currently borrows it while recovered storage owns its
inner task. HTTP content_read already owns the context through its operation.
No new fix or regression test for this review item has been implemented.

## Frontend worker — preserved WIP

Cwd `/Volumes/T7/src/loofah-separation-ui`, branch
`refactor/backend-scoped-search`, HEAD
`041b00d3204b0b61d658fb8f00e06ce4c8a9cda2`. Its first search slice is already
integrated in root as `fe6fa3380`.

Seven dirty F1b files:

```text
apps/desktop/src/editor-bridge/mention-config.ts
apps/desktop/src/editor-bridge/mention-config.test.tsx                 (new)
apps/desktop/src/session/queries.ts
apps/desktop/src/session/summary-queries.ts                           (new)
apps/desktop/src/session/summary-queries.test.tsx                      (new)
apps/desktop/src/shared/open-note-dialog.tsx
apps/desktop/src/shared/open-note-dialog.test.tsx
```

Implemented consumers: Cmd-K list55 and recent IDs; title50 plus full-text20 in
parallel; canonical bounded ID confirmation before full-text rows are displayed;
five oldest titled mentions; no full header/page preload; ghost/deleted-hit,
1000-note distant-hit and scope/late-response fixtures. Controlled-contract tests
passed 63. Real VaultScope regression tests are prepared but final integration,
typecheck and validation against root's actual contract remain pending.

Read-only interface review matched root's names, shapes, keys and invalidation.
Its identified 512-vs-4096-byte query mismatch was fixed in root before commit.
Logs: `/Volumes/T7/loofah-backend-build/agents/ui/logs/`.

After resume, preserve these seven files before moving onto root's clean
checkpoint. Directly cherry-picking bd8e1e52d into this older branch may conflict
in docs because intervening root commits are absent. Use a clean rebased/new
worktree checkpoint with the verified WIP restored, then validate and commit.
Do not overwrite the original WIP or integrate an unvalidated WIP commit.

Next dependency: device-specific scoped recents. Proposed native API:
`device_recent_sessions(scope)` and atomic
`device_remember_session(scope,id)` (prepend/dedupe/max10). Store under private
device app-local-data, not vault store2/config. One atomic schema-1 file should
bind legacy bare IDs only to the exact managed local VaultContext scope, using a
one-time legacy-bound marker; never import into remote/another vault or open a
local vault during remote-only startup. Scope UI/tab restoration and ignore late
responses. Root owns native/shared registration; this is not implemented yet.

## Processing worker — three clean commits plus kernel WIP

Cwd `/Volumes/T7/src/loofah-separation-processing`, branch
`refactor/backend-summary-tags`, HEAD
`60a7860b7f1995c2da51ae3002cfd2b6593f38c1`.

Review and cherry-pick these in order into a clean integration checkout:

1. `baf4db5978bbd938c714ee17661e12796f04d2d7` — generated-summary storage/recovery.
2. `136140bc8a36746c6964a91dcfd0540d5b55f129` — shared job application.
3. `60a7860b7f1995c2da51ae3002cfd2b6593f38c1` — actual legacy parent-directory sync.

Root reviewed these and requested the third corrective commit. Schema-3 intents
recover summary body/tasks/guarded tag projection/strict fresh registry additions;
schemas 1/2 stay readable. Unknown and unrelated newer metadata/registry fields
survive. Actual legacy enhanced-file parent, session parent and registry parent
sync before intent removal. Public additions:
`SessionStore::save_generated_summary_with_suggestions_if_revision(...)` and
`VaultContext::save_generated_summary(...)`.

Validation: final full vault-write 383 passed/2 ignored; prior full core 228
passed/1 ignored; final focused 15 storage +21 worker +8 HTTP tests passed;
check/Clippy/format passed with existing warnings. Full recording/STT reservation
serialization remains open: current active-recording checks at apply/recovery do
not close the entire race. Update central docs for schema-3 backup/rollback
compatibility after integration. Logs: `agents/processing/logs/summary-tags-*`.

Uncommitted Whisper-kernel files remain:

```text
crates/transcribe-whisper-local/src/service/batch.rs
crates/transcribe-whisper-local/src/service/recorded.rs
crates/transcribe-whisper-local/src/service/recorded/tests.rs          (new)
```

Approved synchronous public kernel: transcribe_batch_file with borrowed loaded
model, model/audio paths, ListenParams, Arc<AtomicBool> cancellation and a
cancellation-aware synchronous event callback; returns existing batch Response.
JSON, SSE and legacy recorded processing delegate to it. Model/admission/audio
ownership must last through actual blocking work. SSE queue is bounded to 16
events, waiting cooperatively with 2 ms cancel/closed checks; required Segment
events stay ordered and terminal response follows queued events. No dependency,
manifest, protocol, platform-catalog or availability changes.

First Cargo invocation compiled production but failed new private-test compile
because hound was only transitive. The fixture now uses standard-library WAV
writing; corrected private tests have **not** been compiled/run. Formatting and
diff checks pass. After resume: recompile, run default + three new ignored CPU
tests, check and scoped Clippy, review and commit.

An already-built integration binary independently passed
`real_batch_sse_has_progress_words_and_one_result` and
`real_websocket_finalizes_and_recovers_from_cancellation`. Logs confirm CPU
(`gpu_requested=false`, `metal=false`, BLAS) on macOS. This does not validate the
corrected private tests or Linux. Binary SHA:
`362379b99def8e1f82b4b61621b234c9453d0e7b5741171ed58e7e2f1ef4d5a6`;
executable `target/debug/deps/inference-90a7e30842e29968` in that worktree.
Production function bodies were unchanged after its build; later private tests
and rustdoc changed.

Model: `agents/processing/models/ggml-base-q8_0.bin`, verified SHA
`c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9`
from the existing Windows CI fixture script. Logs: `whisper-kernel-*`.
No Docker daemon was available; actual Linux inference is a CI/container gate.

Portable STT guidance: use listener2-core directly; transcription-core pulls
listener-core/audio/device/host. local-stt-core is catalog DTOs, not inference.
Start server CPU Whisper with explicit verified model path/use_gpu(false), keeping
desktop availability/catalogs unchanged. Reuse pinned Linux ORT 1.28/C++ setup
from `.github/actions/cli_native_setup/action.yaml`; extend server CI/container
centrally later. Soniqo/diarization Linux expansion is separate.

## Capture worker — clean storage checkpoint, native design only

Cwd `/Volumes/T7/src/loofah-separation-capture`, branch
`refactor/backend-capture-spool`, clean HEAD
`7549e2d206c7d7a0f099a6c16244cf22b490dd58`.

Prerequisite `4a4b88c66391d56f52f79271acf80dab6645a821` only reexports the portable
session validator. Root already provides that reexport; do not blindly apply the
duplicate. Review/cherry-pick spool commit 7549e2d itself after root is clean.

New four-file private capture_spool module in loofah-client plus lib registration.
No network, native recorder, canonical vault writes or deletion API. Immutable
server/member/vault/capture/request identity, pending/existing-session targets,
16 kHz finite f32 LE/channel map, ordered checksummed immutable chunks, bounded
constant-size manifests, exclusive owner lease, quotas, owned blocking I/O and
receipt/publication observations. Ack/publication observation never deletes audio.
Exact portable session names and macOS/Windows case aliases are covered. Host
must supply canonical app-private ancestry; test TMPDIR aliases are canonicalized.

18 focused regressions, 4 protocol tests, check, strict no-deps Clippy, formatting
and drift pass. Explicit accelerated two-hour synthetic stereo test: 7,200 chunks,
115,200,000 frames, 921,600,000 PCM bytes; source SHA
`0cb87ebedafe623b3e747d482fb60e798378669c9af6604f6afc77dfdf234885`;
append-time manifest max750 bytes, final manifest1752 bytes. Reopen retained all
chunks after seal/ack/publication. Final run524 s under shared I/O load; first
run256 s. This is storage proof, not a two-hour real device recording.
Evidence: `/Volumes/T7/loofah-backend-build/agents/capture/capture-verification.json`.

Next native slice was designed read-only, **not implemented/authorized for edits
before this pause**. Proposed listener-core DeviceRecordingSink trait + opaque
DeviceCaptureKey; desktop implements it with CaptureWriter, avoiding a
listener-core → network-client dependency. Internal LocalDisk vs Device recording
destination; Rust-only transcription plugin bridge, no public command changes
needed for first internal slice. Coordinate exports/module exceptions first.

Required seams: root actor unconditionally resolves vault_base (also completion
temp fallback); supervisor constructs local disk args and currently restarts
failed recorders/discards queued frames on stop; start_capture reserves managed
local store; TauriRuntime emits unscoped events; recording_meta stamps local
sessions. Device captures must never use these local paths/events, even with the
same session ID. A device runtime rejects storage and redirects all events to a
separate scoped callback; prove storage methods are never invoked.

Proposed bounded delivery: one-second 16k-frame stereo chunks (128kB), short aligned
tail, eight seconds of frame credits across producer/source/recorder/durable
append plus bounded messages/format validation. Native input queues already have
bounds; new credit must be acquired before unbounded actor casts. Account for raw,
AEC and conversion overhead rather than claiming exactly1MB total memory.
Retry only safe Busy with identical pending bytes/sequence, 10–100ms capped
backoff; no credit release until durable confirmation. Prioritize capture over
future background uploader/inventory. Warn after1s pending; proposed stop after5s
without durable progress or credit overflow, drain admitted frames, report
Interrupted on failure; never silently drop/restart/seal after loss.
Need producer stop/join → source barrier → recorder finish barrier.

Frames remain capture-relative. Preserve existing expected_audio_sha256; no
canonical append/replacement, transcript offsets or implied server admission.
Later server finalization owns append/CAS and concurrent-capture conflicts.
Pending captures can seal without a canonical session ID. Test fake source→real
spool, tails/drain, Busy/failure/cancel/restart/device change/same-ID isolation,
stale finalizers and absence of model/network/local-vault dependency. Full native
Mac/Windows and actual long-duration capture acceptance remain open.

## Resume order and unfinished goal

1. Confirm explicit resume, reread applicable AGENTS/skills, inspect goal/status
   and compare worktrees to the saved manifest. Set all artifact/cache paths to
   physical T7 before commands; no local task process needs resuming.
2. Refresh previous-head CI, keep Windows gates honest, and review root's clean
   API checkpoint. Move frontend WIP onto that base safely and validate it.
3. Integrate/review the three summary commits and spool commit separately into
   clean root checkpoints; regenerate shared artifacts centrally and run combined
   relevant checks. Resolve the direct-core cancellation/lease review item.
4. Finish corrected Whisper tests/CPU validation; coordinate the native capture
   sink slice and device-recents interfaces before parallel edits.
5. Continue the complete plan. Main profile selection/remote-only startup,
   remaining scoped UI/mutations/device state, durable upload/service ingestion,
   live/batch transcription/repair, title/tag/template/preview/auto-trigger parity,
   owner-managed credentials/model resources, imports/exports, CLI active-owner
   routing/remote CRUD/MCP read-only, Linux CPU/container, backups/upgrades and
   production/native/network/long-outage acceptance remain substantive work.

Title-policy analysis to preserve: current title input is generated summary or
joined enhanced content, not transcript; stamp nonempty outputs and separately
nonempty raw notes; exact first-line '# title' is unchanged, otherwise prepend
without deleting differing H1. Honor blank/current title and preserve unsaved
client drafts; server cannot infer them from a submit-time boolean. Existing
template regeneration uses the global prompt and preserves output metadata;
do not invent a per-template catalog. Previews should be private bounded
generation/sequence snapshots with invalidation notifications. Auto rules use
completion/finalization, not disconnect/index events; current eligibility is
at least5 words and160 normalized Unicode characters, retry20×500ms.

Do not shrink the remaining scope or mark the goal complete merely because
individual checkpoints pass. The pause is the user's request, not a blocker.

# Frontend/backend separation continuation

## Current state

The user resumed **all work**. The implementation goal remains active; this is a continuation checkpoint, not a pause or a completion claim.

- Worktree: `/Volumes/T7/src/loofah-backend-separation`.
- Branch: `refactor/frontend-backend-separation`.
- Committed source checkpoint: `67ea69e50` (capture source `0868995b4`, strict-metadata fixture `a4e82f668`, scoped tag UI `67ea69e50`).
- Last pushed head: `93dada10bb592c824ae59eeff72187b5079a9558`.
- Draft PR: <https://github.com/bart6114/loofah/pull/123>.
- Original checkout: `/Volumes/T7/src/free-meeting-transcriber`, `main`; tracked files remain clean and untouched.

Default local use stays standalone, offline and login-free. Preserve macOS and Windows 11 x64/ARM64. Linux work is headless only. All local build, test, temporary, model, module-cache and log artifacts must resolve physically to T7. No bases, CRDTs, general remote-vault offline editing, mobile or real-time collaborative editing. Do not merge or release before the full plan's acceptance gates pass.

Read live Git status/HEAD, process state and CI before resuming. The IDs here identify the checkpoint; later documentation commits do not change its source. At this save, Root's shared/native/frontend validation pipelines have completed successfully and no Root build remains running. Workers have independent work in progress; B currently owns the exclusive shared-target validation window. Root also has two unreferenced WAV helper drafts, described below; they are not compiled or committed.

## Verified changes in this checkpoint

The preceding pushed batch contains shared core/IPC/HTTPS operations, authenticated SSE, scoped frontend clients and caches, opt-in notes workspace, checked editing/task/metadata operations, media/attachments, processing settings and summary/existing-template jobs, CLI/MCP remote reads, device draft/submission recovery, and internal managed device capture. Consult the implementation plan for earlier evidence and outstanding full-app migration.

New source since that pushed batch includes:

- Portable title generation (`3269a31ae`), with model/settings parity, bounded output and real cancellation/drain ownership. Root's helper tests passed 10 cases; seven native ChatGPT model tests and the normal desktop check passed. This alone does not enable automatic titles.
- Durable first-audio observation (`da21ecd76`, `775f4e6bf`). Schema 2 stores an explicit producer observation; schema-1 captures remain unchanged until an explicit upgrade. Identity, chunks, seals and source hashes do not change. Root passed 28 storage cases; three subprocess/stress helpers are ignored directly and exercised separately where applicable.
- Bounded registered/in-use tag lookup and checked suggestion storage (`cdf32fd91`, worker `9ef6963e2`), followed by the complete tag API (`3b7dce4ac`, worker `da1afa1e7`). Lookup semantics follow the existing UI; malformed registries fail closed. IPC, Core, HTTPS and SDK share operations and contracts. Worker passed 65 focused cases, including real lost-reply tests at 50/150/500 ms. Root's integrated tag filters passed SDK 2, Core 13 and HTTP 5 cases; two HTTP cases also exercise capture staging. The login-free native tag test passed.
- Lossless checked metadata patches (`80eb96f5e`). Only requested owned fields are replaced in strict raw JSON; nested and top-level unknown metadata survives. Full prospective bytes are bounded. Eight metadata tests and the normal storage check passed. `UniqueValue` now lives in `metadata_revision.rs`; `tags/checked.rs` imports it. Preserve this parser extraction when integrating worker changes.
- Native timing durability (`03ac73463`, worker `147c0718f`). Host finish requires actual resource/supervisor proof, durable audio finalization and the matching durable observation. Busy timing retries do not hold the audio mutex; one same-value reconfirm handles an unknown acknowledgement. Terminal failure retains audio and reports unsafe completion. Root passed all **28 device_capture tests under default parallel scheduling**, the native tag test, native export and normal desktop library check. Unwired internal host APIs still produce 31 unsuppressed normal-library warnings; do not suppress them merely for a clean log.
- Prepared generation resource bridge (`bca63de3c`, worker `b362c1fb2`). `PreparedGeneration` has an optional consuming `Box<dyn TitleRunner>`. Prepared body/title calls pin one settings/model snapshot and retain the real vault context and both admission permits through actual I/O drain. Legacy `generate` stays body-only with its existing fresh settings/policy recheck. Worker passed configured 12, source 14, generation 13 and title 9 tests, all-target check and Clippy.
- Authenticated capture staging (`6dd8892e5`, `0868995b4`). Neutral storage atomically admits the original observation and compares declared seals before sealing. Core owns bounded operations and the vault lifetime after caller drop. HTTPS derives scope from current authorization, accepts binary chunks, returns exact prefix receipts and supports status/seal recovery. The SDK gates capability before credentials and validates receipts. No canonical recording, transcription or remote recording UI is exposed by this staging feature.

Capture validation passed:

- Neutral storage: **31 passed, 3 ignored**.
- Integrated capture filters: SDK 2, Core 5 and HTTP 4 passed, including two new Core ownership/authorization cases and all four new HTTP cases.
- Real SDK HTTP tests at **50/150/500 ms RTT** cancel a lost chunk reply, recover status, replay identical bytes and seal without rebinding.
- Shared Core/client/server **all-target checks passed**.
- Headless dependency graph passed: no Tauri, wry, cpal, audio-actual or device-monitor in Core/server normal dependencies.
- Actual protocol/native exports completed. Native bindings were formatted with oxfmt; protocol exporter output must stay exact.
- Desktop typecheck, changed-source dprint checks, README formatting and `git diff --check` passed.

Generated files in `0868995b4`:

- `apps/desktop/src/types/tauri.gen.ts`: SHA-256 `b44d8feb1477e5f927cbe5b5e00f4f5aa7df2a2166b63557594dd74249a66075`.
- `apps/desktop/src/shared/application/protocol.gen.ts`: SHA-256 `444f3d8ba28de56443f8f9437a182421b9fcee6d0f9639c372c4c2054767c3b8`.

## Hosted CI and publication

Last fully hosted-validated head is now `93dada10bb592c824ae59eeff72187b5079a9558`: all six workflows finished successfully, including both native Windows architectures. Its Linux CPU lane proved the portable Whisper kernel and existing batch/live inference with a checksum-pinned model and actual `GGML_NATIVE:BOOL=OFF`; it did not prove an authenticated server transcription endpoint.

At the latest live check for pushed `93dada10bb`:

- General, backend, CLI, macOS desktop and security workflows succeeded.
- Windows x64 checks/tests, installer and install/uninstall validation succeeded.
- Windows ARM64 checks/tests, installer and install/uninstall validation succeeded.
- Speech-model dispatch was skipped, so this is not hardware/model acceptance.

Windows run: `42427103372`; x64 job `128567613798`, ARM64 job `128567614002`. The Windows run is now complete and healthy, so the next validated batch may be pushed. Continue the rule for later batches: **do not push again until the preceding Windows run finishes healthy.** No new source above `93dada10bb` has been published at this save; verify remote after the documentation/catalog checkpoint.

## Active worker ownership

Reuse the three authorized agents; do not spawn a new team.

### Frontend — `/root/frontend_distribution`

Current UI worktree: `/Volumes/T7/src/loofah-separation-tag-ui`, branch `refactor/backend-scoped-tags`, exact base `3b7dce4ac`.

UI checkpoint worker `f4996b567` is integrated as Root `67ea69e50`: exactly nine approved source/test files, including the existing remote workspace slot. Both Root-generated validation files remain excluded from the worker commit and match the SHA values above. Worker passed **2,078 desktop tests / 248 files**, 124 focused tests, typecheck, owned formatting and type-aware lint (three established warnings). Root passed the 124 combined tests, actual-binding typecheck, locale extraction and strict compile. Root is committing all 109 locale catalogs for the new `Reload tags` label.

UI uses checked metadata for manual add/remove and typed accept/dismiss, preserves current state on conflicts/unknown outcomes, refreshes canonical state before explicit retry, and captures immutable scope/client ownership. Canonical read counts protect canceled refreshes and ABA revisions. Default local operation works while the browser reports offline and requires no login.

New assignment is a **read-only audit** of remaining main-shell/profile/remote-only startup coupling. Identify the next actual main-app slice, query/event/direct-command boundaries, cache/draft/window ownership and native prerequisites. No further profile/client/native edits are authorized until the next boundary is coordinated. No Rust/Cargo in UI worktrees.

Preserve the clean API tree `/Volumes/T7/src/loofah-separation-tag-editing` at worker `da1afa1e7`; never cherry-pick its duplicate storage ancestor. Five earlier UI drafts were restored/hash-verified from `/Volumes/T7/loofah-backend-build/agents/tag-editing/logs/ui-before-root-3b7/manifest.json`.

### Backend — `/root/backend_distribution`

Current tree: `/Volumes/T7/src/loofah-separation-title-stage`, branch `refactor/backend-title-stage`, exact base `3b7dce4ac`.

First checkpoint adds protocol `TitlePolicy::Preserve | GenerateIfEmpty`, defaulting/omitting Preserve so old serialized request bytes remain exact; optional additive title result details; constructor-only Rust caller changes. Contract checkpoint is clean worker `7eb07172eee12a154e4736f43cdc5ffe884b0947`: four golden tests, all-target checks and 14 existing device-submission tests passed (one ignored helper). It is not integrated here. No server capability, SDK gate, UI or generated files are owned by this worker.

Core durable body-confirmation/title sidecars and orchestration are implemented and have compiled; B now owns the exclusive shared-target validation window for exact-binary provenance, new/existing tests, crash cases and default checks. Its review snapshot has roughly 700 production lines plus ledger, worker, recovery and real-transport fixtures, including eight actual child-process crash points. Worker is tightening cancellation admission atomically before Applying and releasing the large saved snapshot before long title work. The implementation uses with strict bounded private records, preserved immutable body output, existing public Applying phase, real auth/cancellation/resource drain and no paid replay after restart. Confirmed body success survives optional title empty/failure/cancellation/conflict/uncertainty. Capture the opaque title baseline after body confirmation, then verify the target still equals the saved body revision. Recovery never recaptures/replays a candidate automatically.

**Do not expose/push a title-policy request that current worker orchestration silently ignores.** Root must pair the full worker implementation with server enforcement, capability/SDK gates, native exports and later scoped live-title-draft protection. Preserve requests keep the legacy body-only path. Stored opt-in records require explicit downgrade/version handling.

The prepared-generation tree `/Volumes/T7/src/loofah-separation-prepared-generation` at worker `b362c1fb2` remains clean; its code is already integrated.

### Capture — `/root/capture_distribution`

Current tree: `/Volumes/T7/src/loofah-separation-recording-publication`, exact base `3b7dce4ac`.

Implementing a WAV-only, **recording-absent** vault-write publication primitive, new `recording_publish/` files plus minimal lib registration. No Core/protocol/SDK/server/native/Cargo expansion is delegated. Core will provide a verified encoded reader, immutable authenticated source/seal binding, artifact hash/length and checked producer timing. Encoding and PCM verification remain in Core.

Existing recordings must not be overwritten: later support needs baseline-plus-capture append, baseline CAS, preserved original start and transcript offsets. Existing canonical/legacy recordings and conflicting transients reject absent publication.

Use audio → transcript-operation → short store guards. Stage/hash outside the global write lock, construct fresh raw owned metadata under the final commit guard, durably journal publication, and retain leases through caller drop. Never consume/delete capture-store audio. Preserve attachments and unknown metadata. A private MetadataWritten phase pins recording/layout/timing proof while allowing unrelated title/tag edits during final checksum and recovery. Actual canonical content must be verified before a completed receipt. Core startup/read/STT gating and publication recovery wiring remain Root work.

C completed its exclusive shared `target-root` validation window. An interleaved Cargo invocation once returned the Root baseline executable with **zero publication tests**; that result was rejected. Its `.d` file proved the wrong source path. That rejected result was not acceptance. C released the window after complete verified checks; B now owns the next window and Root holds new Cargo commands.

The supported targeted rebuild uses `--config 'profile.test.package.vault-write.debug=1'` in the existing shared target, parses Cargo JSON requiring `fresh=false`, pins executable/source paths and SHA, and verifies exact nonzero test names before execution. No cache deletion, alternate target or global rustflags. A verified full run passed **479 tests / 5 ignored**, with 484 listed cases and all 26 publication cases, including six actual subprocess kills. Final checks and the clean feature commit completed successfully. A same-call metadata recheck preserves external edits after the canonical rename. Random create-new staging with RAII only cleans its own pre-intent temporary file; a validated durable receipt pins the retained basename. Unknown hidden files and pre-receipt crash orphans remain untouched.

Root's strict-snapshot fixture prerequisite `a4e82f668` was cherry-picked separately in C as `1dd5c5c45`. Root's own complete storage suite passed **453 / 5 ignored**, plus cargo check. Do not cherry-pick the prerequisite twice when integrating the publication feature. Ordinary checks/library Clippy passed with known baseline warnings; all-target strict lint still reports two unchanged flat-layout permission lints. Do not suppress or fix unrelated files.

Publication feature is clean worker `c5eed7f86a4305555b86a0ef93d90e00fc745a61`, not yet integrated. Pick only that feature commit; the prerequisite duplicates Root's fixture fix. Proof: `/Volumes/T7/loofah-backend-build/agents/capture/recording-publication-verification.json`, SHA-256 `e0b60e4d728686a12457a37dd24b9c5f0a20edeeb5e51fefb954d1b0cc23b0bf`. C has no remaining processes and is holding new work. B now owns the next exclusive validation slot; Root must keep Cargo idle until it releases.

Clean timing tree `/Volumes/T7/src/loofah-separation-capture-timing` at worker `147c0718f` is preserved; Root has already validated it.

## Immediate continuation

1. Verify current processes, branch/status and live CI. Preserve healthy builds. Committed source is `67ea69e50`; catalogs/docs and the two WAV drafts are not yet committed at this save. Windows for the prior pushed batch is fully green.
2. Commit/check the integrated tag locale and documentation checkpoint, then push the validated batch and update/read back draft PR metadata. Keep generated files Root-owned. Do not include uncompiled WAV drafts in that batch.
3. Review publication primitive and wire Core/server capture publication: independent PCM/chunk verification, bounded portable WAV encoding, canonical session creation/binding receipts, fresh authorization, startup/read recovery gating and exact publication proof. Keep existing-recording append a separate explicit checkpoint.
4. Complete/pair optional title worker, server/SDK capability enforcement and native bindings; then add the scoped UI opt-in and live-title-draft protection. Preserve existing default behavior throughout.
5. Finish the main-shell/profile audit with A and coordinate its next actual frontend slice. Implement trusted native capture source admission, app-managed host/quit hooks, durable scope-bound uploader/reconnect/recovery and actual scoped UI integration. Capture must continue locally through network/auth interruption. Queued audio uploads are not general offline editing.
6. Finish portable live/batch STT, existing-recording append/offsets, imports, provider credentials, main-shell/profile/remote-only startup, local CLI owner routing and remaining feature/deployment/platform acceptance. The phase checklist is still open; do not mark the plan complete based on these internal checkpoints.

## Root WAV drafts (not compiled)

`crates/loofah-core/src/capture_ingest/wav.rs` and `wav_tests.rs` are unreferenced drafts for the next publication integration. No Hound dependency or module registration has been added. They verify the complete raw seal/chain and derive a bounded float32 WAV header using the pinned Hound encoder, then supply a one-slot streaming reader retaining the real context. Draft tests cover exact float bits/channel order, post-verification source corruption, bounded reader ownership and a two-hour header without materializing audio. These are **not validation evidence** until compiled and tested. Review/add dependency/module only after the current batch is saved and a shared-target window is available.

Core's first publisher should require an existing session with absent recording; clients can create that note online before capture. Reject pending-session publication and existing-recording append until their separate reservation/CAS/offset work is implemented. Root also needs bounded private pending-receipt discovery and bootstrap/read/STT recovery gating; do not expose half-published canonical audio after restart.

## Commands, evidence and constraints

Root logs live under `/Volumes/T7/loofah-backend-build/logs/`, including:

- `metadata-raw-owned-tests.log`, `capture-ingest-storage-{tests,check}.log`.
- `capture-ingest-service-tests.log`, `tag-editing-integrated-tests.log`, `capture-ingest-service-check.log`.
- `capture-native-timing-tests.log`, `tag-editing-native-tests.log`, `tag-editing-native-export.log`, `capture-native-timing-check.log`.
- `capture-ingest-protocol-export.log`, `capture-tag-bindings-typecheck.log`, `capture-ingest-final-source-format.log`, `capture-ingest-headless-dependencies.txt`.

Set these before local commands:

```sh
export PATH=/Users/bart/.cargo/bin:$PATH
export TMPDIR=/Volumes/T7/loofah-backend-build/tmp
export NPM_CONFIG_CACHE=/Volumes/T7/loofah-backend-build/npm-cache
export DPRINT_CACHE_DIR=/Volumes/T7/loofah-backend-build/dprint-cache
export CLANG_MODULE_CACHE_PATH=/Volumes/T7/loofah-backend-build/clang-module-cache
export SWIFTPM_MODULECACHE_OVERRIDE=/Volumes/T7/loofah-backend-build/swift-module-cache
```

Use the installed Cargo home without repointing/seeding it; local Rust builds use `--locked --offline`. Targets resolve physically to `/Volumes/T7/loofah-search-build/{target-root,target-tauri}`. `~/.cache/loofah` is a real directory whose two target children are symlinks to T7. ORT cache resolves to T7. Fresh worktrees run the idempotent shared-target setup and verify physical destinations before building.

Native commands run from `apps/desktop/src-tauri` to preserve the warm native graph. Use `--lib` for focused package tests to avoid unnecessary zero-test binary launches. New macOS binaries/rustc processes can wait minutes before meaningful CPU/harness output; investigate only genuine stalls, do not cancel healthy work. Different shared-root-target worktrees may replace same-package artifacts between steps. Use explicit exclusive validation windows in addition to Cargo locks, and pin source/test-list/executable evidence. Reject zero-match filtered runs. Do not bypass the shared cache lock.

**Freeze relevant Rust sources in each worktree through the full dependent build/check pipeline.** A yielded exec session is not completion. Wait for success before editing dependencies or beginning dependent operations. Separate worktrees may proceed independently. Export protocol/native bindings centrally; oxfmt the native file from Root, never reformat protocol exporter output. Putting contract export before a long final check can unblock frontend work sooner in future batches.

Use `pnpm -F desktop typecheck` and focused `pnpm -F desktop test --maxWorkers=2 ...`. Apply the repository run-tests and Axum transport skills; prefer in-process tests for routing and real listeners for actual SDK/latency/disconnect behavior. GitHub's full hosted result is the merge gate. Do not fix unrelated warnings or broaden checks after relevant gates pass without a new concern.

## Preserved recovery material

These are backups, not instructions to reapply older source:

- Root capture-before-tag stash: `d13afbb1cb44a7939c3a5755458f97652e3630b1`; 18-file manifest `/Volumes/T7/loofah-backend-build/tmp/capture-ingest-before-tag-integration.json`. Restore was verified: only four expected tag integration regions differed. All final source is now committed in `0868995b4`.
- Prior frontend two-file WIP stash: `cc0165379ce1f1d6d20f56ff1ebb0fe4995f74eb`.
- Prior Root template backup: `757c1aa385a2398bd8643eede0f43b8e87dca9f5`; T7 `template-transport-before-drain-join.json` manifest.
- Historical pause archive: `/Volumes/T7/loofah-backend-build/pause-2026-11-07T093537Z/`; never extract over newer source.

Avoid full merges of divergent cherry-picked worker histories. An earlier capture merge produced 12 conflicts and was safely aborted. Use a fresh exact Root base for new dependencies and integrate scoped commits. Never overwrite source with stale generated contracts or reapply an old stash just because it remains listed.

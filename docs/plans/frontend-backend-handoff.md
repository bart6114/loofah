# Frontend/backend separation continuation

## Goal and fixed constraints

Fully implement, test and validate [the plan](./frontend-backend-separation.md).
The goal remains active across status questions and intermediate checkpoints.
Standalone Local remains the default, offline and login-free. Explicit saved
Remote profiles never fall back to a Local vault. Preserve macOS and Windows 11
x64/ARM64; Linux is headless only. Exclude bases, CRDTs, mobile, collaboration and
general remote-vault offline editing. No merge or release yet.

Use existing isolated worktrees. All build/cache/temp/log/model/test artifacts
physically use T7. Original `/Volumes/T7/src/free-meeting-transcriber` tracked main
remains untouched (`?? docs/plans/` only). Do not restore old stashes or cancel
healthy builds/CI. Do not create a new tree/cache to bypass a build lock.

## Current authoritative source

- Integration: `/Volumes/T7/src/loofah-backend-separation`, branch
  `refactor/frontend-backend-separation`.
- Published checkpoint: `f5a30b2ad78c37b81de82e53634649c0e464bf70`.
- Draft PR: <https://github.com/bart6114/loofah/pull/123>. Its head, body and draft
  status were read back after the push. No merge/release.
- Current combined source after six later unpublished commits:
  `c04e918bad324a4457cdfbb5ac490331ab4ab7ba` (plus this handoff update).
- Those six commits integrate B policy/routes (`e8e0f0c56`), C checked pending
  publication (`168ba1275`), distinct pending capability guard (`518c78ed5`), A
  admission gate (`942ef712e`), C capture lifecycle (`556a78f6d`) and disabled-live
  retained-journal review (`c04e918ba`). Do not push the next batch over healthy
  Windows jobs; finish relevant contract generation/checks first.

The published checkpoint includes startup/profile lease fixes, scoped content and
attachments, CPU deployment, portable uploader/coordinator, Local legacy controls
and scoped undo, bounded main timeline, the captured main-workspace adapter,
internal Core CPU live processing, native recording controls/recovery and the
experimental recording UI. Earlier details in the main plan are historical.
Update this handoff/current summary, not another huge chronological audit.

## User-visible behavior and outstanding integration

Local startup still opens the existing standalone app. Remote startup resolves a
saved profile before opening any Local vault/Store/index or Local STT supervisor.
It currently opens the experimental notes workspace, with a device recording host
outside the authenticated vault tree. Existing unrecorded notes can capture audio;
Stop and discovery of retained captures survive network/sign-in loss. Capture
identity/request UUID, original member/vault, raw source and observed producer time
remain bound. A lost start reply is Unknown; repeating the same UUID cannot start
another microphone session. View teardown stops subscriptions, not recording.

Private device recovery inventory spans original accounts/vaults within the active
profile without a server login. It never exposes an unfiltered remote listing.
Memory status/cache plus upload progress avoid rescanning disk on every frame.
New start invalidates old complete-inventory caches before admitting capture.
Unknown inventory cannot prove idle. Device audio is never automatically deleted.
Raw audio export/local recovery is still needed; the current queue retains data
and can resume an upload to the original authorized destination.

The actual main timeline uses bounded Date/Upcoming/Undated pages and complete tag
facets, exact revision/cursor binding, explicit pagination/count/retry, bounded lazy
tag admission and loaded-only selection. The real captured main workspace and
editor work through scoped clients and private AI/listener stores. It is NOT yet
selected by Remote bootstrap: remaining shared actions/settings/processing,
recording activity, imports/exports and related gates must be connected. Unsupported
legacy views/actions cannot load Local credentials/models or write a Local vault.

The server Core live worker tails the same durable chunks; no second PCM upload
channel is required. Native HTTP/SSE subscription bridge exists. Automatic shared
recording orchestration (live or batch repair, then summaries) is still needed;
do not place that business workflow in React or require a desktop callback to
complete server work. Use stable persisted identities, existing job services and
fresh membership authorization through actual canonical commits.

## Fixed trees, source owners and cache reservations

- **A `frontend_distribution`**:
  `/Volumes/T7/src/loofah-separation-document-editor-ui`.
  Timeline `95ac64b72` is integrated as `49c838f03`; captured workspace `3a08bc056`
  as `223bad704`. A now owns protocol/Store/Core checked session delete/restore,
  lifecycle gate and summary/batch durable admission/busy checks. Same tree updated
  to combined `c04e918ba`; 16 owned WIP paths archived before update. Only narrow
  A module/job hunks were restored, preserving incoming C/B fields.
  A owns **target-root now**, first frozen check session `71974`, log
  `agents/document-editor/logs/lifecycle-first-check.log`. No Native/model work.
- **B `backend_distribution`**:
  `/Volumes/T7/src/loofah-separation-batch-transcription`.
  Validated timeline/live/policy `36d72f831` is integrated. Same clean tree updated
  to `c04e918ba`, only storage lease helper reapplied as `602d97239` (original source
  checkpoint `9cde856e2`, not yet compiled). B now owns Local CLI/MCP owner bridge,
  portable ownership helper, Store I/O pin adoption and Context/Runtime lease
  conversion. Source-first; no Cargo reservation while A uses target-root.
- **C `capture_distribution`**:
  `/Volumes/T7/src/loofah-separation-transcription-cpu`.
  Validated Core live `3e8d17843`, pending `a8edbf9e5`, pending-cap follow-up
  `5c0ab6bd4`, lifecycle `ab82e060b` and disabled-state fix `76d892f91` are integrated.
  All C processes ended and target-root explicitly released to A. C next owns a
  shared INTERNAL rich-create contract for existing CLI metadata flags, coordinated
  with B, preserving old/default/pending receipts and atomic initial note creation.
  No Cargo while A owns root; no Native/Store-lease edits.
- **Root**:
  integration, actual exports/contracts, frontend recording UI/activity, selected
  main activation, native ownership/startup/quit, shared recording workflow and
  remaining acceptance. Fixed Native source runner:
  `/Volumes/T7/src/loofah-separation-desktop-bootstrap`, branch
  `refactor/backend-desktop-bootstrap`, currently clean `1771eacd1` after C pending
  validation. Native recovery source `acee60402` is integrated as `21a8e7bdc`.
  **target-tauri is free for Root**; no current Cargo/test process there.

Physical caches: `/Volumes/T7/loofah-search-build/{target-root,target-tauri}`.
Keep the source of each running build frozen. Use the fixed runner for Native
checks; no second absolute-source-path Native build. Export native bindings from
`apps/desktop/src-tauri`, then oxfmt from repo root (paths containing `..` fail).
Protocol export format remains generator-owned.

## Near-term junctions and critical facts

1. A contracts: `SessionLifecycleSnapshot`, `DeleteSessionRequest/Outcome`,
   `RestoreSessionRequest/Outcome`; Core `read_session_lifecycle`, `delete_session`,
   `restore_session`, with authorized variants. Expected revision and trash token
   are opaque 64hex; no paths/force in HTTPS. Replay is historical, never proof of
   current presence. Private receipt details for Local CLI preserve original
   path/trash_path/deleted_at; timestamp persisted once. Prepared unknown intent
   must not become NotCommitted merely because a later admission is busy.
2. Core gate: shared `admit_session_work` through actual durable admission/binding;
   non-queuing exclusive `try_session_lifecycle_change` for delete/restore. No
   inference-long write lock. Active/uncertain journals and capture work block
   deletion. C aggregate `ensure_capture_session_lifecycle_idle` covers original,
   persisted and deterministic Pending targets, all members, live commit_pending
   and retained journals even when the model is disabled. Unknown evidence fails
   closed; unknown private files remain preserved/ignored.
3. B ownership audit found desktop and old CLI do not currently acquire the
   canonical vault lock. Native MUST acquire it BEFORE Store/startup/watch and
   publish its owner endpoint only with that proof. Remote startup acquires no
   Local lease. Pin the original descriptor through Store clones and actual
   blocking I/O, not just request waiters. B APIs: portable `VaultLease`, typed
   Store pin, Context `with_vault_lease`/`lease_pin`; no explicit early unlock.
   C disabled-ledger File clone must become an original Arc pin with B's seam.
4. CLI owner transport: same-user Unix socket/macOS and named pipe/Windows,
   per-operation resolver (idle MCP holds no owner), no direct fallback while a
   lock is held. Stream large content with bounded frames; no new hard Local note
   cap. Preserve all existing commands/output; import/transcribe need injected
   device host hooks and shared checked canonical application. Old pre-bridge
   desktops need a stopped upgrade; discovery cannot prove they obey a new lock.
5. B model policy is authenticated and memory-only, exposes only configured public
   alias, sanitized availability and batch/live flags. No model path/hash/error or
   secret. Register it in protocol exporter and regenerate actual Native bindings.
   Add configured-host `capture_live` and `capture_pending_publication` capabilities
   only with the matching verified implementation. Both remain unadvertised in
   the current published build. Do not guess model names.

## Verification and hosted state

At published `f5a30b2ad`:

- Whole frontend: **2,510 tests / 277 files**, typecheck, extraction and strict
  compilation of all109 locale catalogs passed. One outdated enhanced-summary
  fixture lacked captured activity; Root updated only that mock, verified its32
  focused regressions, then reran the whole suite green. Lint: two pre-existing
  warnings, zero new warnings/errors in Root-owned UI.
- Native through Cargo: **132 desktop +33 capture-store passed**, existing ignored
  opt-in helpers unchanged. Actual binding export passed separately. All-target
  desktop/Store check passed. The two Codex probes failed only on direct binary
  launch, but the SAME executable passed exact probe and complete Cargo suite;
  they are NOT excluded from the required suite. Use Cargo environment for Native
  runtime tests; retain pinned artifact/source provenance.
- Main timeline149 and captured main-workspace110 focused checks passed; the
  combined frontend run includes them. Local offline/no-login and no Local
  provider/model side effects under the unsupported host were checked.
- Core CPU live: actual20s dual-channel case produced first finalized words1.21s
  after start, before seal, then verified consumed-source seal/SHA, canonical audio
  and transcript, unknown data preservation and real drain. No two-hour/physical
  acceptance claim. Proof `agents/capture/logs/capture-live-verification.json`.

Later unpublished source:

- B timeline17, liveHTTP3, SDKlive3 at50/150/500ms with exact prefix reconnect,
  policy4 and old focused regressions passed; default/CPU all-target checks and
  library Clippy passed. Proof `agents/batch-transcription/logs/timeline-live-policy-verification.json`.
- C pending31 passed on fixed runner1771, checks/Clippy passed. Lifecycle final53,
  default all-target check and Clippy passed on76d. Proofs
  `agents/capture/logs/pending-capture-native-verification.json` and
  `agents/capture/logs/capture-lifecycle-final-verification.json`.
- Root logs/provenance: `device-live-recovery-source-verification.json`,
  `device-live-recovery-cargo-full-tests.log`,
  `recording-timeline-main-frontend-full-final.log`,
  `recording-timeline-main-frontend-typecheck-final.log`, all under
  `/Volumes/T7/loofah-backend-build/logs/`.

Fresh CI on f5a: backend42480766955, Windows42480766961, desktop42480767023 are
running. CLI42480767208, generalCI42480766950 and zizmor42480767173 passed.
Read current results, do not infer completion from these initial states. Previous
c32 Windows ARM64 completed successfully; its x64 packaging and macOS dependency
fetches failed externalHTTP504, not Rust compilation. Do not cancel healthy new
Windows jobs or push another batch over them. No Windows speech-model acceptance
claim from skipped jobs. Last wholly green earlier head was93dada10bb.

## Remaining full-goal work

Complete main feature activation, shared provider credentials/settings, imports and
exports, CLI/MCP owner routing/all mutations, native lease bootstrap, automated
recording/live/repair/summary workflow, pending controls and append, audio recovery
export, retention/cleanup, headless CPU image and fresh Linux real-live validation,
backup/restore/upgrades/version compatibility, two-hour outage/restart capture and
production macOS/Windows responsiveness/UX. Keep the objective active until every
requirement and release gate is proved; a useful checkpoint is not completion.

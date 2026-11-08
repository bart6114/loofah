# Frontend/backend separation continuation

## Current state

The user resumed everything. Active goal: fully implement, test and validate
`docs/plans/frontend-backend-separation.md`. All seven phases remain open.
This checkpoint is not a release or completion claim. Verify live state first.

- Integration tree: `/Volumes/T7/src/loofah-backend-separation`.
- Branch: `refactor/frontend-backend-separation`.
- Latest API checkpoint: `136812c9e`; interface checkpoint: `0ee7b680e`.
- Last pushed head: `66b88b660e86efb53a5938e45e872769d0f74153`.
- Draft PR: <https://github.com/bart6114/loofah/pull/123>; no merge/release.
- Original `/Volumes/T7/src/free-meeting-transcriber` tracked files remain untouched.

Default local use stays standalone, offline and login-free, including solo use.
Preserve macOS and Windows 11 x64/ARM64; Linux is headless only. Exclude bases,
CRDTs, general remote-vault offline editing, mobile and collaborative editing.
Use isolated worktrees. All local artifacts/caches/temp/logs/models must remain
physically on T7. Preserve earlier stashes and recovery copies.

## Latest integrated source

Earlier extraction, authenticated HTTP/SSE, scoped clients/recovery, media,
attachments, AI jobs and CLI/MCP reads are recorded in the main plan.
Newer commits include:

- `87997f6d5`: absent-only checked transcript publication, retained original result,
  durable receipt and crash recovery; 83 selected Store tests passed.
- `1424f2aaa`: complete stopped-storage restore with an empty cache; exact hashes,
  identities/access, creation replay, sealed capture publication and unknown files.
- `dfe918246`, `238c02a66`, `dc32e1857`: bounded transcript overview, actual contracts,
  scoped main consumers/people; 10 API tests, native login-free case and UI checks.
- `9f11e7a16`: unchanged canonical batch transcript mapper moved to transcribe-core;
  old listener2 path reexports it. Seven mapping/integration cases passed.
- `118f5f608`: typed f32 live Whisper facade with bounded admission, complete results,
  explicit gaps/backpressure and actual native drain. 69 unit tests and one real
  two-session CPU case passed. No public authenticated live session yet.
- `5955f278f`: publication fixtures use canonical temp parents on macOS. Strict
  private-directory guards remain unchanged. 62 focused publication cases passed
  under a deliberately symlinked T7 temp parent.
- `503972dcf`: Linux CI explicitly runs the real typed-live CPU fixture.
- `8bd9359b0`: schema-3 compound document device recovery; old versions keep bytes.
- `d5579b9bd`, `ea67486cb`: compound document DTOs and typed SDK transport.
- `20690fa2c`: owned startup/drop/shutdown/media drains; retry dirty transcript flush
  until actual completion. 111 selected worker checks, including 14 new regressions.
  Trusted operation root canonicalizes once; strict private descendants remain.
- `ff4ca7898`, `1f5278510`: compound Summary/template Store/Core transaction, schema-5
  recovery, raw-frontmatter/unknown-field preservation and foreign task-ID guards.
  All three revisions guard saves. Catalog authorization precedes Store locks.
  Store 91 cases plus one existing ignored; final Core 13 selected cases passed.
- `0ee7b680e`: actual native document editor commands and generated native/protocol
  contracts, local/remote client methods and Scope queries/confirmation. Confirmation
  cancels old reads and fetches canonical state, never seeds a historical save echo.
  The local-only legacy enhancement capability is stripped from remote flags.
  Native login-free command/export passed; 75 adapter/cache cases and typecheck pass.

Root integrated the thin document HTTPS adapter and SDK response-hardening
follow-up as `136812c9e`. HTTP uses one catalog grant before Store locks and through encoding;
post-save encoding failure retains Persisted outcome. SDK additionally rejects a
Summary with the wrong logical ID and Saved with a missing document. Independent
agent review found no further adapter/cache defect. All-target desktop/CLI and
server/client/Core checks passed with no owned diagnostics.

## Evidence

All paths below are under `/Volumes/T7/loofah-backend-build/`.

- `logs/document-editor-native-provenance.json`: fresh own binary, new login-free
  IPC case and actual export. Export must run from `apps/desktop/src-tauri` because
  its destination is relative. Format native generated output with oxfmt; never
  hand-edit it. Protocol export output stays in its generator format.
- `logs/document-editor-adapter-ui-tests-final.log`: 75 pass.
- `logs/document-editor-adapter-typecheck-final.log`: pass.
- `logs/document-editor-sdk-followup-provenance.json`: 22 SDK/recovery cases pass,
  including real TLS 50/150/500 ms transport fixtures and both new response guards.
- `logs/document-editor-server-tests-final.log` and
  `document-editor-server-provenance-final.json`: both HTTP cases passed. Initial
  fixture omitted `kind: template_output` and was corrected. The test drops a committed reply body and
  delays rereads; it is not a hard TCP failure or complete latency acceptance test.
- `logs/document-editor-adapter-source-sha256.json`: adapter source hashes.
- `logs/{restore-test,transcript-publication-integrated,transcript-overview-test,
  transcript-overview-native,transcript-overview-check,shared-transcript-mapper,
  typed-live-integrated}-provenance.json`: earlier integration proof.
- `agents/capture/logs/document-editor-store-core-final-proof.json` and
  `document-editor-auth-order-proof.json`: Store/Core/crash/auth-order proof.
- `agents/runtime-ownership/logs/verification.json`: lifecycle proof.

Require own compiler source path, `fresh=false`, dep-file, exact nonzero test
inventory, source and executable hashes. A zero-match run is not verification.
Never cancel healthy builds or wipe/bypass caches. Shared existing cache roots
are `/Volumes/T7/loofah-search-build/{target-root,target-tauri}`; use exclusive
validation windows, not just Cargo locks. macOS binary assessment may stall launch.

## Active distribution

- A (`frontend_distribution`): coherent main Enhanced compound editor/task migration.
  Old frozen tree `/Volumes/T7/src/loofah-separation-document-editor`, base
  `6fbac0fa6`. Its 34 owned files are archived/hash-verified at
  `agents/document-editor/frontend-contract-vv14mtld/`. A is authorized to create
  `/Volumes/T7/src/loofah-separation-document-editor-ui` from exact `0ee7b680e`,
  restore only verified owned paths and validate actual contracts. Root owns
  generated bindings/locales/interfaces. Review fixes include strict missing/empty
  uncertainty confirmation, opaque task-body refusal, mounted-only draft cleanup,
  captured attachment resources and a trusted local-only legacy generation gate.
  Do not activate the remote main shell through remaining legacy source paths.
- B (`backend_distribution`): durable batch transcription Core orchestration in
  `/Volumes/T7/src/loofah-separation-batch-transcription`, base `1f5278510`.
  Protocol-only clean worker commit `93e5f5071` passed 20 tests; not yet integrated.
  It injects a prepared engine and retains authenticated member/source/Context and
  actual drain through terminal publication; no inference restart after ambiguity.
  No Core Whisper/device dependency or public capability before paired CPU E2E.
- C (`capture_distribution`): read-only frontend/adapter review finished. Approved
  source-only preparation extraction and concrete CPU host in fresh
  `/Volumes/T7/src/loofah-separation-transcription-cpu`, base `1f5278510`.
  Own transcribe-core recorded_audio module, thin legacy lease wrapper and new
  unregistered server transcription_cpu module. Root owns hound/tempfile manifest
  edges and later optional feature/config/route registration. Coordinate trait
  details with B. Use explicit Ubuntu 24.04 CPU packaging initially; current
  bookworm default image stays unchanged. No Cargo without a reserved window.
- Root: integration, contracts, adapters, exports/locales, lifecycle/configuration,
  deployment, remaining CLI ownership, docs and hosted acceptance. Root's native
  Cargo and HTTP tests are finished; verify current cache reservations before builds.

## CI and remaining gates

At pushed `66b88b660`, backend (including container/Linux CPU), CLI, ci and zizmor
passed. Desktop frontend passed. Both Windows architectures completed native
checks/tests and installers successfully. macOS Rust failed publication fixtures
because `/var` aliases `/private/var`; the canonical-parent fixture fix is already
integrated and locally verified. Previous capture-lease race is fixed. The last
fully green head remains `93dada10bb`. Push the next validated corrective batch
without waiting for an old failed SHA to become green; never cancel healthy CI.
Update/read back PR #123 body/head after pushing. Skipped speech-model jobs do not
establish Windows real inference/device acceptance.

Remaining: main remote profiles/remote-only startup; remaining scoped content,
AI/streaming/import/export and CLI owner/mutation workflows; owner-managed
credentials; trusted device capture source/uploader, public controls, quit drain,
pending-note/append recovery; authenticated live/batch transcription and checked
promotion; portable diarization; verified source retention/cleanup; upgrade/
downgrade and full production macOS/Windows QA. Validate realistic RTT/disconnects,
two-hour recording, concurrent AI and realistic vault sizes before release.

Raw capture staging retains a 2 GiB/64-capture per-vault quota; publication does not
reclaim sources. Do not delete recoverable audio based only on chunk acknowledgments.
Systemd still has a 60-second forced-stop budget; permanent flush failure retains
ownership internally but can be externally killed. Full-storage restore with the
current binary passed; cross-version/operator rollback acceptance remains open.

## Preserved recovery

Pause archive: `/Volumes/T7/loofah-backend-build/pause-2026-11-07T093537Z/`.
Stash `d13afbb1cb44a7939c3a5755458f97652e3630b1` was restored/hash-verified and
incorporated in `0868995b4`; do not reapply it. Preserve other old stashes and A's
old tree/archive; do not mistake them for pending source instructions.

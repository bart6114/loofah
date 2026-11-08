# Frontend/backend separation handoff

## Status: resumed; implementation and integrated validation continue

The implementation resumed on Apple Silicon macOS in an isolated
`/Users/bartsmeets/src/loofah-backend-separation` worktree. The user's workstation
override replaces every historical T7 storage requirement for this run. Checkout,
Cargo targets and intermediates, downloaded dependencies, module caches, models
and logs use local storage. The local environment is recorded outside Git at
`/Users/bartsmeets/Library/Caches/loofah-separation/env.sh`; short Unix socket test
paths use `/private/tmp/loofah-owner-validation`. Both target symlinks resolve to
the local cache. Override both `CARGO_BUILD_TARGET_DIR` and
`CARGO_BUILD_BUILD_DIR`: the ancestor user Cargo configuration otherwise selects
the former external drive even with a different `CARGO_HOME`.

New integrated source includes durable explicit device processing declarations,
offline cancellation and bounded scoped caption observation; checked Local CLI
MP3 publication and transcript replacement through an injected portable host;
and owner-only provider credential storage with opaque revisions, endpoint
binding and per-admission key resolution. The Native owner endpoint composes the
existing managed Context and original lease. Its shutdown joins admitted owner
work before the final Store flush. Generated contracts and full Native/platform
validation remain integration gates; source checkpoints alone do not prove them.

Completed continuation checks so far (local logs are under the cache above):

- Original-head frontend baseline: 2,563 tests in 279 files. The current shared
  main/AI/device integration passes 2,577 tests in 280 files and desktop typecheck.
- All four formerly pending processing SDK TLS regressions passed. The initial
  provider/device frontend cohort passed 75 tests across five files.
- The full twelve-package Native/library run passed 1,665 tests and exposed six
  failures. Corrections cover Local CLI error/output/startup parity, absence-only
  transcript guards, the managed owner socket fixture, and real live-capture
  journal contention. Its failures are historical findings, not a current full
  pass; repeat integrated qualification after the remaining changes.
- A subsequent complete twelve-package Native/library run passed **1,710 tests**
  with 16 explicit ignored cases and zero failures. This supersedes the earlier
  six-failure run for that source checkpoint. The later Native maintenance and
  permission corrections passed 56 focused tests; all six CLI integration targets
  passed 32 tests, including complete-session trash/recovery, restricted parent
  permissions, first-cache initialization and persisted-write reporting.
- The actual server executable passed startup/SIGTERM and both maintenance command
  integration tests. Locale extraction and strict compilation passed.
- A 145-case focused Native/Core/server cohort passed, including deterministic
  live-ledger contention and Store Busy publication recovery, device processing
  and captions, profile/capture gates, retained media recovery, and stopped
  backup/restore compatibility checks.
- Two actual configured CPU scenarios passed with a freshly downloaded and
  SHA-256-verified `ggml-base-q8_0.bin`: canonical dual-channel batch transcription,
  and live captions before seal followed by matching canonical promotion.
- The seven-package all-target Cargo check passed. Later focused conditional
  summary/Local ownership checks passed seven cases; the actual Native exporter
  and two additional proof cases passed separately. Generated contracts came
  from the real exporters. This is macOS evidence, not Windows qualification.

Saved Remote startup profiles now mount the shared main shell; the opt-in server
notes dialog retains its smaller preview. Main summary/template controls and
legacy automatic dispatch adapters use scoped shared jobs. The optional durable
`only_if_empty` policy requires `conditional_summary_jobs`, keeps legacy request
bytes unchanged when false, and conflicts before generation if the reviewed
summary has filled or changed. This adapter work does not complete Native/Core
automatic processing durability before producer start.

Shared manual transcription now has a scoped optional client contract and UI.
The complete transcription policy/job capability pair is required. A device
receipt freezes the request UUID, source hash and model before transport dispatch;
loading recovery does not start inference. Explicit recovery reuses that request,
while acknowledged receipts refresh status without another submission. Standard
manual jobs preserve the absence-only transcript contract. Existing Local
replacement/import controls still require migration to checked replacement
authority; this UI does not grant it. Current-source integration and Native
acceptance remain pending for these additions.

Local shared lifecycle UI activation remains default OFF. Managed Local CLI
stale-channel deletion now uses the tested strict Native physical-resource host
proof; unregistered or uncertain hosts refuse. Recording append,
verified retention, complete Native automatic processing, realistic production UX,
sustained recording/network acceptance and final macOS/Windows/Linux qualification
remain required. Stopped backup/restore source and focused validation are present;
full service upgrade acceptance is still pending. Local stays standalone, offline
and login-free. Do not merge, release or bump versions.

## Transfer checkpoint (historical)

The user explicitly requested that this workstation pause after saving all current
progress to draft PR [123](https://github.com/bart6114/loofah/pull/123). All three
agents have stopped implementation and local validation. Root's remaining local
validation was interrupted deliberately for that request. No hosted CI was
cancelled. The implementation goal remains incomplete; do not merge or release.

[The continuation prompt](./frontend-backend-continuation-prompt.md) is intended
for the destination workstation. [Portable validation evidence](./evidence/frontend-backend-checkpoint-2026-11-08.json)
contains copied proof records, source hashes, test counts, pending commands and
an honest account of interrupted runs. Raw logs, binaries, model files, user
vaults and credentials are not committed and are not prerequisites for resuming.

## Authoritative source

- Repository: `bart6114/loofah`.
- Draft PR: <https://github.com/bart6114/loofah/pull/123>.
- Branch: `refactor/frontend-backend-separation`.
- Code checkpoint before this documentation commit:
  `305df8f1129e31bdfeaa3cf1f136e8a544e1d8e8`.
- Old integration path: `/Volumes/T7/src/loofah-backend-separation`.
- Original `/Volumes/T7/src/free-meeting-transcriber` tracked main is untouched.

Everything owned by the agents has been integrated into the branch. Do not
reapply original agent commits or merge their divergent worktree histories.
Current integrated checkpoints:

| Integrated commit | Contents | Validation at handoff |
| --- | --- | --- |
| `1912a62ce` | Scoped checked deletion/undo UI | 170 affected frontend tests and typecheck passed |
| `24527acff` | Lease-pinned recording recovery before Store readiness | Four managed bootstrap and two Store recovery cases passed |
| `6067361c9`, `945228d3b`, `c7e311b54` | macOS/Windows fixture corrections | Canonical-temp and raw-path cases executed locally; fresh Windows CI remains required |
| `e462df8c8` | Exact-UUID acknowledgement of queued live/batch persistence | 190 affected frontend tests and actual-binding typecheck passed |
| `09c3906b6` | Octal literals in two existing Unix permission tests | Mechanical correction after all-target Clippy exposed the baseline deny lint |
| `c0901de74` | Native/Core Local work registry, tracked native drain and physical-admission guard, shared journal/startup integration | Initial focused cohort passed; extended physical-admission test also passed in final interrupted combined run |
| `77da9494b`, `a816d6d25`, `e7330ef84` | Explicit server capture processing contracts, owned live/repair/summary pipeline and wrapper hygiene | C's exact feature source passed 95 focused cases plus one real CPU scenario; checks/Clippy passed |
| `e2958c78d` | Thin SDK declaration/status/cancel adapters and four real TLS tests | SOURCE ONLY: tests authored but not executed; check/Clippy pending |
| `4c819d8bc` | Basic Local CLI/MCP owner routing and bounded read-only host | Own no-run compile passed in 13m 24s; new tests/check/Clippy NOT executed |
| `305df8f11` | Registers processing contracts in protocol exporter | SOURCE ONLY: regenerate protocol and Native bindings, then drift/type checks |

## Invariants and scope

Fully implement, test and validate [the entire plan](./frontend-backend-separation.md).
Local remains the default standalone, offline, login-free experience, including
one-person/same-device use. No account, SSO or server setup belongs in Local
onboarding. Explicit Remote never falls back to a Local vault.

Use one shared Rust application layer and thin IPC/HTTPS adapters. Frontend
requests/subscriptions use the scoped client abstractions. Keep file I/O,
atomicity, canonical revision checks and index maintenance in Rust. Vault files
remain canonical; preserve the current format, exact attachment ownership and
unknown/dot-file exclusion. Preserve macOS and Windows 11 x64/ARM64. Linux work is
headless-server only. Exclude bases, CRDTs, mobile, real-time collaboration and
general remote-vault offline editing.

Implement on an isolated worktree. All build/cache/temp/model/test/log artifacts
must physically reside on T7, including both Cargo targets and native module
caches. Inspect the destination workstation and AGENTS.md before choosing paths.
Use one stable Native runner path; do not alternate absolute worktree paths or
build several Native graphs concurrently. Keep source frozen during each build.
Do not cancel unrelated or hosted builds. The local interruptions in this handoff
were explicitly authorized by the user's pause request.

## Transfer-time behavior and limitations (historical)

The default desktop acquires its canonical vault lease before settings migration.
Context, Store and settings retain the original descriptor through actual I/O.
Remote startup opens no Local vault and separates credentials by saved profile.
The full main remote workspace remains gated; the experimental notes workspace
is the active Remote experience. Unsupported Remote actions cannot load Local
credentials/models or write a Local vault.

Device capture is independent of webview/authentication lifetime. A pending
recording can start before sign-in using an already-saved member/vault destination;
this queues raw audio, not a general editable offline note. Upload and publication
remain authorized against the original destination. Signed-out recovery can
export retained WAV audio without clobbering a destination, or explicitly prepare
a stopped, timed prefix for later upload. Original PCM, checksum chain and identity
are retained by default; automatic device cleanup is disabled. Explicit **Free
device audio** is available only for the original connected, editable Remote
scope. Native persists the original cleanup UUID before contacting the server;
Core verifies the original publication and actual child/source drains, and the
SDK independently streams the canonical bytes before device retirement. The
original profile generation, authenticated client, recording lease and exclusive
source admission remain owned through exact private-file unlinks. A later
intentional server replacement is permitted: the lease pins the verified file
generation, not its pathname forever. Private spool files must not be externally
edited; final identity/stamp checks establish unchanged ownership, not a second
hash CAS. Declared cleanup keeps intact raw audio exportable; interrupted removal
requires explicit same-UUID verification, and completed receipts cannot reopen
raw authority. Startup never removes source files. Native/Windows acceptance and
full source validation still gate completion of this retention slice.

Shared checked session read/delete/restore is implemented across Core, Store,
IPC, HTTPS, SDK and UI. **Local shared lifecycle activation remains default OFF.**
Retain that flag until the new producer/persistence safeguards and complete
Native/Windows acceptance have passed. Existing Local UI behavior remains
available; there is no Remote-to-Local fallback.

Local work uses a caller-stable UUID. Core retains a shared session lifecycle
admission until BOTH actual native completion and all original frontend writes
are acknowledged. Unknown starts, failed writes and unproven custom callbacks
retain protection. Late acknowledgements cannot release newer generations.
Tracked Local capture blocks another Local or Device capture until an exact
supervisor-ActorId resource proof arrives; missing proof after supervisor exit
remains protected. Device's earlier hardware-release/file-drain behavior is kept.
Pending batch preflight releases only before producer dispatch; actual native
batch work retains the original runtime/audio ownership chain.

Managed Local CLI lifecycle operations now require a registered Native host proof,
bound to the exact Context and original work registry. Strict actor admission
rejects a missing/unresponsive actor, active or retiring capture, and pending
finalizers. The proof holds capture and batch maintenance locks through Store's
actual tree inspection and trash move, blocking new capture/batch starts. A batch
runtime retains its lock after its visible registry entry disappears; an actor
that exits with unconfirmed resources makes its shared gate unavailable until
process restart. Standalone CLI owners explicitly register against their original
vault lease. Local UI activation is still off; this source checkpoint requires
integrated Native and Windows validation.

Store append publication now has a separate retained admission source and compound
recording/transcript intent. Identity binds server, vault, member, capture, request
and admission. Identical retries retain the original recording selection and raw
transcript presence/bytes; they cannot adopt a newer canonical source. A combined
16 kHz float WAV is checked for layout, frame count, finite samples and checksum,
then migrated from the exact original canonical or legacy recording name. Unknown
files and metadata are preserved. Interrupted commits fence that session's audio
and transcript reads/writes; note and title edits remain available. Recovery uses
retained artifacts and performs no inference. Completed receipts are historical
and cannot recreate deleted content. Multiple simultaneous owned recording files
require review; originals and unfinished stages remain retained for later verified
retention. The new Store regression suite is source-only until the coordinated
Cargo lane runs it; platform durability remains bounded by the existing filesystem
helpers (directory synchronization is Unix-only).

The server's explicit processing intent is durable and member-private. It uses
existing live, batch and summary jobs; first admission pins the operator model.
Existing unsealed captures can feed live processing, Pending captures wait for
binding/publication then batch. Repair requires verified incomplete coverage.
Summary follows exact source guards; verified Empty may summarize saved notes
only if explicitly declared. Changed/unknown results are retained without paid
replay. Private child-origin markers prevent missing-parent or older-reader work
from being mistaken for ordinary manual AI jobs. Server work owns shutdown/drain.

**The device does not yet persist/submit this processing declaration or expose
its live processing/captions UI.** The new SDK adapters are a source checkpoint,
not an executed end-to-end feature. Actual snapshot field is `model: String`.

The basic CLI now resolves the owner before independent access; a busy vault
without a usable endpoint never falls back. Idle MCP holds no lease. Per-ID reads
have a restricted original-lease Context with recovered readers and no global
session enumeration/config migration/watch/search startup. Doctor and first Init
preserve their old semantics. This branch is an intermediate CLI migration:
**import/transcribe remain unsupported until checked MP3 publication and exact
transcript replacement are implemented. Native owner endpoint composition is
also unfinished. Do not ship this checkpoint as CLI parity.**

## Validation: completed versus pending

The last full combined frontend run passed **2,563 tests across 279 files**,
whole desktop typechecking and locale extraction/compilation. This covers the
integrated lifecycle and persistence UI with actual Native/plugin bindings before
the newer SDK enum changes. The next generated bindings still need regeneration
and another typecheck/drift check.

Earlier retained-audio/lifecycle validation passed 104 unique Rust cases,
actual Native/protocol export and CPU-enabled all-target checks/library Clippy.
The Local work cohort passed 17 focused cases plus two actual exporter cases.
Subsequent ten-package all-target check passed in 102.39s; all-target Clippy then
stopped on two existing `from_mode(0)` deny lints. `09c3906b6` corrects them to
`0o0`; the final Clippy rerun was not completed before pausing.

C's pipeline source passed 95 focused/default regression cases plus ONE actual
configured20-second CPU scenario. First finalized caption arrived2231ms before seal;
canonical audio/transcript hashes, no-repair outcome and unknown files were read
back. Default/feature checks and Clippy passed. This is not physical device,
two-hour recording or fresh Windows acceptance.

Root's final combined pre-SDK/pre-CLI source (`e7330ef84`) compiled in 3m 05s.
**46 test cases were observed passing**, including the extended physical-admission
regression, Core pipeline/origin tests, HTTP processing cases and raw-path fixture.
The process group was then interrupted with SIGINT at the user's explicit pause;
there is NO complete-suite/check/Clippy/export pass for this run. The evidence
file records the exact observed cases. An intentionally injected observer panic
was caught by its passing regression; it is not an unhandled production panic.

B's exact22-file CLI source (`6c85c593`, integrated as `4c819d8bc`) completed a
five-package no-run compile with fresh test executables. No new CLI, transport or
Core test suites, production check or Clippy executed for that checkpoint. A's
four SDK tests also remain unexecuted. The FINAL integrated branch has not been
compiled or tested as a whole.

Hosted CI on old published `ade7bb6e5`: backend, CLI, general/frontend and zizmor
passed; desktop Rust and both Windows architectures failed fixtures. The macOS
`/var` alias, Windows verbatim-path normalization, and locked-file truncation
fixtures were diagnosed and corrected above. Fresh final-head CI is required;
do not infer final Windows acceptance from an old macOS pass or no-run compile.

## First continuation work

1. Fetch the draft branch into a clean isolated worktree on the destination
   machine. Read current AGENTS.md and run-tests/axum-test-transport skills.
   Verify both physical target/cache locations on T7 before a Rust build.
2. Inspect final-head hosted CI and pending source checkpoints before editing.
   Compile/run the new SDK TLS cases and basic CLI owner/MCP cases. No new model
   or paid-provider call is needed for those cases. Correct actual integration
   errors without weakening source/ownership/cancellation guards.
3. Regenerate protocol and Native bindings using their real exporters. Their
   source registrations exist; generated files intentionally have not been
   hand-edited for the latest SDK enum. Native/plugin exporters resolve from
   CARGO_MANIFEST_DIR; protocol exporter owns its formatting. Oxfmt Native/plugin
   `.gen.ts` directly because dprint excludes those generated files.
4. Complete the combined native/all-target check, library or all-target Clippy as
   appropriate, focused regression cohort, frontend/type/locale drift checks and
   fresh native Windows x64/ARM64 CI. Both workflows explicitly select
   `loofah-local-owner`. Use nonzero test inventories and actual readback.
5. Wire explicit durable device processing declarations and SDK triggers into
   Native capture/upload BEFORE offline capture starts. Bind full identity,
   request UUID, fingerprint, original options and producer ownership. Historical
   captures without a declaration must not start inference. Preserve raw capture
   and uploads if optional processing is unavailable. Compare Pending results
   against authoritative ingest/publication binding; do not duplicate Core's
   derived session-ID business logic in the SDK. Add status/cancel/caption delivery
   through the shared client and scoped subscriptions.
6. Finish CLI existing behavior: Local-only injected normalizer/transcriber over
   existing engines, checked absent-only MP3 publication and recording-SHA plus
   raw-transcript-revision replacement with retained result/recovery. Do not borrow
   classic WAV's4 GiB cap for MP3, overwrite speaker edits, replay unknown paid
   work or route unchecked legacy writers. Restore all existing flags/output/
   error behavior and run the complete import/STT acceptance cases. Audit trusted
   Local 4096-byte/64-tag metadata and 8192-query/10000-offset limits so HTTPS budgets
   do not silently narrow previous Local CLI behavior. Compose the Native owner
   endpoint over the EXISTING managed Context/lease; no second Runtime/watcher.

Rust 1.94's libtest accepts multiple OR filters after `--`. Batch them in ONE stable
package/feature invocation to avoid repeated startup of nine zero-test binaries.
Use Cargo's environment for Native tests: earlier direct Mach-O launches differed
from Cargo for two Codex probes. Do not skip those probes to claim success. The
old machine caches and pinned executables are historical evidence, not portable
requirements. `--offline` is optional on a new machine without downloaded deps;
keep `--locked` and use normal approved dependency downloads as needed.

## Remaining full-plan gates after those steps

- Owner-managed Remote provider secrets, settings/model policy and remaining
  main frontend AI parity. C's next credential work was ANALYSIS ONLY: per-vault
  private store outside canonical content, original lease through owned IO,
  owner-only write-only set/clear and status with opaque CAS; permitted aliases/
  endpoints only, redacted inputs/errors; vault override over operator ENV, clear
  falls back to ENV; bind overrides to normalized operator endpoint fingerprints.
  Resolve keys for fresh admissions while admitted work retains its original key.
  Local Keychain and ChatGPT remain unchanged. No source was written for this.
- Main remote workspace activation and remaining content/import/export/device,
  tray/floating-recording and platform behavior.
- Recording append, safe retention/verified raw cleanup, backups/restore/upgrades
  and client/server compatibility.
- Realistic-vault production macOS responsiveness while recording/AI runs,
  sustained physical capture and network/restart/disconnect acceptance, and fresh
  native Windows x64/ARM64 installer/runtime checks. Linux headless/CPU/container
  qualification remains separate from Linux desktop, which is out of scope.

## Managed Local transcription source checkpoint

The Native Core transcription host uses the existing on-device Soniqo and Whisper
model aliases, language policy, prepared-audio kernel, channel mapping and speaker
detection. It never starts or replaces the legacy local HTTP STT server. Whisper
loads retain an explicit verified model descriptor and preserve the compiled
Metal/Vulkan policy; server CPU resource loading remains unchanged. New jobs keep
the shared absent-only transcript contract and the Local zero-UUID self channel.

Strict actor admission rejects capture, retirement, unknown actor state and legacy
batch work. Starting live capture cancels admitted background inference without
waiting for it; native work retains its source, original vault context and model
reservation until actual completion. Repeated background jobs cannot acquire that
reservation during cancellation or model release. App shutdown drains the Core
worker and remaining native reservations. Missing models fail offline; the host
does not download models or consult provider credentials.

This is a source checkpoint pending the root-owned Native/Windows validation lane,
not evidence of real model or physical recording acceptance. Explicit replacement
of an existing transcript still uses the guarded legacy workflow; the absent-only
job contract has not been weakened.

## Optional real CPU fixture on a new workstation

The audio fixture is tracked at `crates/data/src/english_1/audio.wav`. The20-second test
converts it to dual-channel f32. Model is external, not in Git:
`ggml-base-q8_0.bin `,81,768,585 bytes, SHA256
`c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9`.
Use the checked-in model download/hash procedure in `backend_ci.yaml`, with model,
cache and temporary paths on T7. Then, using the destination's model path:

```sh
LOOFAH_WHISPER_MODEL=/absolute/T7/model/ggml-base-q8_0.bin \
  cargo test --locked -p loofah-server --features cpu-transcription --lib -- \
  tests::transcription_host::capture_live::real_configured_cpu_live_captions_precede_seal_and_promote_matching_capture \
  --exact --ignored --nocapture --test-threads=1
```

Linux uses Ubuntu 24.04/GCC 14 and the checked-in
`.github/actions/cli_native_setup/linux.sh`; backend CI provides CPU/ONNX setup.
Do not assume macOS Metal/ONNX binaries or model caches are portable. Do not
repeat this inference merely because unrelated adapter code changed.

## Old workstation state only

- Integration: `/Volumes/T7/src/loofah-backend-separation`, branch
  `refactor/frontend-backend-separation`.
- Native runner: `/Volumes/T7/src/loofah-separation-desktop-bootstrap`, source
  `e7330ef84` plus exporter registration. Earlier source is preserved at branch
  `refactor/backend-native-local-work-before-pipeline` (`345097ae1`). Final local
  run 1595/process group 46588 exited 130 for the explicit pause; no process remains.
- A: `/Volumes/T7/src/loofah-separation-document-editor-ui`, source `c10ba0bf9`.
  Owned source committed. Only three old Root-generated validation inputs remain
  dirty; they are already represented by Root's proper generated/source state.
  Do not copy them over newer exports.
- B: `/Volumes/T7/src/loofah-separation-batch-transcription`, source `6c85c593`,
  clean. Build 36382 finished 0; sample probe was stopped. No process remains.
- C: `/Volumes/T7/src/loofah-separation-transcription-cpu`, source `dfb827577`,
  clean. No process remains.
- Logs/models/artifacts: `/Volumes/T7/loofah-backend-build/` and
  `/Volumes/T7/loofah-search-build/`. The evidence JSON copies the important proof
  records into Git. Old process handles and absolute executable paths are not
  usable on the destination workstation. No stash or uncommitted owned source is
  needed to resume.

Append producer admission now has an SDK process-local authority bound to the
original profile generation, authenticated member and login generation. Native
holds its short dispatch guard through producer admission; profile/login ABA,
disconnect and window cancellation invalidate it. Bounded append-preparation
inventory restores original durable requests after restart without dispatching
HTTP or minting replacement requests; malformed or future identity evidence
requires review. These SDK changes await the shared integration validation lane.

Native remote recording append uses the SDK's durable preparation inventory and original account/profile dispatch capability. Existing-recording UI calls the explicit append command only for the original connected editable scope with `capture_append`; pending new recordings remain available offline. Lost preparation replies reuse every saved UUID and processing choice, including after process restart. An unresolved same-session declaration blocks a fresh baseline. Native takes the authoritative Mixed mono or mic/system stereo format from the saved admission, persists that complete target before producer dispatch, and deterministically downmixes native stereo into Mixed mono without changing 16 kHz frame counts. Profile/token fences cover actor RPC dispatch and release before its acknowledgement. Raw-only append retains the existing transcript as an incomplete prefix; explicit processing includes the suffix through the shared coordinator. Server capability advertisement and Native/Windows acceptance remain gated on integrated validation.

Manual transcription submissions now have their own bounded device receipt store and typed Native prepare/list/get/observe/forget commands. Preparation persists one immutable request UUID, account/vault scope and complete Absent-only transcription input before network submission; an unresolved, active or uncertain request for the same session blocks a fresh UUID. Read/list never dispatch work. Explicit recovery reconfirms the original saved receipt, keeps monotonic observed job identity/state, and never grants transcript replacement authority. Source tests cover process death, failed directory sync, scope changes, changed model/hash/options, corrupt/future records and cancelled I/O waiters; integrated Native/SDK validation remains required.
Local native capture now declares a Core-owned durable intent before producer
admission. The original session, append state, selected STT options, summary
revision and separate child owners (zero UUID transcription, `desktop` summary)
are immutable. Native physical drain and canonical persistence are independent
trusted callbacks; neither renderer acknowledgement nor terminal UI state grants
post-processing authority. Source fingerprints are acquired only after both
callbacks succeed. Restarted incomplete producers require review.

Automatic Local repair uses the existing owned native transcription engine with a
checked replacement baseline, private original audio, a durable dispatch marker
and retained bounded result. Unknown dispatch without retained output is never
replayed. Explicit `LocalCaptureIntents::recover` reconciles only the original
intent/output; a changed canonical baseline remains a conflict. Automatic summary
requires five canonical words and 160 whitespace-normalized Unicode characters.
Its durable child keeps the original recording/transcript source binding and
summary target revision through generation and apply; fresh captures use
`only_if_empty`. Manual note-only summaries retain their existing API. This Core
slice awaits Native adapter integration and shared-lane validation.
Local capture now declares its automatic-processing intent in the managed Rust
owner before starting the native producer. The tracked actor receives a
per-request runtime pinned to that vault; ordered live deltas, transcript
finalization, recording timestamps and final audio cataloging run without a
renderer acknowledgment. A bounded persistence queue fails closed and stops only
the original request if it cannot keep up. Unknown native completion or failed
canonical persistence leaves a durable review state and does not admit inference.
The old global timestamp listener and recording-specific TypeScript persistence,
batch and summary callbacks are removed. Native exit must drain this owner before
shutting down its transcription and summary workers. Focused scope handoff tests
pass; native physical-drain/persistence tests require the coordinated native build
lane, including Windows qualification.

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

The latest broad seventeen-package macOS library cohort passed 2,135 cases,
with one Local actor fixture failure and 26 explicit ignored cases. Its sole
failure came from two parallel fixtures sharing production singleton actor names;
fixture isolation and actual child-task retirement corrections are integrated and
await the next complete run. Earlier broad findings also corrected positive
reopen tests for transient CLOEXEC lock inheritance during concurrent fork/exec,
strict HTTPS retention fixtures and a live-worker lifecycle assertion. Negative
ownership tests and actual ownership pins remain unchanged.

Current shared frontend integration passes 2,565 tests across 283 files, including
explicit immutable manual transcription recovery and recording-parent status.
The local storage-volume fixture passed 7,200 stereo chunks, 115,200,000 frames
and 921,600,000 bytes with exact reopening/seal/hash and a 750-byte maximum
manifest. This is four minutes of storage-volume work, not two-hour wall-clock
capture qualification. A separate paced, real CPU SDK/server soak is in progress.
Four real Linux ARM64 executable maintenance tests passed against frozen source
`7b50d486b9` plus their committed fixtures and a lock-only dependency overlay;
they cover split secrets, unknown files, pending capture publication, rollback,
legacy recovery and future-state refusal. They do not qualify later Native changes
or all historical binaries.

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
batch and summary callbacks are removed. Native exit requests this owner to stop and joins its exact physical/persistence
drain before Store flush and final transcription/summary worker shutdown. Focused scope handoff tests
pass; native physical-drain/persistence tests require the coordinated native build
lane, including Windows qualification.

The Local client also exposes initialized recording-parent inventory and explicit
recovery. These IPC calls require the already managed capture service and its
pointer-equal original vault Context; querying them never opens a vault, starts a
worker or dispatches inference. Remote clients do not expose or probe this API.
Recovery uses the original request UUID and Core's retained-result policy; it does
not create a new request or retry automatically after an uncertain outcome.

Windows validation exposed a temporary-owner delivery race between concurrent
first CLI searches: the command that started the owner can finish while another
reader is receiving its result. Owner resolution now retries transient handshake
loss while rechecking the original OS vault lease. CLI reads make at most three
attempts after a disconnected transport, resolving ownership afresh each time.
The request identity is retained; application/protocol errors and every uncertain
mutation remain non-replayed. Deterministic shutdown tests cover the read fallback,
unknown-write preservation and the handshake/lease-release boundary; both Windows
architectures still require their native CI rerun.
The checked Local media adapter reuses the existing CLI media journal and Store
publication transactions. Manual retranscription can bind the exact recording,
raw transcript presence/revision, channel layout and session creation identity
before confirmation; changed input is rejected before model admission. Media
receipts freeze only transcription model preferences, excluding unrelated settings
and credentials. Read-only media inventory does not resume work. A Core bridge
uses the configured owned Native transcription engine without submitting an
absent-only job or granting it replacement authority. Native/UI migration and
manual automatic-summary integration remain in progress; these source changes do
not yet remove the remaining legacy manual import path.

Local subtitle imports now have dedicated `ImportSubtitles` / `RecoverSubtitles` operations. Core retains the original SRT/VTT source and parsed result under the original request UUID, requires exclusive lifecycle and the Native physical/persistence admission proof, and publishes through a new explicit Store transcript-only receipt mode. Note-only sessions need no audio/model; append preserves unknown existing transcript JSON fields and uses raw presence/revision plus session-created-at CAS. Completed receipts report history without reseeding canonical data. Inputs are bounded to 32 MiB / 250,000 cues and retained parsed output to 64 MiB. Existing audio-bound publication receipts cannot acquire transcript-only authority. Source-only regressions cover parsing, original-request recovery, physical guards, CAS, and crash boundaries; integration validation remains in the sole shared build lane.
Manual Local media summaries use a separate pre-dispatch parent declaration, never fabricated capture completion. The original media request, child request UUID, automatic preference, summary target revision and title policy are durable before import/transcription. Core completion verifies the original media journal and completed Store transcript receipt; only that exact canonical source can admit a shared summary job. Subtitle children use a transcript-only raw revision/session-generation guard; audio children also bind the recording SHA. The same five-word/160-character automatic eligibility rule applies. Explicit retry returns the original child status and never resumes a historical generation; shared job schema 5 adds an explicit manual-media origin while legacy/origin-free serialized bytes remain unchanged. Native adapter wiring and shared-lane compilation/tests are pending.

The Native manual-media service now owns Local imports and checked retranscription
through the same Core CLI media journal and initialized device engine. It retains
verified path/browser inputs, freezes the manual summary parent before dispatch,
and tracks the actual worker independently of the invoking webview. Cancellation
signals the original worker and remains running until it drains; explicit recovery
publishes only retained output and never replays a model. Imported raw audio remains
available when transcription fails. Native inventory is read-only, uses the original
Context pointer, and is absent when no Local owner was initialized. Managed CLI
media now receives that same Native host; standalone CLI media remains independent.
The adapter and regression sources still require the coordinated Native build;
legacy manual frontend callers are not yet removed in this checkpoint.

All Native exit entry points now route initialized Local capture, manual media and
transcription owners through their drain path even when the CLI endpoint failed
to bind. Manual media cancellation retains actual source/model tasks; Store flush
and final worker shutdown follow those owned drains. This addition requires the
combined Native/platform validation lane.
Manual frontend audio/subtitle imports and retranscription now consume optional
`VaultClient.media`. The old batch hook no longer creates or appends canonical
transcripts, writes date metadata, or starts summary work. Regeneration captures
its exact source before speaker-reset confirmation. Queue entries retain the
actual VaultScope and original UUID; explicit retry recovers that request rather
than importing again. Browser files use bounded device staging, and a profile
replacement cannot retarget queued or delayed work. Local capture/live rendering
keeps its separate capability gate. Eight focused frontend suites (86 tests) pass;
full typechecking awaits current generated Native exports in the integration lane.

Manual media keeps a completed, validated engine result separately from its
publication permission. Cancellation or access loss during final drain retains
that output before refusing canonical publication; the immutable source directory
stays pinned through final digest validation. Read-only result IPC returns the
original retained transcript (including conflicted output) without adopting current
source state or retrying a model. ASR result reads are bounded to 64 MiB and verify
receipt identity/digest; subtitle reads use the existing bounded parsed receipt.
Source regressions cover final source ownership, cancellation/revocation, and
historical result reads after canonical deletion. Shared Native validation remains
required for this follow-up.
Remote CLI creation now has an additive `session_initial_metadata` capability and checked `/vaults/{vault}/session-creations` request. It delegates initial note and metadata publication to the existing Core rich-creation receipt, preserving the older title/markdown DTO and serialized receipts. The separate route does not shadow a valid session ID. Unknown fields, metadata budgets, reader/private-vault access, same-request replay and changed-request refusal have source regressions; shared-lane validation is pending. Remote CLI mutation command wiring remains in progress.

Remote CLI mutation source now uses a command-scoped SDK gateway for checked creation (including initial metadata), rename, note set/append, tag changes/listing, soft-delete, attachment upload, and complete exports. The original profile generation/member/vault and profile lease remain pinned through dispatch and owned file transfers; unknown writes are not automatically retried. Local schema-2 output is unchanged. Remote paths are null and deletion returns its opaque trash token/revision. PDF attachments are downloaded through verified SDK leases before rendering. MCP remains read-only; Remote import/transcription and Local filesystem/recovery commands remain explicitly unavailable. Source formatting and diff checks completed; Cargo and platform acceptance remain on the sole integration build path.

The shared client exposes optional media operations only when its initialized
Native media owner is available. Source snapshots, staged imports, explicit
recovery/cancellation and retained results keep the original scope and request
UUID. The recording-parent and transcription-job panels display durable status
without automatic inference replay; copied retained transcripts do not mutate
changed canonical content. Generated Native/protocol contracts and the complete
manual-media frontend migration require the integration build/checks.
Manual-media summary completion refusals now retain an actionable `reason` in the original parent status, including source/target conflicts and unavailable processing. The parent preserves its existing source/child state because a lost receipt may already have admitted owned child work; recording a refusal never revokes that authority or starts another inference. Read-only status exposes the retained reason after restart, while successful original-child replay clears it. Source-only regression extends the frozen-target conflict case to retained status and repeated exact-request refusal.
### Scoped desktop session export

Native `application_export_session` and `remote_export_session` use the original
Local context or authenticated Shared connection and the existing portable export
renderer for PDF, Markdown, text, and Org. The selected summary/template document,
note, and rendered transcript are frozen before rendering. PDF images resolve only
explicit `attachments/<encoded basename>` references through scoped Core/SDK media
leases; no vault directory enumeration or external image fetch occurs. Private
copies are hash-verified, bounded to 32 images, 32 MiB each and 64 MiB total. Text is
bounded to 8 MiB and the resulting file to 128 MiB.

Shared publication refreshes authorization for the original session and holds the
original profile and login generation only across the final filesystem commit.
A selected Shared vault may differ from the profile's saved default vault. Account,
profile, window, cancellation, or shutdown changes cannot select a replacement
source. Downloads/Loofah is checked for symlinks and directory replacement; a
same-directory temporary file is synced and published without replacing an existing
filename. The original snapshots and private image copies remain owned until real
blocking render/write work finishes. `SessionExportState::request_shutdown` and
`drain` must be included in Native exit before dependent context teardown. This
source checkpoint still requires the parent's actual Native build, exporter,
frontend integration, and platform validation; it is not Windows qualification.

Remote media SDK and Native bridge source now bind immutable declaration receipts to the original profile ID, HTTPS server, member, and vault before mutation HTTP. Existing-session audio/subtitle imports carry explicit metadata/raw-transcript/recording baselines; source bytes use a separate SHA/size checked PUT and the original opaque admission revision is persisted first. Ordinary duplicate submission only inspects status. Explicit recovery can re-declare the same saved input after authoritative NotFound; explicit source retry requires caller-selected bytes matching the original descriptor. Source readers keep original profile/client/device ownership until actual I/O drains; short account locks cover owned dispatch without blocking unrelated requests during large uploads. Login ABA, changed profile, lost replies, and receipt immutability have focused SDK regression source. NativeRemoteConnection exposes the same scoped query/submit/cancel/recover/result controls for the generic Remote media adapter; Core/server capability and summary scoping remain part of the coordinated implementation. No Cargo/platform validation was run in worker worktrees.

The shared desktop export dialog now calls the scoped Native export owner for
Local and Remote PDF/text/Markdown/Org exports, including the selected summary or
template document. Its five focused UI cases pass, including cancellation and
late-output refusal after closing. Native export shutdown/drain is included in
all exit paths. The manual-media status panel exposes original-request recovery,
cancellation and retained-output copying; six UI cases pass, including access
loss and navigation while a result is in flight.

The Remote media facade now consumes the immutable Native/SDK declaration and
source-upload operations. Receiving and Ready remain pending, with the original
reserved session ID. An explicit upload retry reuses the original request and
checks the original selected bytes; ordinary repeated submission only reads
status and never adopts fresh settings or baselines. Remote model/summary
choices come from server policy/settings. Unavailable transcription preserves
raw-audio import. Core/server integration and fresh combined/platform acceptance
remain pending for this facade.

The workstation's home Rust installation was discovered to contain individual
files resolving to the historical external drive. A fresh official Rust 1.94.0
installation now lives under the local cache's `rustup/`; `env.sh` sets both
`RUSTUP_HOME` and `CARGO_HOME`. Helper binaries, package links, model/cache roots,
Apple tools and their loaded libraries were checked for external-drive links.
The new local toolchain passed the focused 20-package lane: five SDK media cases
(including actual cancelled blocking-source drain), three Remote CLI gateway
cases and actual Native/plugin exporters. The earlier full lane had 2,279 passing
tests, 26 ignored tests and three failing CLI HTTP fixtures; those three now pass
with the fixture correction. This does not substitute for a new full integrated
run after the pending Remote Core/server and CLI media source lands.

Remote manual-media summary parents now use a trusted server member principal and the same member-private media namespace as their original publication receipts. Local parents omit the new optional scope field and retain `desktop` ownership and prior serialized bytes. Summary children freeze that original member and source; parent/status/result lookup cannot adopt another member's or Local receipt with the same request UUID. A focused shared-session regression covers two members plus Local declarations, original-member-only inference/child access, and historical replay after canonical transcript deletion. Root/server integration must call the crate-only scoped declaration/completion/status entry points; these are not renderer-selected filesystem paths or principals.

Remote manual media is now advertised with paired `media_jobs` and
`media_initial_metadata` capabilities. Declarations, bounded exact-byte uploads,
member-private status/cancellation/recovery/results and summary parents reuse the
shared journals and publications. Raw-only imports work without a transcription
model. Remote CLI import/transcription, immutable initial metadata, partial-import
output and explicit original-request/source recovery now use these APIs. The
combined Native lane passed 31 focused media/CLI/Core/HTTP/export cases plus the
three capture/append fixture cases; the separate paired-capability case and
20-package all-target check also passed. Linux qualification passed seven Core
media, six scoped/manual summary, three HTTP and one actual configured CPU
import/replacement case. Full integrated and fresh platform acceptance remain
required after subsequent source.

Saved device media declarations now have a collapsed global recovery panel,
including lost declaration replies before any canonical note exists. Expansion
only inspects original receipts/status; recovery and source resumption require an
explicit action with the same UUID. Fifty combined recovery/recording-host/media
UI tests passed, followed by 31 cases including media-only device staging.
Read-only device preparation does not grant attachment publication authority.

A production macOS QA app built successfully using the local Rust toolchain.
Its first launch exposed the intentional unknown-application-root refusal. The
known `io.loofah.separation-qa` identity now gets its own discoverable backend
journal root, and CLI discovery reuses that exact root without writing a
production profile. Nine storage regressions pass. A rebuilt app must still
complete disposable-vault UI, physical recording and large-vault acceptance.

Hosted checkpoint `0d8596eb5` passed formatting/lint, frontend and workflow
security. CLI Clippy's oversized gateway enum is boxed. A macOS append test now
canonicalizes its temporary root before invoking the strict path guard. Two Linux
positive-reopen tests exposed incidental inherited flock descriptors between fork
and exec: the kernel behavior was reproduced, and the strict immediate-release
assertion now runs in an isolated exec child and also verifies the original Arc
is gone. No production unlock was added. Eleven full Linux capture-store runs
passed 52 tests each, including 704 immediate reopen iterations. Fresh hosted
checks are still required for these corrections; ongoing Windows runs have not
been cancelled.
Checked transcript speaker editing now has an explicit `transcript_editor` Remote
capability, SDK operations and Native application commands. The renderer reads a
canonical transcript snapshot and freezes its revision when rename begins;
channel-wide assignments are one atomic checked batch. A stale revision reloads
for review and never automatically resubmits. New speaker names use a checked
people-registry writer that preserves unknown fields and refuses corrupt or
ambiguous registries. Canonical transcript reads do not wait for inference's
publication lock; mutations retain the original lifecycle/physical-work admission
through actual Store work. The Local renderer retains its existing qualification
flag and does not enable this new path yet. Focused frontend verification passed 136 tests including the snapshot-binding regression. Rust/Native/platform qualification for
this source checkpoint remains on the sole integration build path.

Native microphone/system-audio Permissions settings are available in Remote
workspaces without hydrating vault configuration or preloading Local processors.
Recording reminder/floating settings remain gated until their events target the
scoped device capture owner; the legacy notification listener must never be
mounted in a Remote workspace because it creates Local sessions.
The scoped device shell now has source seams for bounded latest-value Native
telemetry and identity-bearing floating Stop events on macOS and Windows.
Telemetry callbacks use a nonblocking bounded cache; UI reads select the complete
original capture identity. Floating views copy their rendered capture key into
the Stop event; legacy Local listeners ignore keyed events. The Remote device
host now owns reminders, auto-stop, sleep-stop and the floating overlay without
mounting the Local listener or creating a Local note. Button, shortcut and tray
starts share the same original destination, frozen options and pending request
IDs; unknown replies require explicit retry of that same request. Recording
settings are available only with the scoped device controls mounted. Device
preference initialization excludes local inference and model/language defaults.
The active-only telemetry loop stops on retirement; delayed Stop events retain
the rendered capture identity. Auto-stop rechecks microphone usage and the
original capture, and cancels after a network interruption. Focused frontend
verification passed 105 tests. Actual generated bindings, Native builds and both
Windows architectures still need integration qualification for this checkpoint.

An opt-in Native acceptance case now covers configured Local Ollama through the
production `ProcessingState` / `LocalModels` / `DeviceSecrets` constructor and
shared Core jobs. It declares the existing automatic manual-media parent before
a checked SRT import, verifies generated summary/title publication, exact original
transcript bytes, and all original note prose beneath the existing generated-title
heading stamp, replays one original child, and verifies that a
short transcript is skipped. No account, credential-store plugin, download or
provider substitute is installed by the test. Set `LOOFAH_TEST_OLLAMA_BASE_URL`
to an explicit loopback `http://.../v1` endpoint and `LOOFAH_TEST_OLLAMA_MODEL` to
an already downloaded model, then select the ignored
`processing::ollama_acceptance::configured_local_ollama_publishes_automatic_summary_and_title_without_login`
case on the existing stable Native test graph. Both successful and failed
assertions drain real summary/title work before releasing the disposable vault.
Native execution has reached actual local summary/title inference; fixture checks
now follow the existing canonical title-heading stamp on summary and notes.
Complete root-lane acceptance is pending and does not replace physical microphone
or platform GUI acceptance.

Windows x64 checkpoint `0d` exposed two append durability bugs: SDK receipt
replay and Core PCM description called `sync_all` on read-only handles, which
Windows rejects. SDK replay now reopens the same verified private receipt with
write access and no truncate/create flags. Core normalization/concatenation now
sync the original owned output descriptor after WAV finalization; inspecting
retained prefix/suffix inputs remains read-only. A new immutable-input regression
covers the latter. Two changed-source fixtures now simulate external atomic
replacement instead of truncating a mandatory-locked Windows source; the Store
case also asserts that the original Windows in-place write remains locked. No
production lease, lock, CAS or durability check was removed. Source formatting
passed; Native focused and fresh x64/ARM64 execution remain pending.

Original Remote media source export now has an SDK streaming verifier and a Native `remote_media_export_source` command. The renderer supplies only its original request UUID and connection/vault routing; the SDK resolves the scoped durable submission and exact server admission, then validates content type, revision, SHA-256, length and unencoded whole-body bytes. A private Downloads/Loofah staging file survives the actual writer drain, and a fresh original profile/member/login fence gates the final no-clobber save. This path does not use the 128 MiB rendered-export buffer and never dispatches inference or rebinds media authority. Focused source regressions cover descriptor/body corruption, login/profile ABA, aborted caller flush ownership, capability checks before credentials, and large-file no-clobber staging. Source formatted; execution and Native command generation remain assigned to the single integration build lane. Server advertisement of `media_source_export` remains gated on its qualification.

Windows ARM checkpoint `0d` exposed CLI main-thread stack overflows across ordinary session commands. The CLI now heap-pins Local owner startup/recovery and Remote SDK dispatch at their existing ownership boundaries, plus the task-local entry and optional Remote media workflows. These stay on the original task with the original cancellation and authority; no thread-stack increase or extra runtime is used. The measured Native entry future fell from 35,152 to 608 bytes, gateway state from 6,960 to 2,584, and Local owner entry from 6,208 to 1,520. A regression measures future types without constructing them, enforcing 4 KiB entry/transport and 16 KiB workflow budgets. Actual generated poll frames and Windows x64/ARM64 subprocess tests remain required: type size alone does not prove runtime stack bounds. The shipped CLI skill now includes read-only server `sessions media` and exact-original-request recovery guidance, fixing the command documentation contract exposed by the full library run.



## Controlled operator acceptance, 2026-11-08

Actual process acceptance used immutable Linux ARM64 server source `c9290ee7d8`
(with its recorded desktop-only lockfile edge) and the CPU runtime-packaged
`43977c22c` server. This is earlier than current root source; both report application
version 0.45.7, so this is a cross-commit compatibility check, not a released-version
matrix. Models were not configured. Synthetic OIDC and a non-inference streaming
provider ran only inside an isolated Docker network namespace.

The actual maintenance commands rejected a live service, missing/mismatched
private bundles, and future catalog formats. Restore into empty storage matched
all 14 original file digests, preserved unknown/dot bytes and empty directories,
and regenerated an external cache. Authenticated readback preserved owner/reader
roles, private credential status and creation replay. A sealed unpublished capture
survived restoration and published exactly once; a receiving media admission
survived another backup without inventing canonical content. Killed-process restart,
old-executable snapshot rollback, interrupted legacy Markdown/task recovery, and
future-runtime refusal were exercised. A dispatched provider job restored as
`interrupted`; same-request replay left the original two provider calls unchanged.
No user data, real credentials, inference, deployment or soak state was used.

Local evidence is under
`~/Library/Caches/loofah-separation/operator-acceptance-141d5c237/`; `proof.json`
records exact binary/image provenance, script hashes and 26 checkpoints. Its digest
is `da77b842a85be801a5f6454c3ff349ef99836782f12c26f81faa9338e234ac28`.
The corresponding `operator-acceptance-*`, `operator-upgrade-recovery-*` and
`operator-interrupted-job-*` logs remain in the task cache logs directory, including
failed harness attempts. No product bug was found in this acceptance slice.

Still separate: current-root regression, hosted x64 CPU packaging, production
OIDC/TLS, released-version compatibility, large-copy interruption, client reconnect
and uninterrupted physical recording. The server still needs content-free
operational metrics and required-vault readiness probes. ONNX live/batch admission
currently shares a bounded single slot and has no explicit live-priority scheduler.

### Two-hour generated capture and corrected large-source qualification (2026-11-08)

The controlled Linux ARM test build completed **7,200.332 seconds of paced capture**
through the production SDK/authenticated server: 7,200 chunks, 115,200,000 stereo
16kHz frames, and 921,600,000 PCM bytes. It survived repeated outages (including
600.099 seconds), a server restart, lost committed chunk/publication replies, and
fresh-process device resume. Maximum producer scheduling delay was 297ms. This
qualifies generated PCM transport, not OS microphone/system capture or Windows.

That first full run **failed final semantic acceptance** and exceeded the intended
publication memory budget. No device raw was retired. A separate streaming C
verifier checked every canonical frame/channel against the committed English2
fixture and deterministic quiet markers. Its canonical SHA-256 was
`9d6cde6f90951a0a9ba5505160633baa599bd413e45db2a657bf942d54f7c7ca`
(921,600,068 WAV bytes), matching the publication receipt. The raw PCM SHA-256 was
`4db95c8be36cd54e029948e6fda7883221f8f2d3ca712dfb1b4071bb0709fd06`.

Two concrete defects were reproduced and corrected. The watcher journal read the
entire canonical WAV before checking whether that path was registered; it now
checks membership first and hashes registered files in bounded buffers with final
file-identity checks. Peak original server RSS was 1,090,112KiB. Separately, batch
Whisper carried previous recognized text across independent VAD ranges, biasing
later speech toward the previous ending. Only dynamic carried text now resets on
per-channel discontinuities; static vocabulary and contiguous context remain, and
live inference behavior is unchanged. The identical retained 360-second prefix
failed before that fix and passed afterward. Corrected real CPU inference over the
entire retained recording passed all 24 speech windows in 126 seconds (observed
peak RSS 342,240KiB).

A separate, explicitly **non-paced** full-size replay on frozen root `53b8ff205`
plus the qualified fixes then verified every canonical PCM frame, the identical
WAV hash, all 24 recognizable speech windows, and real canonical repair. Its fresh
server peak was 379,932KiB, below the 786,432KiB budget. This replay is not a second
two-hour wall-clock run. Its first retirement attempt exposed the proxy's 30-second
and SDK's 35-second request deadlines: actual owned server cleanup continued,
leaving 1,344 of 7,200 server chunks pending when the failed harness stopped;
all device chunks were retained. Retirement alone now has a bounded 300-second
request budget. Ordinary capture calls retain 35 seconds. An actual 35-second
versus 36-second HTTP regression passed in 71.26 seconds.

Explicit resume preserved the same origin, issuer, member, profile, capture,
cleanup UUID `29146854-fb28-4f8d-91e3-8abec07b2ab6`, and completed repair job
`835629ef-fa1d-4d23-8454-f412b5bb3e06`. Fresh SDK canonical verification and both
server/device retirement passed; both durable journals are Retired with zero raw
chunk files. The completed processing snapshot was unchanged, proving no new
inference dispatch. Original failed-run raw evidence remains intact.

The original live path delivered sequences 0–39 exactly once, covering the eight
speech clips through 2,100 seconds. The harness proxy caused periodic SSE
reconnects, and cursor recovery preserved those captions. During the later
600-second network outage, Core intentionally stopped live inference after 120
seconds without admitted audio, marked `repair_required`/`idle`, and waited for
sealed canonical repair. This is the bounded idle-resource policy, not VAD silence
or silent caption loss; uninterrupted live captions across that outage were not
qualified. The composed evidence above preserves these limits and the initial
failures instead of relabeling the original full run as passing.

Logs, immutable binary/source provenance, all-window checks, and retained test
state are under this workstation's local `Library/Caches/loofah-separation`, with
`capture-soak-full-7200-c9290ee7d8-run2`, `whisper-gap-*`,
`capture-memory-replay-*`, and `capture-retirement-existing-resume-c1c0ef7cb`
artifact names. None depend on an external drive or earlier workstation state.

### Integrated desktop and Native validation, local workstation (2026-11-08)

The source-export path is now paired through Core, HTTP, SDK and the Native
streaming saver. The service advertises `media_source_export`; the recovery panel
exposes an explicit Save original file action only with that capability. The
original upload UUID, admission revision, full byte count/digest and fresh original
profile/member/editor authority gate publication into Downloads/Loofah. Six SDK
source-export cases passed in the Native library cohort, including late editor
revocation, delayed headers and actual cancelled-writer drain.

Secondary-output removal now has a gated shared confirmation component and header
wiring. It flushes and protects device drafts, reads the saved output before
confirmation, freezes its raw document revision/session generation and request
UUID, and never refreshes that baseline on invalidation. Unknown replies retain
explicit same-request retry; conflicts require another review. Known noncommitted
refusals recheck device drafts. A changed session/scope retires the confirmation,
and tabs change only after a known deleted outcome. Its Store/API/SDK/Native
adapters and capability activation are still being integrated; the optional
client operation remains absent until those are qualified.

Actual exporters produced protocol, Native and Windows floating-control bindings.
The Windows plugin exporter is selected explicitly in the stable integration
package set (21 packages; same feature graph and absolute Native build path).
Desktop typecheck passed. The complete frontend cohort passed 2,586 cases in 292
files; a subsequent 109-case focused cohort covers the final confirmation guards,
source saver and vault restart changes. Locale extraction/strict compilation and
lint passed (114 warnings, zero errors). Workflow helpers passed 26
cases and zizmor reported no high-severity findings.

The Native 21-library cohort at source `905af7f40f` plus the paired capability
change passed 2,352 tests, failed one stale CLI skill contract, and ignored 32
opt-in/helpers. The documentation contract was corrected and passed on rerun.
All-target checking passed after capture-retirement integration. The six CLI
integration targets passed apart from a stale missing-profile expectation; its
updated profile tests passed separately, preserving isolated Local storage and
checking that Remote failures never create it. The exact current debug executable
also built successfully. These are scoped local results, not fresh Windows proof.

The opt-in real Native Ollama case passed in 8.37 seconds using the separately
downloaded local `qwen2.5:1.5b` model on a cloud-disabled loopback service. It checks
automatic summary/title publication, original child replay, short-input skipping,
exact original transcript bytes and complete note prose beneath the existing title
heading stamp. No Loofah account or external provider is involved.

An optimized macOS QA baseline at source `6e5206b175` used disposable local
configuration and synthetic vaults. GUI note editing, full-text search, a
5,000-session/2,500-transcript startup, a new note, and public English2 audio import
completed with 73 transcript words and an automatic local Ollama summary. PDF text
and both rendered pages were checked. Input-to-accessibility automation bounds
were 478ms for search, 430ms for opening a distant note and 402ms for creating a
note; these include automation overhead and are not frame-latency/p95 claims.
The final build still needs production regression. A real settings bug found there
is fixed: explicit vault switching now relaunches directly instead of consulting
the newly selected vault's onboarding flag and deferring forever.

No microphone or system-audio access was granted. The post-onboarding note-only
QA fixture sets `ONBOARDING=false` without altering OS permission checks; physical
capture waits for the pending confirmation. Evidence is under the local task cache
`qa/production-ui-baseline-6e-evidence.json` and named validation logs. No user vault
was used. No merge, release, version bump or deployment has occurred.

Server CPU admission now has an explicit opt-in reserved live partition within the existing operator total. `live_reserved_groups` defaults to zero; one-group legacy hosts still report live busy during batch work. Batch admission waits in a bounded FIFO host queue before source decoding/inference and honours cancellation or shutdown without dispatch/replay. ONNX accepts only legacy `(total=1,reserved=0)` or explicit `(2,1)`, separates its streaming-model snapshot from the mutable batch/diarizer mutex, and drains every group before destroying models. Whisper keeps its existing native total cap and carries the server permit inside the trusted native source owner; Local Metal/GPU/resource behavior is untouched. Source regressions cover total/partition bounds, FIFO/overflow/cancelled waiters, abandoned caller physical drain, panic admission closure, startup ownership, ONNX two-lane shutdown and strict configuration. Unit/platform and real CPU concurrency qualification are pending the coordinated Linux lane; no default capacity or memory increase is implied.

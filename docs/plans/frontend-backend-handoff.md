# Frontend/backend separation handoff

Desktop app testing on this workstation is **paused at the user's explicit
request** on 2026-11-09. Do not launch QA desktop apps, start physical capture or
resume GUI acceptance until the user gives the signal. Both idle QA apps were
stopped. Source review, implementation, headless service checks and hosted CI may
continue. Production SSO acceptance is separately deferred by the user.

The optimized stable 21-package graph built successfully from clean frozen
`94c0cf6b9005701c09942b36bfe5ec6cf8b1c560` in 481.44 seconds. The isolated
`io.loofah.remote-qa-a` bundle, CLI and server have verified manifests and
immutable local copies. Native retirement UI acceptance did not run: initial
startup encountered a task-only capabilities-proxy failure before sign-in, then
desktop testing was paused. This build is compiler/bundle evidence only; it does
not activate `media_retirement`, `capture_append` or the Local gates. The portable
record preserves exact artifact hashes and the user pause.

Current acceptance status, 2026-11-09: all six PR workflows at `053447421f` are
terminal. Five passed. Each native Windows architecture passed 2,250 executions,
including the 91-case listener library, all four Git helpers, both SDK refresh
regressions and installer/runtime smoke checks. Linux headless passed 1,666 tests,
including all three append HTTP cases. macOS stopped before workspace tests on
an external SpeechCore HTTP 504 in both the original attempt and its single
authorized retry; that retry budget is exhausted. Frontend passed and was not
rerun. The later CI prewarm is not qualified by those runs. Production HTTPS SSO
is explicitly deferred by the user, not passed. Physical capture remains separate
acceptance work, and observed Qwen accuracy and conciseness remain failed. Local
lifecycle, output deletion and transcript activation gates remain default off;
no merge or release is authorized.

This document retains chronological checkpoint history. Later dated evidence
supersedes earlier uses of “current”, “pending” and “analysis-only”; those older
paragraphs are not a fresh backlog. In particular, earlier remaining-gates text
about owner-only provider credentials predates their implemented checked workflow.
Historical failures and their source-scoped corrections remain preserved.

The `2263b47eb7` optimized Remote artifact exposed a cached About commit label:
`tauri-plugin-misc` still embedded `0d8596eb5d` even though its independently
verified build and bundle manifests identified current source. `vergen-gix`
watched the linked worktree's unchanged symbolic HEAD but missed the branch ref
in Git's common directory. A narrow build helper now tracks the real loose ref,
existing packed refs and the bounded refs ancestor needed for packed-to-loose
transitions. Vergen still generates the value; version `0.45.7` is unchanged.
Four standalone Rust helper tests using real temporary Git repositories passed
for linked loose/packed/detached refs and source-archive fallback. The ordinary
`6e7656d1b2` optimized build and fresh-profile GUI check passed: About displayed
that exact commit and version `0.45.7`, without Local statistics. No cache wipe or
QA SHA override was used. The older unsigned profile's blocked Keychain read and
unchanged original state are recorded below; no OS grant or credential deletion
was used to obtain the fresh-profile result.

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

Latest full local stable-21 Native checkpoint: 2,429 library tests passed (34 explicit ignored),
using the actual macOS `/var` alias at test runtime. Separately, the `080764fe20`
empty-fence guard passed 12 focused tests across the same 21 library targets,
including five new regressions, and its all-target check passed in 28.17 seconds. The complete frontend cohort passed
2,622 tests in 298 files, with later timeline checks/typechecking qualified separately; actual Native Ollama preview/summary/title
and replay passed, and all-target checks plus debug binaries passed. Qualified
previews are now advertised with configured workers. The exact dev-only A/B QA
trust allowlist passed its default-build negative test. Checked Local lifecycle,
output deletion and transcript UI activation remain default off pending final
platform/physical acceptance. The corrected, shaped two-hour paced SDK/server run
passed on its frozen source; physical capture and later source remain separate
qualification gates. Actual Native A/B browser sign-in, shared editing, Reader downgrade, child-window
vault/parent-close isolation and live access revocation passed on the frozen
`71ea43cf94` QA artifacts. The refreshed `2263b47eb7` artifact subsequently
qualified the patched native menu and connected-Reader controls, as recorded below.

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

Local shared lifecycle/output deletion activation remains default OFF pending
final Native and Windows acceptance. Recording append, verified retention,
automatic processing, provider credentials, imports/exports and Local owner
routing are integrated; the later checkpoints record their exact qualification.
Stopped-service upgrade and forced large-copy interruption acceptance have passed
on their frozen Linux artifacts. Streamed previews and independent Remote note
windows are under integration. Physical recording, concurrent production UX,
realistic capture network profiles, Local-vault adoption and the final combined
macOS/Windows/Linux matrix remain open. Local stays standalone, offline and
login-free. Do not merge, release or bump versions.

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
capture qualification. The separate corrected paced, real CPU SDK/server soak
passed as recorded below.
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
through actual Store work. The Local factory now exposes the same typed
`transcriptEditor` methods only when
called with `transcriptEditorAvailable: true`; its default remains false, so normal
Local startup keeps the existing renderer and offline, login-free behavior. The
adapter forwards the original transcript revision unchanged and reports late
mutation cancellation as unknown without automatic replay. Native mutation
wrappers require both the original Local lease and configured physical lifecycle
host before entering Core; failed host registration cannot fall through to server
admission. Checked assignment still retains the existing strict proof through
actual Store writes. Read-only transcript snapshots remain available while a
physical producer is busy. Default activation awaits the final Native and Windows
qualification gates. The adapter and existing speaker-renderer cohort passed 74
frontend tests, and desktop typecheck passed; the earlier snapshot-binding cohort
passed 136 tests. The new Native missing-host/busy-host/success regression is
source-ready for the sole integration build path.

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

The Remote home screen now enables recording from the mounted scoped device
controller's actual readiness and imports from the editable connected media
client, instead of the legacy Local feature gate. Unknown starts remain blocked
until explicit original-request recovery, while the existing retry control keeps
its frozen request. Public GitHub issue/PR resource tabs are device-only anonymous
GET views and no longer require Local vault controls; Local onboarding keeps its
existing gate. The 31-case home/captured-workspace/device/resource cohort and
desktop typecheck passed. These are frontend/device changes, not new server
GitHub credentials or a Remote-to-Local fallback.

### Required-vault readiness and private operational status

The server now preserves `/health` as catalog liveness and adds `/ready` for all
active catalog vaults. Readiness is cached, generation-fenced, expires on a
monotonic clock, and returns 503 during startup, failure, shutdown, stale sampling
or catalog write contention. Both container health checks use `/ready`. The
owner-only `/api/v1/vaults/{vault}/operations` endpoint exposes content-free
counters, bounded durable backlog/job observations and filesystem pressure; it
never scans on the request path. Disabled, unavailable and incomplete samples are
explicit. Private media sampling reads bounded admission metadata, never PCM.
Owned attachment workers retain the original metrics scope through completion.
See `apps/server/README.md` for probe and counter semantics.

Controlled Linux ARM64 source qualification used the isolated worker source
through `26c5e8299`, with the final mechanical lint correction `510b02109`:

- Full CPU-feature server library cohort: **153 passed, 5 configured-model tests
  ignored**. No model inference was requested for this package.
- New capture-store/Core operational regressions: **1 + 2 passed**, covering
  nonblocking spool observations, bounded latency/redaction and durable media
  admission sampling. Server tests cover current owner authorization, failed
  runtime readiness, catalog contention, SSE reconnect/resync counters and owned
  streaming-upload observations.
- Strict server all-target Clippy (`--no-deps -- -D warnings`) passed after the
  mechanical correction. The older worker graph still emits the existing Core
  `replay_local_capture` dead-code warning; this is not a server lint failure.
- Logs are retained in the workstation's local cache as
  `server-operational-dfbf44371-first.log`,
  `server-operational-26c5e8299-qualified.log` and
  `server-operational-510b02109-clippy.log`. The middle log includes the passing
  full cohort followed by the three lint findings fixed by the final log.

The final Docker probe edit is declarative source wiring; this package did not
rebuild or deploy an image, run Native tests, or claim large-vault performance
qualification. Frozen capture/model/operator evidence remains intact.

Server CPU admission now has an explicit opt-in reserved live partition within the existing operator total. `live_reserved_groups` defaults to zero; one-group legacy hosts still report live busy during batch work. Batch admission waits in a bounded FIFO host queue before source decoding/inference and honours cancellation or shutdown without dispatch/replay. ONNX accepts only legacy `(total=1,reserved=0)` or explicit `(2,1)`, separates its streaming-model snapshot from the mutable batch/diarizer mutex, and drains every group before destroying models. Whisper keeps its existing native total cap and shares the server permit between the trusted native source and its actual-completion owner, so FIFO waiters cannot wake before the native admission retires, including preparation failures. Local Metal/GPU/resource behavior is untouched. Source regressions cover total/partition bounds, FIFO/overflow/cancelled waiters, abandoned caller physical drain, panic admission closure, startup ownership, ONNX two-lane shutdown and strict configuration. No default capacity or memory increase is implied. Aggregate policy reports ready when physical capacity remains, including the reserved live group during batch work; actual per-mode admission remains authoritative.


The coordinated Linux CPU graph through `d13d0fb06` passed 28 focused host/admission/configuration/policy tests, including deterministic normal-completion and rejected-preparation handoff windows. Real configured ONNX overlap then passed in 17.58 seconds with the existing verified models, two CPUs and a 6 GiB container limit: a nonempty live caption arrived in 3,968 ms while the checked 320-second stereo batch still held its actual mutable model lock. Concurrent health plus authenticated metadata reads completed in 6 ms. Explicit batch cancellation and full native/live drain took 8,072 ms; the recording, metadata, original note and unknown attachment remained unchanged, with no partial canonical transcript. The batch had 5,120,000 frames and SHA-256 `938170e91687b18f8f9c4c289fd579770163ca2df9485b591d2c15578ddd926d`. Evidence is in task-cache logs `cpu-admission-linux-retirement.log`, `cpu-admission-linux-reserved-policy.log` and `cpu-admission-linux-real-reserved-second.log`. The first real run reached overlap but failed only because its health fixture expected HTTP 200 instead of the existing 204 contract; its failure log remains retained. These are server CPU results, not Native microphone or Windows qualification.

Strict CPU-feature server library Clippy (`--lib --no-deps -- -D warnings`) passed on this graph. The older-base all-target run stopped on two existing `capture_soak.rs` nested-if lints, and dependency linting additionally stopped on existing `fs-format` nested-if lints; those runs are retained separately as `cpu-admission-linux-clippy-server.log` and `cpu-admission-linux-clippy.log`. They are not represented as all-target passes. Current-root all-target qualification remains with the integration lane.


Remote media imports now negotiate optional processing independently: notes-only
and CPU-only services do not need summary settings, subtitle/raw imports do not
request unavailable transcription policy, and summaries preserve titles when
generated titles are absent. The focused adapter/deletion frontend cohort passed
64 tests, desktop typecheck passed, and actual Native bindings were regenerated
and formatted. The current 21-package Native graph passed 21 focused export,
operational and Store deletion cases, followed by all three explicit checked
deletion Native/SDK/real-HTTP cases. Local checked deletion remains default off;
these focused passes do not replace the final platform matrix.

Packaged Linux x64 CPU CI now requires and runs the reserved ONNX overlap case from the same hashed release test executable and existing runtime image, with two CPUs and a 6 GiB memory limit. The original configured Whisper, configured ONNX media and ONNX live cancellation cases remain. Artifact inventory checks reject every missing or duplicate ONNX case; scheduler/policy tests also run. Five Python tests and actionlint passed locally; hosted x64 execution remains the actual architecture gate.

Forced backup/restore interruption acceptance completed on 2026-11-09 using immutable Linux ARM64 image `sha256:0f42e74bb02a9f88a1dc42990b93d70c52a0c19e4d26fddceca73ff020e259ed`, source `43977c22c6`, executable SHA-256 `0a0ed95e379b23584f2ba28fce4ff986647da303ed9357df8d7fd01a2421e9f3`. The maintenance implementation is unchanged at current `db4920390b`; intervening catalog changes only add readiness accessors/tests. One 768 MiB non-sparse synthetic unknown attachment was added to a private copy of the earlier stopped-storage fixture. Backup and restore were observed with real partial source prefixes, SIGSTOP-frozen at 21,889,024 and 25,034,752 bytes respectively, then SIGKILL-terminated (exit137, no OOM). Competing backup refused the held original service lock. Partial restore never published its destination, existing canonical storage refused replacement, and all 15 original file hashes plus directory modes remained unchanged. Exact-destination retries produced verified paired bundles and an identical restored inventory. Peak logical storage was 2,462,987,004 bytes, within the 5 GiB budget; only this harness's abandoned staging directories were removed after process retirement and evidence retention.

The external reusable harness and 12 checkpoints are retained under `~/Library/Caches/loofah-separation/operator-copy-interruption-43977c22c-20261109/`, with log `operator-copy-interruption-43977c22c.log`. `proof.json` SHA-256 is `c03920476803b0e2d2307a6b511b6bb90e11b56320ed1512a582a705efbd78cd`; `run.py` SHA-256 is `7c26f10c95a16d52eaad2b495cb36fde0417acbb20b4884566728c7b67e72059`. No new build, image, service deployment, account, model or production data was used. This qualifies those exact controlled artifacts and copy paths, not latest-root execution, Linux x64, arbitrary released-version rollback, live reconnect or uninterrupted physical recording.


Independent Remote SDK views now retain their original account/profile/vault
authority, share transfer capacity, and own their stop/drain lifetime. Short
dispatch fences release profile/login mutexes before response IO. The six matching
Native SDK/Core cases passed, including delayed headers and predispatch ABA;
Native child-window/UI integration remains in progress.

Private bounded processing-preview Core/API source is integrated with the
capability still disabled. The initial current 21-package Native graph passed
three real-TLS SDK preview cases and four Core sidecar/retention cases, plus actual
Native/plugin binding exports. Generated contracts were formatted and desktop
typecheck passed. Timer-flushed partial updates, UI, server network tests and the
full combined preview cohort remain pending. These are focused integration
results; the final source/platform matrix is still required.


The full frontend cohort through the initial preview backend and window SDK
checkpoint passed **2,599 tests in 293 files**. This precedes the standalone
window and preview UI source integration. CLI CI path filters now include the
shared application/client/protocol, owner/media, capture, vault and storage crates,
so later shared-only edits cannot silently skip CLI validation. Formatting and
workflow checks passed, including 26 Python helper tests and the configured
zizmor threshold.


Remote standalone windows now enter through their one-shot claimed workspace
without the normal startup wrapper, login/discovery or Local bootstrap. Their
creator admission begins before async preparation; app exit closes further
admission and waits actual child IO/window creation to drain. The first combined
Native run passed the desktop cohort (185 passed, 2 ignored), then stopped at one
new SDK fixture missing `recording_leases`; this is not a full-suite pass. The
localized actual-entry/caption cohort passed 13 cases and strict locale compilation
passed.

The old optimized QA build's GUI PDF export stalled in the system directory-open
call while staging Downloads output. Rendering had completed, but publication was
not qualified. Modal cancellation restored the note UI; normal Quit still waited
for that physical IO, so only the verified disposable QA process was terminated.
The separate CLI PDF proof remains passed. The dedicated separation QA bundle now
requires an explicit absolute `LOOFAH_QA_DOWNLOADS_ROOT` to keep future GUI exports
in the task artifact tree; ordinary application bundles keep their Downloads
behavior. No new OS access grant was accepted. The final production build still
requires regression.
## Stopped-service Local adoption qualification (2026-11-09)

`loofah-server adopt-vault` now copies a stopped Local vault into an explicitly
provisioned fresh catalog target inside a new, absent complete storage root. It
preserves catalog/member/role bytes, other vaults and runtime receipts, source
canonical/unknown/dot files and supported modes. Original roots are retained;
service/source/target ownership follows the actual blocking copy even after the
caller is abandoned. Exact fresh initialization directories/idle locks are the
only allowed target state; settings, Trash, work/receipt files and unknown empty
runtime directories refuse adoption. The README documents online provisioning,
stop/backup/copy/config switch and reversible rollback without a merge or sync.

Linux ARM64 qualification reused the existing CPU feature graph, source volume,
models/runtime mounts and caches, with no inference or new image. On source
`56225fae7b`, all **13 maintenance library cases** passed, including real Service
provision/init/shutdown/adopt/reopen with original member access. All **5 actual
maintenance CLI integration cases** passed, including adoption without auth or
cache configuration and refusal to replace an existing output. Strict server
all-target `cargo clippy --no-deps -- -D warnings` passed; the graph includes the
separate mechanical capture-soak lint correction `3c10d434d`.

The first and final logs are `vault-adoption-linux-first.log` and
`vault-adoption-linux-final.log` under the workstation cache `logs/` directory.
This is controlled Linux operator qualification, not a Native feature-activation
or Windows qualification claim. No user vault, deployed service or Local
activation flag changed.


The combined standalone-window, read-only preview and opt-in Local transcript
frontend now passes **2,615 tests in 298 files**, plus desktop typecheck and
strict locale compilation. The actual-entry mock needed a TypeScript-only module
namespace correction; its runtime assertion still exercises the real entry
module. Native bindings came from the corrected concrete Wry exporter. Preview
capability activation and checked Local transcript/lifecycle/output controls
remain gated on the pending Native/provider/platform qualification.


The current combined 21-package Native library suite passed **2,419 tests,
34 explicit ignored cases, zero failures** (`native-full-21-window-preview-third.log`).
This includes private previews, original-scope windows/default-vault preferences,
concurrent refresh fences, physical-host transcript guards, stopped Local-vault
adoption, checked deletion, operational readiness and existing Store/Core/CLI
regressions. The corrected concurrent-refresh driver passed its focused case
before the broad run; the first two failing runs remain recorded.

Actual loopback Native Ollama preview/automatic summary/title/replay acceptance
passed in 2.04 seconds (`native-real-ollama-preview-acceptance-second.log`). A valid
short summary in the first attempt exposed an arbitrary 80-byte fixture heuristic;
the fixture now requires the meeting subject and a relevant fact while retaining
all nonempty partial, identity, filtering, canonical preservation and replay
assertions. There was no extra paid work or provider fallback. The 35-case focused
Native preview/window/adoption/guard/export cohort also passed. Preview capability
activation follows this qualification; Local checked lifecycle/output/transcript
UI activation remains default off for final platform/physical acceptance.


Private processing previews are now advertised only with the configured summary
worker capability. The configured/unconfigured Native server pair passed both
cases; the Local facade/preview/scope cohort passed 97 cases and desktop typecheck
passed. The stable 21-package all-target check passed in 34.41 seconds and all selected
debug binaries built in 1 minute 24 seconds. Logs are `native-preview-capability-pair.log`,
`preview-capability-frontend.log`, `native-all-target-window-preview-adoption.log`
and `native-window-preview-adoption-binaries.log`. These supplement the preceding
full Native/frontend/model results; fresh hosted Windows/Linux qualification and
physical production QA remain open.
Native Remote TLS UI qualification may use only the exact development fixture bundle IDs `io.loofah.remote-qa`, `io.loofah.remote-qa-a`, and `io.loofah.remote-qa-b`, with the desktop `dev` feature and an explicit `LOOFAH_QA_ROOT_CA`. The A/B IDs allow independent macOS single-instance sockets and OS credential namespaces; separate `LOOFAH_GLOBAL_BASE` and absolute `LOOFAH_QA_DOWNLOADS_ROOT` directories remain required for disposable runs. This does not enable custom trust in production or authorize OS trust changes. A synthetic numeric-loopback HTTP issuer can exercise the actual browser PKCE callback while the API uses verified fixture TLS; it does not qualify production HTTPS SSO. Build both artifacts serially from the existing Native worktree and shared cache.


The corrected shaped rehearsal passed in 81.01 seconds before launching the
authorized 7,200-second paced run. All three profiles carried successful capture
uploads (6/13/18 chunks), exact PCM/repair/retirement and RSS assertions passed,
and maximum producer lateness was 33ms. The earlier zero-traffic 500ms rehearsal
failure is retained; its coverage assertion was preserved. The long run keeps
the original outages/restarts and adds bounded network shaping without host
network changes. It uses immutable server source 573 and a separately hashed
harness overlay 5be/31ad; current Native production behavior is unchanged except
the subsequently qualified preview capability declaration. Monitor/evidence
paths remain under the local task cache. The subsequent corrected long run passed
as recorded below.

Local startup no longer enumerates unrelated speech-model caches or downloads a missing diarizer when a vault opens. Explicit user speech-model downloads still request the optional diarizer on completion; existing downloaded diarization remains available, and a missing diarizer does not block transcription. This shared initialization rule applies to both macOS and Windows.


## Actual independent Native clients, 2026-11-09

Two serial optimized macOS ARM64 builds from `71ea43cf9455beb536b13dcb22481f17f62a02a8`
used the stable 21-package graph plus the explicit `desktop/dev` QA feature and
`MOCK_AUDIO=0`. Their distinct A/B bundle IDs, global directories, credentials,
Downloads and temporary roots stayed isolated from the user's app and vault.
The immutable server executable came from the same source. Build requests,
Cargo results, source/lock hashes, bundle manifests and runtime evidence are in
`~/Library/Caches/loofah-separation/qa/remote-browser-lab/native-two-client/`.

Both apps completed actual system-browser PKCE with synthetic Alice/Bob accounts.
The API used verified fixture HTTPS; the issuer used the narrowly allowed numeric
loopback HTTP fixture. This qualifies that browser callback flow, not production
HTTPS SSO or OS trust changes. The external issuer's initial restrictive CSP
blocked its cross-origin callback; its corrected policy permits only the exact
validated callback origin. Invalid callbacks, replay and PKCE checks stay strict.
B started while the API was unavailable and showed its saved Remote destination
and connection failure, without falling back to Local.

A created a shared vault and granted the existing synthetic Bob Editor access.
A saved an initial note; B appended a checked edit; a fresh independent A child
read both. The child saved a third paragraph, verified in canonical `notes.md`.
A separate Personal child retained its original scope after the parent switched
to Shared and closed; a further child edit reached only the original Personal
vault. Downgrading Bob to Reader refreshed his workspace and made his note and
new child read-only. Removing Bob replaced the already-open child content with
“This note's workspace is no longer available.” Sign-out returned the parent to
Connect. Sign-out's physical window inventory was not independently qualified.

The run exposed two remaining frontend gaps: the connected Reader's global
pending-recording header stayed enabled, and native File → New Note revealed the
Remote parent without creating a note. The Reader gate now has 38 passing focused
checks, including delayed reminders, unknown-request identity, disconnected raw
capture and Stop. Native menu routing is being corrected separately. Patched GUI,
final-head platform checks, production performance and physical recording remain
open; no microphone or system-audio grant was accepted.

The four hosted test/workflow failures at `71ea43cf94` are isolated fixture issues,
not ignored passes: exact CPU-case selection, eager maintenance recovery timing,
macOS physical temporary roots and Windows export directory ownership. The latter
preserves Windows' safe rename refusal while pinned. Corrected macOS temporary-root
and export cases passed at `37a5b36ab3`; actual Linux maintenance execution and fresh
Windows execution remain pending. CLI/frontend/container checks and both Windows
speech-model architectures passed on the earlier published source. The Native
Windows ARM64 compile is still running; healthy CI has not been cancelled.


The Reader and native-menu corrections are integrated at `2433ca73f6` and
`2d3418d495`. The combined frontend run passed **2,622 tests in 298 files**;
desktop typechecking and formatting passed. Focused menu/Reader/About checks passed
111 cases. The actual Local idle-supervisor regression passed after the explicit
result-type correction at `584233f62f`; selecting its dependency's test root did
not change the resolved normal feature graph. The Linux eager-maintenance recovery
fixture passed one exact case against the existing frozen `573da57617` production
graph plus its test-only overlay. This remains distinct from final-head Linux CI.

Both A/B QA processes later completed ordinary menu Quit with exit code zero.
The post-CSP issuer counted two browser authorizations, two code exchanges, seven
refresh rotations and no rejections; earlier failed fixture metrics remain saved.
The final immutable UI receipt is `ui-acceptance-71ea43cf94-final.json` in the lab
root above, with a portable copy in the checkpoint evidence. A subsequent review
found that `NamedTempFile` cleanup after a Unix destination-directory replacement
could target the replacement namespace. Physical staging/publication/cleanup
ownership and same-basename sentinels are being corrected before patched PDF QA;
the existing positive rename fixture did not establish this cleanup property.

Export staging now retains the physical destination directory on Unix: creation,
no-clobber rename and cleanup use its descriptor, download writers duplicate the
original file, and macOS quarantine metadata uses that file descriptor. A replaced
Downloads/Loofah path causes refusal without deleting a same-named foreign file.
Publication rechecks the visible directory identity before returning a path.
Windows retains its original directory/file pins and no-clobber publication.
Focused sentinels cover cancellation, failed publication, streamed downloads and
replacement after directory admission. These new cases still require the stable
Native graph and fresh Windows checks; no additional inference is involved.

The Windows staged-file wrapper also retains the original file identity. Before
marking a download or publishing, it checks the current non-symlink staging name;
on lost ownership, it disables TempPath cleanup without changing the foreign
entry's bytes or attributes. This is a same-owner/name check, not an adversarial
same-UID race guarantee. Existing directory pins and no-clobber publication remain
in use. A Windows-only regression covers actual staging-name rename refusal or
replacement, both cancellation and publication, and foreign Zone.Identifier
preservation. Both native Windows architectures must execute this new case.


The full default Native 21-package library graph passed **2,426 tests**, with 34
explicit ignored cases and no failures at `0bb62618a`, including the six new
physical export-owner sentinels. The subsequent Windows same-owner/name wrapper
is integrated at `687e42a4b6`; the stable graph's all-target Cargo check and focused
export cases passed. Its platform-specific staging replacement and streamed writer
regressions require the next native Windows x64/ARM64 execution. The generated
Native/plugin bindings were produced by the actual library exporters and directly
formatted; the frontend type surface remains unchanged.

All hosted runs for `71ea43cf94` are now terminal. Native ARM64 had the same single
export directory-rename fixture failure as x64: 2,228 passed, one failed and 47
ignored across 69 test cohorts, with no additional failure or CLI stack overflow.
Its exact log SHA-256 is
`803cb075f02e662638442f886fc15cec4dd62053c1edd498e4dbef6ccbda5c26`.
No healthy CI run was cancelled; fresh checks can now run on the corrected head.


## Optimized Local GUI and current-head qualification, 2026-11-09

The single optimized macOS ARM64 Local QA artifact from frozen `b755769c25` uses
the stable 21-package graph plus explicit `desktop/dev`/custom-protocol, real audio
selection (`MOCK_AUDIO=0`) and `VITE_PROFILE=true`. A bounded task-only reporting
asset exposes numeric diagnostics through existing tracing; production source and
user flows have no new telemetry. The exact executable SHA-256 is
`cda3ecd98c4ee6f695419930655785946dd5bed5d092094c806876a43cc930c3`.
Its isolated synthetic vault initially held 5,001 sessions and 2,501 transcripts.
The index completed 4.264 seconds after process spawn, 1.422 seconds after root
actor startup, with no index errors. Local opened without login. The selected
existing task Whisper model prewarmed on Metal for 16.543 seconds in the background;
no ASR or model download occurred in this cohort. All paths are local, not T7.

Actual note edits persisted through the GUI. The GUI exported saved note, summary
and transcript to a 26,217-byte four-page tagged A4 PDF; text and all four rendered
pages were inspected without clipping. One explicit local Ollama summary request
ran from `01:49:52.312Z` to `01:49:58.111Z` (5.799 seconds). Its partial preview was
visible while search remained interactive. Two overlapping five-second metric
windows had frame p95 18 ms, frame maxima 26/29 ms and input maxima 16/15 ms across
2/9 samples. Neither window lies wholly inside generation; these are individual
window statistics, not a pooled percentile. Shared client queries bypass the old
query instrument, so query latency remains unmeasured. CUA-inclusive search bounds
and their automation overhead are retained separately.

One user admission/job made two HTTP calls. Logs and frozen generation source
strongly identify the deliberate invalid-heading retry, which cancels and drains
the first stream; its rejected text was not retained. Transport only retries an
explicit 429 and does not replay unknown transport outcomes. This is no evidence
of a second user job or inferred historical work. The native title receipt was
`already_named`. The existing verified local FluidAudio model files match their
official revision; reuse is documented separately from fresh downloads.

The packaged CLI listed the desktop-owned vault, created a checked note, and the
running GUI received/opened it. The packaged read-only MCP server negotiated its
protocol, discovered its tool schema, returned the exact note/marker, preserved
its SHA and exited zero. The installed stable `loof` symlink remained unchanged.
The frozen pre-import receipt is under the task lab's
`local-performance/gui-acceptance-b755-pre-import.json`, copied into the portable
checkpoint evidence. CPU import and physical capture remained unqualified in that pre-import receipt.

After the corrected soak had drained, the same optimized artifact imported the
30-second English fixture through the Local GUI. Canonical audio, a 73-word
transcript and its automatic summary were saved. Native Whisper used Metal; the
Native diarizer completed eight temporal segments and the saved provider hints
assigned all 73 words to one speaker cluster. This proves diarizer execution, not
multi-speaker accuracy. No additional inference was run to investigate that
observation. The import's status panel exposed a separate defect: the standalone
principal is intentionally the legacy nil UUID, but the transcription job ledger
rejected it using its nonnil operation-ID validator. Import and summary success
do not qualify that panel.

The correction separates canonical principal UUID validation from nonnil
request/job/cursor validation, preserving the Native-injected identity, existing
host authorization and member isolation. Source regressions cover nil-principal
submit/list/get/result and journal reopen without regeneration, invalid principal
forms, nil operation IDs and authorizer refusal. The later `92c5fb961c` Native
suite and refreshed `8eb0b9a532` GUI artifact qualified the correction, as recorded
below; the original b755 observation remains preserved in
`local-performance/audio-import-diagnostics.json`.

Fresh hosted frontend/editor, CLI, container, format/lint/security and configured
Linux x64 CPU qualification passed at `b755769c25`; real Windows speech processing
also passed on both architectures. Both native Windows installer/runtime jobs
subsequently passed; macOS workspace checks found the two noncanonical temporary-root
fixtures corrected below. The backend failure came from a policy
fixture's global startup observer seeing legitimate eager warmup. Its corrected
fixture blocks the target's real startup, proves policy reads still complete,
then releases/joins that startup, retaining authorization/capability checks.
The exact case passed on the stable Native graph and frozen Linux production
plus the test-only overlay. An initial Native command used its former function
name and selected zero tests; that log is explicitly excluded from qualification.
The follow-up empty-stream display fix passed 43 timeline cases and typechecking;
an initial wrong-path test selection is likewise not a test pass. Both corrections
remain outside that hosted checkpoint's qualification. Local lifecycle,
output deletion and transcript activation gates remain default off.

## Corrected paced capture acceptance, 2026-11-09

The corrected real-time run passed `capture_soak_wall_clock`: one passed, zero
failed or ignored, 7,477.73 seconds total. The producer ran for 7,200.914 seconds
before its deliberate process exit, sealing 7,200 chunks, 115,200,000 frames and
921,600,000 PCM bytes at 16 kHz, with distinct microphone/system stereo channels.
Maximum producer lateness was 50 ms. A separate process resumed the original
capture identity and processing declaration. This is generated audio through the
actual authenticated SDK/server, not OS physical capture.

Production source was frozen at `573da5761740040aee94d57f71ca9cdb3379cab9`;
only the test harness overlays `5be9242930` and `31ad81601` were added. The immutable
server SHA256 was `0f22f2467c50461eda818f726921a4ae5632134e8905a69e528b7c62f636d6fd`;
the harness SHA256 was `02b1c1299a518a156a9e6be4a30300a07b83ded0c5544becd2e282f8b7734e03`.
The controlled Linux ARM64 build used the verified base-q8 Whisper model already
recorded in the portable evidence. It does not qualify later current-head code,
production amd64 packaging, native Windows or microphone permissions.

All original faults occurred: 3-, 600- and 5-second network outages, a server
restart at 70 minutes, two lost successful chunk replies, one lost successful
publication reply, and producer/app restart after sealing. Bounded proxy shaping
also exercised these profiles without changing host networking:

| Added RTT / deterministic jitter | Per-direction bandwidth | Successful chunk PUTs | Observed proxy-to-header median |
| --- | --- | --- | --- |
| 50 ms / ±5 ms | 2 MiB/s | 120 | 63 ms |
| 150 ms / ±20 ms | 1 MiB/s | 119 | 174 ms |
| 500 ms / ±50 ms | 512 KiB/s | 120 | 518.5 ms |

Observed proxy-to-header durations are separate from configured added RTT and
are not end-to-end latency measurements. Every profile carried over 15 MB of
upstream plaintext and nonzero downstream traffic. The 600-second outage exceeded
the existing 120-second no-admitted-audio deadline: live processing retained its
40 unique caption sequences and required canonical repair. This is expected
bounded idle behavior, not a claim of uninterrupted live captions across the outage.

Exactly one canonical repair job succeeded, from 02:12:16.760 to 02:15:14.173 UTC.
Each of the 24 speech windows independently contained the required recognizable
English words. The verifier compared every PCM sample in both channels, frame
count, WAV length and an independently streamed SHA256. Canonical audio was
921,600,068 bytes with SHA256
`9d6cde6f90951a0a9ba5505160633baa599bd413e45db2a657bf942d54f7c7ca`.
Maximum observed server RSS/high-water was 448,384 KiB before restart and
372,036 KiB afterward. The explicit final gates read 447,584 and 371,516 KiB;
all were below the unchanged 786,432 KiB limit.

Fresh authenticated canonical verification and actual-use drains preceded raw
retirement. Both server and device retained `retired` tombstones with zero chunks;
a separate read-only inspection confirmed both physical chunk directories empty.
They share cleanup UUID `1be02e04-bd82-4769-917b-67cd4eb090b1`. Acceptance completed
at 02:16:22.681 UTC, then the server/test container exited successfully.

The run deliberately overlapped two bounded, single-CPU fixture checks with no
inference, the optimized Native 5,001-session startup/model prewarm, and one
Native GPU Ollama summary job from 01:49:52.312 to 01:49:58.111 UTC. Capture lateness
remained 50 ms. These exact intervals and their qualification limits are preserved
in the evidence; no additional CPU transcription ran during canonical repair.

The portable JSON record is
`docs/plans/evidence/frontend-backend-checkpoint-2026-11-08.json`, under
`records.corrected_paced_capture_2026_11_09`. It embeds source/artifact/model hashes,
fault events, shaping counters, semantic/PCM results, sampled memory, tombstone
inspection, concurrent intervals and hashes of the local evidence files. Full
logs and canonical output remain under the local cache in
`tmp-linux/soak-shaped-paced-7200-31ad81601` and
`logs/soak-shaped-31ad81601-provenance.json`. Earlier failed paced runs and the
separately labelled non-paced repair replay remain preserved; this corrected pass
does not rewrite their outcomes or mark the full implementation plan complete.


## Local identity correction qualification, 2026-11-09

The correction at `92c5fb961c` passed the complete stable Native 21-package library
suite: **2,429 passed, 34 explicit ignored, no failures**. A task-only Cargo runner
set the actual macOS `/var/folders/.../T/` alias only for test execution, preserving
Cargo's dynamic-library runtime environment and canonical compiler/cache paths.
All three new principal cases passed, along with both previously failing Local
lifecycle fixtures and strict journal-reopen coverage. The subsequent all-target
Cargo check passed. Generated bindings came from the real exporters and were
formatted directly; they have no type-surface change. An initial result verifier
mistakenly counted two pre-existing principal tests alongside the three new cases;
its scoped-name correction did not change or rerun the successful library suite.

The actual b755 audio import and automatic summary completed before this fix. Its
73 words and 73 hints reflect one detected speaker cluster; the diarizer ran eight
temporal segments, which is not proof of eight speakers or multi-speaker accuracy.
The old app then quit through its ordinary menu and its verified PID stopped.
The refreshed optimized artifact reused that original vault/journal and qualified
the status panel without reimporting or inferring historical work, as detailed below.
Compiler-only Debug incremental objects were pruned after successful checks to preserve local disk headroom;
dependency libraries, executables, release cache, models and raw evidence remain.


The refreshed optimized artifact from `8eb0b9a532` passed the original-vault GUI
reopen check. Executable SHA-256:
`829802c0384861bb9718e442be01f31cc93a51bf0b19dcc81276cb55c85e09d6`.
It reopened the original `english2` transcript with Media saved/Summary saved and
a working Transcription jobs panel, without the former UUID error or Retry.
The settled empty future-stream count is gone. All six original canonical file
hashes and all 18 private journal JSON hashes remain identical, with no added or
removed journal/job entries and zero new Ollama inference POSTs. Startup's selected
Whisper prewarm remains distinct from ASR. The baseline b755 artifact/manifests
were preserved. Exact source/binary/runtime and comparisons are copied into the
portable checkpoint under `records.local_principal_refreshed_gui_2026_11_09`;
raw evidence is in the task lab's `local-performance-refresh/reopen-acceptance.json`.

All hosted jobs for `b755769c25` are terminal. Both native Windows architectures
passed NSIS, installed app startup, installed CLI/MCP and uninstall, each with
2,235 passed/zero failed/47 ignored across 69 test cohorts. Both explicit Windows
export ownership/writer regressions passed. The PR jobs checked out synthetic
merge `f265e3cbe5` (b755 into `a6a8f001d9`); the separate successful Windows speech
dispatch checked out b755 directly. Backend/macOS failures remain recorded as
failures of their original fixtures, with later correction evidence scoped above.
No healthy CI was cancelled or retried. Portable records
`hosted_b755769_terminal_2026_11_09` and `native21_local_principal_var_2026_11_09`
preserve the terminal outcomes, artifact/log hashes, Native totals and runtime-runner
audit. These results do not qualify subsequent current-head changes.

The refreshed Remote GUI at `2263b47eb7`, against server `8eb0b9a532`, qualified
Owner File → New Note and the patched Reader controls. Reader note/date/tag/capture,
global processing, creation and import controls were disabled; Reader File → New
Note was refused and the original two notes remained. Synthetic browser PKCE and
verified loopback HTTPS were used, without OS trust or user credential changes.
This is not production HTTPS SSO or physical capture acceptance. One explicit
Remote summary reported saved but contained only an opening HTML fence;
semantic summary acceptance remains open; the later narrow empty-fence publication
guard has focused and actual functional verification below. The
About panel opened without Local vault statistics; its cached build-commit label
was subsequently corrected and qualified by the `6e7656d1b2` artifact. The Reader/menu
receipt remains embedded under `native_remote_reader_menu_2263_2026_11_09`.

The actual `6e7656d1b2` artifact, in a fresh task-only profile, completed synthetic
Alice browser PKCE, reopened Personal, and displayed version `0.45.7` and commit
`6e7656d1b2` through the native About menu, without Local vault statistics or any
inference request. Reusing the older unsigned profile had blocked credential loading
in `SecKeychainFindGenericPassword`/SecurityServer IPC while the main thread remained
in its normal AppKit event loop. An unsigned identity/credential ACL mismatch is an
inference, not an established OS prompt diagnosis. The older process quit normally;
no OS grant was made, no existing credential was deleted, and original global/profile
and journal files remained byte-identical. A new profile identity avoided modifying
that original state. Portable records `native_remote_about_6e_2026_11_09` and
`native_remote_unsigned_startup_6e_2026_11_09` retain the receipts and their hashes.
Physical recording, production HTTPS SSO and final current-head platform
qualification remain separate gates.


## Empty generated-fence publication guard, 2026-11-09

Actual Remote QA exposed a saved seven-byte summary containing only an opening
HTML fence. Shared generation now rejects fence-only output before retaining a new
result or publishing canonical content (`080764fe20`). It preserves short prose,
real fenced content and literal fences in indented code. Historical retained-result
validation and receipt replay remain unchanged; no extra model attempt or retry
budget was added, and rejected output does not overwrite the canonical summary.

The default stable 21-package Native library graph with the `fence` filter passed
12 tests, zero failures and zero ignored, including all five exact new regression
names. The all-target check passed in 28.17 seconds with existing warnings. An
initial post-run verifier expected an invented test-name substring and failed;
correcting it to the five actual names verified the already successful log without
rerunning or altering the tests. The portable record
`native_generated_empty_fence_080_2026_11_09` preserves that distinction and the
source/log hashes. These focused results are separate from the earlier full-suite
2,429-pass total. The subsequent guarded-server runtime check below qualified
functional publication, with model accuracy and conciseness explicitly failing.


## Guarded Remote reconnect and functional summary, 2026-11-09

The older `6e7656d1b2` client reopened its persisted authenticated profile without
browser login or an OS grant. During the server's ordinary stopped/drained restart,
the actual GUI showed “Reconnecting… Loaded notes remain available.” Loaded text
remained available and read-only, mutations were disabled, and no Local fallback
occurred. It automatically reconnected to the verified HTTPS server built at
`080764fe20` (binary SHA-256
`f34df7191ab77357f5e3c50e7e8c94b168bf4d46e7cdb54e7893623c9de505ef`).
The original seven-byte summary, its one job, result, preview and title receipts
remained byte-identical. Restart dispatched no new job or inference. This proves
the recorded older-client/newer-server combination with the synthetic issuer; it
is not production SSO qualification.

One newly requested GUI job, `5ea0cf2b-6e7a-475c-9ed4-4500716d6e9c`, completed in
15.224 seconds. Its retained result exactly matched the 3,125-byte canonical
summary (SHA-256 `76d296ae78759c0bc00a8eeba67e071ad61223795a32d625418f1f074ff21594`),
and the renderer reported saved. Exactly two jobs then existed: the unchanged
historical job and this fresh job. Two bounded serial Ollama completion POSTs
were observed; the first attempt was cancelled and the second produced retained
content. Title processing finished `already_named`. No historical replay or
further inference was requested to improve the result.

Functional generation, retention and canonical apply passed. Model accuracy and
conciseness did not: the output grew to 464 words from a 124-word note, invented
completed regression work, conflated raw retention with a Friday restore deadline,
misapplied offline/login-free requirements to Remote use, and contradicted explicit
action items. The exact synthetic input and contradictory output remain in the
portable receipt. An intermediate GUI preview was not observed; the retained
server preview alone is qualified. This is not semantic summary acceptance.

Portable records `remote_guarded_restart_080_2026_11_09`,
`remote_guarded_summary_quality_limited_080_2026_11_09` and
`native_guarded_artifacts_080_2026_11_09` preserve original receipts and hashes.
Final current-head CI, physical capture/OS permission confirmation and production
HTTPS SSO remain open. Local activation gates remain default off.


The bounded read-only audit of that note-only job found no concrete prompt
assembly defect at `080764fe20`. The 849-byte note appears once in
`post_meeting_memo`; participants and transcript speakers are empty, and neither
Alice nor Bob is marked as the current user. There is no `transcript.json`, the
custom prompt is empty, and the default instructions prohibit invented identities,
facts and filler. The observed contradictions therefore support the recorded
model-quality failure. The raw provider request was not retained, so this is
source/canonical evidence rather than an exact HTTP prompt reconstruction. No
additional inference or speculative source change followed. The portable audit
is `remote_summary_prompt_audit_080_2026_11_09`.

## Capture profile fences during concurrent refresh, 2026-11-09

A final read-only crosscheck found three remaining profile/token lock-order sites
in capture append and retirement. They now reuse the existing
`authenticated_scope::original_commit_guards`: if a concurrent owned refresh
holds the token mutex, the capture operation releases its profile fence before
waiting, then rechecks the original profile epoch. Original member/server/vault,
request UUID, immutable declaration, login generation, cancellation and retirement
ownership checks remain intact. The change neither adopts a new baseline nor
introduces inference, mutation replay or raw deletion authority.

On source `24198b3296ca4afcb5817ab51d29b21ac4e1c1a6`, the stable 21-package Native
library graph passed 21 selected SDK cases, with zero failures and one explicit
ignored case. Two new public-boundary regressions passed:

- `tests::capture_append_authority::append_dispatch_releases_profile_while_concurrent_refresh_waits_for_credentials`
- `tests::capture_upload::cleanup_commit_releases_profile_while_concurrent_refresh_waits_for_credentials`

Both block a real SDK refresh at fake credential persistence with a held mutex.
The original capture guard is queued first; a profile save must finish before the
refresh is released, and the operation must then reject the changed original
profile generation. The cleanup case additionally verifies no extra retirement
POST and preservation of its raw chunk. These tests do not depend on a sleep or
an assumed HTTP response delay.

The third, pre-POST retirement site uses the same helper but has no new standalone
contention test. Its existing public regressions also passed:
`cleanup_dispatch_rejects_original_profile_or_login_aba_during_member_verification`
and `cleanup_dispatch_releases_profile_fence_before_waiting_for_retirement_response`.
This coverage distinction is intentional and recorded, not represented as a third
new blocked-refresh test.

The ignored case was
`cleanup_retirement_budget_outlives_ordinary_capture_timeout_without_changing_identity`:
it exercises the real 35-second ordinary request deadline against a 36-second
retirement response. It is not a soak test. This focused cohort performed no model
inference and does not replace the earlier full-library or frozen-source paced
acceptance. The all-target check passed in 31.80 seconds with the existing nine
library/four library-test desktop warnings.

The Windows workflow now runs the four standalone Git build-metadata regressions
with `rustc --test -D warnings` on both x64 and ARM64, without expanding the Cargo
feature/package graph. Actual execution of this new step on both architectures
remains pending. The workflow's local Python checks passed 29 tests; actionlint
and the high-severity zizmor gate passed. Zizmor ran offline with its existing
36 ignored/67 suppressed findings. At this checkpoint, hosted CI for the earlier
`073e5b7cd3e50caa890c48e317894582de39d878` source was still running and was left
untouched; it does not qualify these later source/workflow changes.

The existing portable JSON record now contains
`records.capture_refresh_lock_validation_2026_11_09`, including all 21 passed test
names, the exact ignored case, source hashes, coverage limits and SHA256 hashes
of the six actual validation logs. The primary logs are
`native-capture-refresh-fence-tests.log`, its `.json` result,
`native-capture-refresh-fence-check.log` and `capture-refresh-{workflow-tests,actionlint,zizmor}.log`
under the local task cache. All prior evidence records remain unchanged.

A separate explicit `--ignored --exact` invocation then passed
`tests::capture_upload::cleanup_retirement_budget_outlives_ordinary_capture_timeout_without_changing_identity`
on the same stable 21-package graph and source
`24198b3296ca4afcb5817ab51d29b21ac4e1c1a6`: one passed, zero failed or ignored.
The SDK case took 73.37 seconds, exercising an actual ordinary 35-second timeout
followed by a successful 36-second delayed fixture TLS retirement response while
preserving the original capture/request identity. It performed no inference and
was not a soak run. The earlier focused cohort remains historically 21 passed,
zero failed, one ignored; this later result does not rewrite that run.

`records.capture_retirement_deadline_explicit_2026_11_09` in the existing portable
JSON embeds the exact result and SHA256 hashes of
`native-capture-retirement-deadline-explicit.log` and its `.json` verifier. The
verifier checked the exact test name, 21 cohort result lines and the one-pass total.
Hosted CI for the earlier `073e5b7cd3` source was still in flight at this checkpoint;
no push, CI cancellation or new source build is represented by this evidence.

## Current Local physical-QA artifact prepared, 2026-11-09

An optimized, unsigned Local QA artifact was built from clean frozen source
`aa0a5ed1c70ac6ecd767e2bebc8567ea4d7f142d` in the sole Native worktree. Cargo
completed successfully in 4m20s using the existing 21-package graph, `--locked`
and `--bins`, with `desktop/dev`, `loofah-client/dev-fixtures`,
`loofah-server/cpu-transcription` and `tauri/custom-protocol`. Actual generated
`VERGEN_GIT_SHA` equals that source and is present in the executable; no metadata
environment pin, version change or cache reset was used. The helper's three tests
and bounded performance-overlay check passed.

The new artifact is **UNLAUNCHED** at
`~/Library/Caches/loofah-separation/qa/remote-browser-lab/local-physical-current/Loofah Local Performance QA.app`.
Its executable SHA256 is
`bd94f90970b421e9f35c586f2f68f741c18eab5b3d9dff52f6454b81039c5dd8`;
`bundle-manifest.json` SHA256 is
`e82c92ceea286a96d9a802daa7d90582725242342e1d559fce9284ca6db64d73`.
`verification.json` records the exact source, build receipt and preservation
checks. The adjacent `launch.sh` was prepared but not executed; its exact path
and SHA256 are recorded in the portable JSON.

The bundle retains `io.loofah.separation-qa`, the existing isolated
`local-performance` vault/global/model roots and original private journal root.
The task-only profiler overlay, separate temporary/download/log directories and
Native feature graph are unchanged in purpose. All 14,258 baseline files were
byte-identical after the build, including the original synthetic vault, global
configuration, models, journals and prior Local artifact executables/manifests.
No existing Local process was stopped. No OS grants, recording, download or
inference occurred, and no Local capability gate was activated. This prepares
an artifact for later authorized physical/concurrent-performance QA; it does
not qualify that acceptance or replace the earlier frozen Local GUI evidence.

The existing portable JSON appends
`records.native_local_physical_artifact_aa0_2026_11_09`, embedding the exact
verification receipt plus manifest/launch/preservation hashes, build features
and unchanged task data roots. Previous records remain unchanged.

## Git metadata fixture path portability, 2026-11-09

Fixture-only checkpoint `b929baf7d` follows parent `365c2f7db9`; production
`plugins/misc/build_git.rs` is unchanged. The fixture now canonicalizes the
administrative directory returned by Git before comparing it with production
canonical paths, and creates its linked worktree using `../linked` from the
known main repository directory. Assertions and the four covered scenarios
remain unchanged.

Rust's [official canonicalize documentation](https://doc.rust-lang.org/std/fs/fn.canonicalize.html)
states that Windows uses extended-length paths, which may be incompatible when
passed to other applications. This motivates fixture portability hardening;
there was no observed Windows failure and this is not an actual Windows pass.
The existing standalone invocation compiled with
`rustc --edition=2024 --test -D warnings` and passed all four tests locally on
macOS ARM64 in 0.21 seconds, with zero failures or ignored tests. Formatting
and `git diff --check` also passed.

`records.git_metadata_fixture_portability_b929_2026_11_09` in the existing
portable JSON records the exact source/helper hashes, commands, result and
SHA256 of `git-metadata-path-portability-b929-tests.log`. That task log copies
the exact test stdout from the completed tool execution; this documentation
step did not rerun tests. Prior evidence records remain unchanged. No Cargo
or Native build, inference or permission changes occurred. The current `aa0`
Local physical-QA artifact's production code is unchanged and the app remains
unlaunched. Earlier-source hosted Windows checks remain a separate pending
gate; no push or cancellation is represented here.


## Original Local owner shutdown, 2026-11-09

After the unlaunched `aa0` artifact was prepared, the original `8eb` Local app
quit through its normal Native menu and PID 50115 was verified absent. Before
quit, `english2` showed Media Saved and Summary Saved, with no active capture,
pending job or UUID error. An independent comparison found six canonical files
and 18 private journals byte-identical, with the same eight job/result/title/preview
files and no additions or removals. The current `aa0` artifact remains unlaunched;
this only frees the original owner for later permission-stage QA. The earlier
build-time statement that no old process was stopped remains true for that build.
Portable record `native_local_old_owner_shutdown_2026_11_09` retains the exact
receipt and hashes. It makes no global Ollama request-count claim, and does not
qualify physical capture or OS permission acceptance.


## Terminal hosted checkpoint `073e5b7cd3`, 2026-11-09

All seven workflows completed successfully, observed at
`2026-11-09T05:29:44.812031+00:00`. PR jobs checked out synthetic merge
`a471c04829c9fec21ab0f3fd0fe4069fc3d3f2d7` of
`073e5b7cd3e50caa890c48e317894582de39d878` into unchanged base `a6a8f001d9`.
The separately authorized Windows speech dispatch checked out `073e5b7cd3`
directly. No healthy job was cancelled or retried.

- Native Windows x64 and ARM64 each passed 2,243 tests, zero failed and 47 ignored
  across 69 cohorts, then NSIS installation, app startup, installed CLI/MCP and
  uninstall smoke checks. All three Local-principal and five empty-fence regression
  names actually executed successfully on both architectures. The four Git
  metadata helper tests were absent: this workflow did not select their package.
  The new standalone Windows step is a later-source coverage gate, not an inferred
  pass at this checkpoint.
- macOS completed 3,231 passing test executions, zero failed and 124 ignored across
  182 desktop/workspace cohorts; this is a different scope from the local
  stable-21 suite. Frontend passed 2,622 desktop tests in 298 files and 303 editor
  tests in 31 files, with typechecking. CLI passed 274 tests, zero failed and two
  ignored across 20 cohorts, plus its release build.
- Headless backend passed 1,661 tests, zero failed and 19 ignored across 22 cohorts,
  including all five maintenance integration cases. Container, configured Linux
  CPU, formatting, lint and security checks passed. Each original configured
  kernel case selected exactly one passing test. Reserved ONNX overlap delivered
  its first caption while batch owned its model, with concurrent HTTP at 12 ms.
  Windows real Whisper/ONNX transcription and speaker checks passed on both
  architectures.

`records.hosted_073e5b7_terminal_2026_11_09` embeds the terminal receipt with run/job
URLs, raw log hashes, exact-case checks and both installer artifact IDs/digests.
The raw audit remains in `logs/ci-073e5b7-audit`. Later SDK lock-order correction
and Git fixture changes retain their separate local qualifications: 21 focused
passes with one initially ignored deadline test, that explicit deadline test
subsequently passing, all-target checking, and four standalone macOS Git tests.
Their next hosted checkpoint still needs both Windows architectures. Physical
capture/OS permission confirmation, production SSO staging and the observed LLM
quality limitation remain open; Local activation gates remain default off.

## Windows Git fixture configuration failure and correction, 2026-11-09

The actual Windows x64 standalone metadata step on
`36f85d04c80cc4a03ac779ba9979f9581e012115` failed all four tests before the
production helper was reached. Job `128998508014`, log lines 885–925, shows
initial `git init --initial-branch=main` failing with “unknown error occurred
while reading the configuration files” at `plugins/misc/tests/build_git.rs:63`.
This is an observed fixture failure, not a production helper regression.

The earlier `b929baf7d` adjustment normalized comparison paths and the worktree
argument but still passed canonical Windows extended-length paths through
`GIT_CONFIG_GLOBAL` and `core.hooksPath`. Fixture-only correction `c96bb69fd`
uses `../config` and `../hooks`, with an assertion restricting each Git command
to the known `main` or `linked` sibling directory. The empty global config,
disabled system config, dedicated empty hooks and all existing assertions remain
intact; production `build_git.rs` is unchanged. The configuration-read failure
is consistent with the external-path incompatibility described in Rust's
[canonicalize contract](https://doc.rust-lang.org/std/fs/fn.canonicalize.html);
corrected Windows execution is still required to verify the diagnosis and fix.

The actual local standalone compile with `rustc --edition=2024 --test -D warnings`
passed all four tests on macOS ARM64 in 0.18 seconds, with zero failures or
ignored tests. Its stdout/stderr were redirected directly to
`git-metadata-relative-config-tests.log`. Formatting and diff checks passed.
The existing portable JSON appends
`records.git_metadata_windows_config_failure_and_fixture_fix_2026_11_09` with
exact test/helper hashes, failed hosted-log/snapshot hashes and local-result
log hash. Earlier evidence remains unchanged, including the earlier local pass.

This is not a Windows fix-pass claim. Other hosted checks were left running
without cancellation or retries. No Cargo/Native rebuild, production helper
change, inference, permission grant, launch, push or PR write occurred. The
`aa0` Local physical-QA artifact remains unchanged and unlaunched.


## Terminal PR workflows at `36f85d04c8`, independent speech pending

All six PR workflows are terminal: backend, desktop, CLI, formatting/lint and
security passed. PR checkout was synthetic merge
`9d1791521e04c9e55e7736c2eee48a0ffe61351a` of exact branch head
`36f85d04c80cc4a03ac779ba9979f9581e012115` into `a6a8f001d9`. macOS logged
3,233 passing executions/zero failed/124 ignored across 182 desktop/workspace
cohorts; Linux headless passed 1,663/zero failed/19 ignored across 22 cohorts.
Both public SDK blocked-refresh regressions actually passed in both jobs.
Frontend passed 2,622 desktop and 303 editor tests plus typechecking; CLI passed
274/zero failed/two ignored plus release build. Configured Linux CPU, reserved
live/batch overlap and container checks passed.

Both native Windows jobs ran the four new standalone Git fixtures and failed all
four at initial `git init`, with “unknown error occurred while reading the
configuration files.” They did not reach the production helper, Cargo suites,
new SDK cases or installer/runtime checks. The fixture passed Windows extended
paths to external Git configuration/hooks settings; the later relative-path
correction is recorded above. This is not a Windows pass for that correction.
Raw logs and hashes preserve each architecture's actual failure independently.

The separate speech run `42568044984` remains in progress at this receipt and
continues untouched on direct `36f85d04c8`. The workflow uses
`windows-ci-${{ github.ref }}`: its branch-ref group differs from the next
`refs/pull/123/merge` group, and push triggers only target `main`. Thus the next
fixture-only PR push does not cancel the remaining manual ARM64 work. No retry or second
manual speech dispatch is planned for unchanged production processing code.
The next PR still must execute all four helpers and both SDK concurrency cases
on both Windows architectures.

Portable record `hosted_36f85d0_pr_terminal_speech_pending_2026_11_09` embeds the
source, exact workflow/job states, raw log hashes, failure details and concurrency
proof. It deliberately does not claim every workflow terminal or speech success.
All earlier records remain unchanged; no cancellation, retry, release or user
permission change was made.


The direct-36 x64 speech job failed at the public six-speaker
fixture download with HTTP 504, before speaker inference. Its preceding Whisper
and ONNX transcription cases passed; this is not a speaker-model regression or
a complete direct-36 x64 speech pass. ARM64 remains running at this update.
`records.hosted_36_speech_x64_fixture_download_failure_2026_11_09` preserves the
exact failed step, timestamp and raw log hash. No retry, cancellation or second
manual dispatch was made. The earlier pending receipt remains unchanged.

## Listener fixture actor-name collision and local validation, 2026-11-09

The actual macOS workspace job `129005473438` at
`8ca3db9a4055fe257a73f21dd5caa6a8c9ebc9a1` failed the recovery-mask test at
`root.rs:1622`: `StartDeviceCapture` returned `FailedToStartSession`. The listener
cohort was 89 passed, one failed and zero ignored. Its raw log remains preserved.
The recovery fixture's `e` iteration and the parallel maintenance fixture both
used the same 64-character `eeee…` capture key. Their independent fake roots
still register session actors in Ractor's process-global namespace. The
maintenance test deliberately holds source/recorder ownership while the recovery
test tries to register the same name. The log reports the recovery failure at
06:09:42.221330 UTC and maintenance completion at 06:09:42.229613 UTC, 8.283 ms
later. These are test-completion timestamps, not measured registration times.

Worker fix `d2d59a7593`, integrated as
`8a4160c03aec8188a335fd7da464b48e06ce887d`, changes only `cfg(test)` helpers and
coverage. Character and numbered fixtures now allocate unique, validated
64-hex-character keys using a shared process-local counter. Each test keeps its
original key for subsequent operations. Production actor names, physical/source
fences, actual drain ownership, timeouts and existing assertions are unchanged;
there is no global test serialization. The new regression keeps two same-label
captures alive together and verifies separate source ownership, independent
stops, exactly 16,000 finished frames each and actor unregistration after drain.

The complete listener-core library passed **91 tests, zero failed and zero
ignored**, in 1.14 seconds at default parallelism on frozen source `8a4160c03a`.
The named cases all passed:

- `actors::root::tests::same_label_device_fixtures_keep_independent_actor_names_and_drains`
- `actors::root::tests::all_recovery_masks_interrupt_device_prefix_without_reopening_source`
- `actors::root::tests::maintenance_rejects_active_retiring_and_storage_finalizing_capture`

The Cargo runner retained the unchanged stable-21 feature graph and Cargo
runtime environment. It ran `listener_core-*` unfiltered and intentionally
selected zero matching tests in the other 20 executables; those executables are
**not qualified by this run**. The same graph's all-target check passed in 21.01
seconds, with the existing nine desktop library and four test warnings. This
used injected fake audio and no model inference or physical capture.

Portable record `records.listener_fixture_actor_namespace_collision_2026_11_09`
contains the exact source, three test names, failed CI log hash, timing evidence,
runner/receipt/check hashes and an independently compared unchanged production
prefix hash. Prior failure and qualification records remain unchanged. This is
local fixture validation, not a new hosted Windows or physical-QA pass. No CI
retry/cancellation, production artifact change, build by the docs worker,
inference or push occurred; ongoing hosted checks retain their separate scope.


## Windows speech retry at the original source, 2026-11-09

The direct `36f85d04c80cc4a03ac779ba9979f9581e012115` speech run
`42568044984` completed successfully on its second attempt, observed at
06:38:12 UTC. Both Windows x64 and ARM64 logged eight passing tests, zero failed
and zero ignored across seven cohorts: strict Whisper CPU, live/batch ownership,
ONNX transcription, real speaker labels and the reference embedding. Both raw
checkout logs identify the original direct branch commit, not a later PR merge.

The first attempt on each architecture failed the public six-speaker fixture GET
with HTTP 504 before speaker inference. Those logs and the first failed local
availability check remain preserved. After a later bounded public-URL/API
preflight returned identical valid 1,303,284-byte PCM16 mono 16 kHz WAV files, the
single authorized failed-jobs-only retry ran. Its computed fixture SHA256 is
`51a8b70d0213e25b410aa9494044f8a031177c144ffeca2a72f70aded939f760`;
no upstream or committed trusted digest pin was available. No healthy job was
cancelled and no new manual workflow was dispatched.

Portable record `hosted_36_speech_single_retry_terminal_2026_11_09` retains the
failure, preflight, retry authorization, exact source and both successful raw-log
hashes. A source comparison through local `582d6d858e` confirms only docs,
standalone Git fixtures and listener `cfg(test)` code changed after `36`; the
listener production prefix is byte-identical. This permits source-equivalent
processing qualification without claiming later changed runtime code was tested.

## Production append HTTP contract coverage, 2026-11-09

Frozen local source `67b9213990798a5080292ac67bd20195da776e0d` passed exactly
three authenticated production Axum-router tests: exact mono/stereo raw append
publication and replay after service reopen, stale transcript CAS rejection
without refreshed authority, and reader/original-request scope enforcement.
The unchanged stable-21 graph ran one qualifying server cohort; the other 20
executables intentionally selected zero cases and are not qualified by this run.
All-target checking passed in 21.24 seconds with the existing nine desktop library
and four test warnings.

Portable record `native_production_append_http_routes_2026_11_09` includes the
three exact names and independently verified test/check log hashes. These tests
used generated raw PCM, with no inference or physical capture. The
`capture_append` capability remains unadvertised, and Local lifecycle, output
deletion and transcript activation gates remain off. This is local source-scoped
coverage, separate from the hosted `8ca` results.


## Hosted PR checkpoint `8ca3db9a40`, terminal 2026-11-09

All six PR workflows were terminal at 07:13:59 UTC. Backend, native Windows, CLI,
formatting/lint and security passed. The desktop workflow retained its macOS
listener fixture failure; its frontend job passed. PR jobs checked out synthetic
merge `6d46da2f1dd9bd1bcc66577fa3cf48e644763d29`, with parents
`a6a8f001d9165881e59fd0fa29df944da26b8801` and branch head
`8ca3db9a4055fe257a73f21dd5caa6a8c9ebc9a1`.

Each native Windows architecture logged **2,249 passing executions, zero failed
and 47 ignored across 70 cohorts**. This aggregate includes four standalone Git
helper tests and four release-policy tests outside Cargo. Raw logs independently
confirm all four exact Git helper names, both blocked-refresh SDK regressions,
all three Local-principal cases and all five empty-fence cases passed on each
architecture. Both NSIS builds and installer, app startup, installed CLI/MCP and
uninstall smoke tests passed. Artifact metadata records x64 artifact `13654649310`
with digest `cc547bfd2061bfe909043407a539b6daf6abc50c8556deaa261258206e3e4142`,
and ARM64 artifact `13656176331` with digest
`0cc99663907ba30c7bc7a71904b5cc010f157db875b10597099bd1b296524a93`.

Linux headless passed 1,663 tests, zero failed and 19 ignored across 22 cohorts;
both new SDK regressions passed there. Configured Linux CPU, reserved live/batch
overlap and container checks passed. Frontend passed 2,622 desktop tests in 298
files and 303 editor tests in 31 files plus typechecking. CLI passed 274 tests,
zero failed and two ignored. macOS stopped at the already diagnosed listener
fixture after 595 passing executions, one failed and 16 ignored across 33 cohorts;
that job did not reach the SDK/Core cases. Its earlier desktop library passed 193
cases with two ignored. The later local 91-case listener correction and three
append HTTP cases above remain separate source-scoped evidence.

Portable record `hosted_8ca3db9_pr_terminal_2026_11_09` preserves exact head/merge
parents, workflow/job outcomes, named-case checks, raw log hashes and artifact
metadata. The successful speech retry is recorded separately at original source
`36`, rather than presented as a new `8ca` dispatch. No current PR job was retried
or cancelled. Later implementation work and the next head's hosted qualification
remain outside this record; physical capture, production SSO and model-quality
gates remain open.
## Explicit Remote media-source retirement: Core/protocol source checkpoint

The new `MediaDispatcher::retirement_status` and `retire` contract keeps
retirement separate from existing media/result phases. Only an unequivocal
completed media receipt, or a new admission durably marked never dispatched,
can qualify. Ambiguous, failed, dispatched-cancelled, legacy unproven and Review
work stays retained. Original summary work must be skipped or successfully
finished; active model, session, upload writer and original-source readers block
cleanup through their actual drain. Exported/discard/no-upload dispositions are
explicit and original revision/declaration/source-bound. Exported is a caller's
retained-copy decision; Core verifies the complete original source but does not
claim to inspect the external copy.

Before any unlink, a version-3 admission records the immutable decision and
exact source/partial file identities, lengths and hashes. Cleanup refuses
unknown files, links, changed identities and byte mismatches. It preserves the
empty private source directory, canonical vault content, and every lower media,
subtitle, summary and publication journal. The completed receipt is written to
the separate bounded `retired/` archive before removing the active admission;
only that final removal releases active request and declared-byte quotas.
Archive reservations include unfinished retirements, with a fixed 4,096-record
and 16 MiB ceiling. Finished dispatcher task/receiver entries are removed.
Retired point lookup and exact decision replay remain available without
re-admission, source re-export, decoding or inference. Completed replay ignores
later session activity and never touches subsequently reappeared source bytes.
An interrupted cleanup keeps its active quota and accepts only its original
explicit decision to continue.

New admissions persist explicit dispatch evidence before model/application
work. Existing v1/v2 records remain readable; missing evidence is never upgraded
to never-dispatched. A proven legacy success can acquire a v3 retirement intent.
The bounded operational sampler accepts validated v3 intents and continues to
count active pending cleanup. Stopped whole-storage backup already includes
these private records and retained directories. **Downgrade after retirement is
incompatible:** older binaries may ignore the archive and reuse a retired UUID.
Restoring a complete matching pre-retirement backup is required before running
such a binary; a new-reader tombstone is not an old-reader global format fence.
Pending identity-bound cleanup restored onto different filesystem identities
fails closed for operator review rather than deleting replacement files.

Focused source regressions cover both quota limits, original replay after
reopen, actual source-reader/caller drain, canonical/result preservation,
crashes after source unlink and after receipt publication, ambiguous states,
member/revocation checks, changed/unknown/hardlinked/symlinked sources, bounded
archive exhaustion, future receipts and completed replay during later work.
Filters are `local_cli::remote::retirement::tests` and
`media_retirement_binds_original_declaration`. This checkpoint has formatting
and whitespace checks only; Cargo execution and lockfile resolution are reserved
for the main validation lane. The narrow Windows `winapi-util` dependency uses
volume/file-index and link-count evidence; both native Windows architectures
remain required. No retirement capability, automatic cleanup, exported bindings,
model inference or application launch is activated by this source checkpoint.

## SpeechCore download resilience in macOS CI

The two `0534474` macOS attempts failed before workspace tests while SwiftPM
fetched the same pinned SpeechCore v0.0.14 archive (HTTP 504). A single local
preflight returned the exact 2,112,822-byte archive and matched its upstream
SwiftPM checksum; that did not establish availability from the hosted runner.

The CI-only prewarm verifies the committed speech-swift 0.0.29 revision, checks
an existing artifact before network access, and caches only the exact
checksum-keyed archive. Missing bytes receive at most three classified transient
attempts, each with a 45-second total transfer deadline, bounded backoff and a
16 MiB limit. Redirects stay on explicitly approved official HTTPS hosts; signed
redirect URLs and raw curl errors are not logged. Checksum, TLS and permanent
HTTP failures are not retried. Verified bytes publish atomically in the same
cache directory, and SwiftPM independently verifies its unchanged manifest
checksum. Cargo compiler/test commands still execute once; no dependency pin,
TLS/signature behavior or production build script changes.

The failed runner log records image `macos-26-arm64/20261102.0473.1` and active
Xcode 26.6. Its [exact image manifest](https://github.com/actions/runner-images/blob/macos-26-arm64/20261102.0473/images/macos/macos-26-arm64-Readme.md)
confirms that default, and [Apple's Xcode 26.6 notes](https://developer.apple.com/documentation/xcode-release-notes/xcode-26_6-release-notes)
identify Swift 6.3. The historical logs do not print a SwiftPM version command.
The new step prints and checks `swift package --version`, refusing an unreviewed
layout version. The shared `artifacts/` location and URL-derived filename follow
[SwiftPM 6.3's configuration](https://github.com/swiftlang/swift-package-manager/blob/swift-6.3-RELEASE/Sources/Workspace/Workspace%2BConfiguration.swift#L138-L140)
and [binary artifact implementation](https://github.com/swiftlang/swift-package-manager/blob/swift-6.3-RELEASE/Sources/Workspace/Workspace%2BBinaryArtifacts.swift#L548-L602).

Offline helper regressions use temporary task-local files and simulated network
responses. They cover exact pins/cache key, cache verification without writes,
atomic publication, transient limits, partial cleanup, checksum/size/TLS errors,
redirect host/deadline bounds, and symlink/hardlink refusal. No user-global cache
is modified by local validation, and no new download, Native build, inference or
workflow rerun is performed as part of this source checkpoint. Actual hosted
cache consumption and the previously unreached workspace tests remain pending.

Source validation passed all **44 workflow-helper tests**, including 15 new
prewarm tests, plus formatting/whitespace checks and offline zizmor at the
workflow's high-severity threshold. Actionlint reported the existing missing
name/description in the unchanged `pnpm_install` action and existing SC2086 on
the SDKROOT line; excluding exactly those baseline diagnostics passes, with no
new workflow diagnostic. The existing workflow-helper discovery already runs
these tests, so the local run-tests script needs no parity change.

## Local qualification of explicit media-upload retirement

The checked Core, authenticated HTTP, SDK, Native receipt/export and shared
recovery-panel implementation is integrated through `0833c8ccf9`. The new
`media_retirement` capability remains unadvertised. Existing Local
lifecycle/output/transcript activation and server `capture_append` gates are
unchanged; these checks do not activate those workflows or establish physical
capture, OS permission, production SSO or current Windows acceptance.

Retirement removes only the dispatcher's exact retained upload `source` and
`partial` files and releases active upload/request quota. It preserves canonical
content, lower subtitle/normalization/publication copies and all result/operation
journals. Discard can remove the only original uploaded copy and needs explicit
review. Exported disposition requires an actual Native verified no-clobber save;
that observation records publication, not continued possession of the saved
file. A merely local pending decision or server-Blocked outcome still permits
an explicit source save. Server-confirmed Retiring/Retired stays fenced, including
when a later status refresh fails. Saving does not resume cleanup, change the
original disposition/UUID, or unblock upload/recovery/cancellation/inference.

The request/source gates, original summary pin and shutdown ownership cover both
source hash passes. A short initial session busy check releases exclusivity while
hashing; fresh lifecycle, job-idle, drain and authorization checks protect final
unlink/archive. Deterministic tests complete checked note appends during both
paused hash passes and reject late session work or revocation before deletion.
The five-minute retirement POST budget remains separate from ordinary 35-second
media calls. Lost replies retain the same durable decision; readonly status
never initiates cleanup. Unknown files, changed bytes/identities, link aliases,
future schemas and ambiguous work remain preserved. There is no startup sweep.
The schema3/tombstone downgrade restriction described above still applies.

Qualification uses distinct source scopes:

- `31aa435da0`: focused Native 21-package graph, **60 passed / 0 failed / 0 ignored**, with
  nine nonzero cohorts, including 13 Core retirement cases, the 19-case SDK
  selection, 11 server cases, protocol/CLI checks and four actual Native exporters.
  The all-target check passed in 46.85 seconds. Zero-match cohorts are not tests.
- `cc33740c0`, including the session-fence correction: focused selection
  **62/0/0**, followed by the complete 21-library cohort **2,463/0/34**, with no
  filtered cases. The 34 ignored cases retain their original scope and are not
  model, timing or physical acceptance.
- `0833c8ccf9`, after the SDK/Native/UI pending-export correction: targeted Native
  **32/0/0** across four nonzero cohorts; the 17 zero-match executables are not
  qualification. The corresponding frontend selection passed **36 tests in two
  files**, and desktop typechecking passed. This later targeted run is not a
  second full-suite result and is not added to the older full-suite total.

The earlier full frontend run used `31aa435da0` plus actual generated protocol
and Native contracts later committed in `cdd8dda77`. Shared editor **303 tests in
31 files**, desktop **2,630 tests in 298 files**, and both typechecks passed. That
script then failed its expected generated-locale drift check; it is not recorded
as an entirely successful script. After the generated catalog commit, the
separate locale extraction/strict compilation/drift check passed. Protocol export
and drift checks passed using actual exporters. The 44 offline workflow-helper
tests also passed; their simulated network fixtures do not qualify hosted CI.

The first retirement compilation failed with ENOSPC before any tests ran. Its
log and task-cache prune manifests remain retained, alongside later passing
runs. No test qualification is assigned to that attempt. The existing portable
JSON now records exact source hashes, per-cohort counts, named cases, log hashes,
generated-contract hashes and the failed-attempt/prune provenance. Earlier
records, including paced capture evidence and failed historical runs, remain
unchanged. The later all-target check also passed on exact `0833c8ccf9` in
20.21 seconds, retaining the existing nine desktop-library and four test
warnings. No optimized retirement QA build or actual retirement UI run is
qualified by this checkpoint.


## Hosted checkpoint `053447421f`, terminal 2026-11-09

All six PR workflows were terminal at **08:54:44 UTC**. Backend, native Windows,
CLI, formatting/lint and security passed. Jobs checked out merge
`65df6a3223c1af40bf1f11680cd56c626d0599bd`, whose parents are base
`a6a8f001d9165881e59fd0fa29df944da26b8801` and exact branch head
`053447421f2333737fd720c7905b8bd51cad447e`.

Each Windows architecture logged **2,250 passing executions, zero failed and
47 ignored across 70 cohorts**, including the four standalone Git helpers and
four release-policy cases outside Cargo. Both listener libraries passed all
91 tests (x64 1.78 seconds; ARM64 1.67 seconds), including the new simultaneous
same-label fixture regression, recovery-mask case and maintenance collision case.
Both SDK refresh regressions, all three Local-principal cases and all five
empty-fence cases passed on each architecture. Both NSIS builds and installed
app, CLI/MCP and uninstall smoke checks passed. Portable artifact metadata
preserves the exact x64 and ARM64 artifact IDs, sizes and digests.

Linux headless passed **1,666/0/19** across 22 cohorts, including all three new
production-router append HTTP cases. Windows does not select the server package,
so those cases are not claimed there. Packaged Linux CPU and container checks
passed. With total CPU capacity two and one reserved live slot, the overlap case
emitted its first caption after 6,853 ms while the batch owned the model,
completed the concurrent HTTP request in 10 ms and recorded
15,092 ms of actual drain. Duplicate printed exact-selection summaries are not
counted as additional test executions. CLI passed **274/0/2**. Frontend passed
**2,622 desktop tests in 298 files**, **303 editor tests in 31 files**, and typechecking.

macOS jobs `129031492297` and `129034442435` both stopped during `cargo check`
when SwiftPM received HTTP 504 for the pinned SpeechCore v0.0.14 archive, before
workspace tests. Between attempts, one bounded official GET returned 2,112,822
bytes matching the pinned SwiftPM checksum. That local availability proof did
not establish runner availability. Only the failed macOS job was retried once;
the frontend's copied attempt-2 job has identical original timestamps and steps,
proving its successful execution was carried over rather than repeated. Both
failures, the preflight and the exhausted retry remain preserved. No third attempt,
healthy-job cancellation or manual speech dispatch occurred.

Record `hosted_0534474_terminal_2026_11_09` embeds the terminal summary and seven
hashed supporting receipts, including raw-log hashes, source equivalence,
configured CPU evidence and both failure attempts. Original direct-`36` Windows
speech success remains source-equivalent processing evidence through `053`;
later retirement code is outside that comparison. CI prewarm `1101693d05` is
later source with separate offline validation; its hosted cache consumption and
the previously unreached macOS workspace cohort remain pending. This checkpoint
does not qualify that helper or the later retirement implementation.

The user explicitly deferred production HTTPS SSO acceptance. Earlier statements
that staging configuration was required remain historical; this deferral is not
a production SSO pass and does not change prior synthetic-login evidence. Physical
capture, remaining activation gates and the observed model-quality failures keep
their separate scopes. No merge or release is authorized.

## Short Local physical capture and explicit desktop pause, 2026-11-09

After the user approved QA microphone and system-audio access, the isolated
`io.loofah.separation-qa` bundle (display name “Loofah Local Performance QA”)
exercised the actual Logitech StreamCam microphone and system-audio inputs.
No OS permission prompt or setting was changed during these captures. Execution
belongs to clean source `aa0a5ed1c70ac6ecd767e2bebc8567ea4d7f142d`, executable
SHA-256 `bd94f90970b421e9f35c586f2f68f741c18eab5b3d9dff52f6454b81039c5dd8`.
The reused disposable vault indexed 5,003 sessions, 6,672 notes and 2,502
transcripts. Local startup displayed no login or SSO flow.

Session `b94b1b04-f3bb-46f8-9c06-776b4b12e845` captured twice using Quantized
Whisper Base in English batch mode. The first MP3 was 98.892 seconds; Resume
extended the same recording to 194.184 seconds, excluding the intervening pause.
Both are 16 kHz stereo with finite nonzero decoded signal on both channels and
no NaN/Inf samples. The controlled English2 system-audio fixture appears around
55–72 seconds in the first transcript and 55–72/116–133 seconds after resume.
The batch transcript grew from 100 to 225 unique word IDs; all original IDs were
replaced, so neither ID/prefix preservation nor frame-perfect PCM continuity is
claimed. The first capture's owned files remain retained separately.

Note editing stayed available during capture. Resume opened Note; Summary
displayed its recording read-only notice and returned to editing after completion.
Both stops finalized, both Whisper batches completed (2.828/3.853 seconds), and
both automatic summaries saved. Tiny Qwen summary accuracy/conciseness failed;
only functional saving is qualified. Quiet microphone text is excluded from the
report. Controlled spoken microphone accuracy, live/long physical capture,
Windows physical capture and the full recording UX matrix remain unqualified.

Read-only analysis retained 319 independent five-second performance windows.
Wholly contained capture windows numbered 19 initially and 18 during resume;
their highest individual frame-interval p95 was 18 ms, with maxima 45/33 ms.
Across the broader capture/postprocessing interval, eight windows contained 166
input-to-next-frame samples: highest individual window p95 18 ms, maximum 20 ms.
These are paint-feedback measurements, not save latency or an aggregate p95.
The selected intervals precede compilation. They do not qualify the full
concurrent capture/upload/search/AI or controlled-baseline responsiveness gate.

Exact file/tree comparisons identify unchanged Local production paths between
AA0 and `94c0cf6b90`; the portable record contains those hashes and relevant
Remote/test/generated exceptions. This supports source attribution only, not
execution of the 94 artifact or whole-head equivalence. All canonical files
stayed unchanged during evidence analysis. The user's later instruction paused
desktop tests until their signal; both idle QA apps were stopped. No further
desktop testing, capture, launch or permission change occurred. The
physical stdout has no separate owner-shutdown receipt, so no additional drain
qualification is inferred from stopping the idle process.

## Real HTTP media retirement and same-source restart, 2026-11-09

Immutable server `94c0cf6b9005701c09942b36bfe5ec6cf8b1c560`, SHA-256
`e38a9605c88f6f79d74054632b9acd15c591ecda5ef97f799cf36783db58259a`, passed
27 actual authenticated TLS requests for a new synthetic two-word SRT with no
summary declaration. The original download matched its 49-byte source and
SHA-256 before fsynced no-clobber publication. An explicit Exported decision was
persisted before POST. All three retirement POSTs used the same UUID: initial,
identical replay, and replay after graceful restart of the same binary. They
returned the identical Retired receipt with quota released. Original terminal
status, result and skipped-summary receipt remained unchanged; no model job or
inference was created.

The dispatcher upload file and active admission were removed. Its empty source
directory, canonical content, lower `source.srt` and operation/result journals
remain intentionally. All 40 preexisting fixture storage files, including both
historical summary jobs/results, stayed byte-identical. All 53 current files
survived restart unchanged. Original-source GET returns 503 Unavailable after
retirement. The first harness assumed 409 and later assumed the empty directory
would disappear; both test assumptions are preserved and corrected without a
new import or inference. This is not total disk erasure.

The initial task-only capability proxy failed by touching its already-closed
socket after fully consuming Content-Length. Its backed-up version reproduces
EBADF; the completion fix passes all nine offline helper tests. Actual TLS
capabilities then returned 200/1,032 bytes versus the real upstream's
200/1,013 bytes, with only `media_retirement` added. The overlay was disabled
after acceptance, and the production capability remains absent. No production
source, trust or authentication setting changed for this fixture correction.

The portable record embeds the headless receipt and helper repair/restart
provenance. Direct Python HTTP acceptance supplements the Core/route tests; it
does not qualify Native SDK export or the paused retirement UI. All owned QA
desktop apps, fixture services and task-local Ollama have since stopped, while
their data, models, binaries and logs remain preserved for the user's signal.

## Completed retirement through stopped backup/restore, 2026-11-09

The same immutable `94c0cf6b90` server passed actual maintenance CLI `backup`,
`verify-backup` and `restore`, followed by authenticated TLS replay from a fresh
restored storage root and initially absent cache. CLI exits were zero in
0.384/0.013/0.512 seconds. Main and required private snapshots pair by UUID and
SHA-256; provider credential paths occur only in the owner-only private bundle
(13 entries including directories), with no credential values in the report.

All 53 original service files and directory modes stayed unchanged. The 52
restored payload files and modes match exactly; the excluded ephemeral service
lock was recreated at startup. Configuration changed only storage/cache roots.
The completed retirement GET and exact original-UUID POST returned the same
receipt, terminal status, retained result and skipped-summary record. Quota
remained released; original source stayed unavailable, and no new model job
appeared. Canonical and historical job/result hashes remained unchanged. The
fixture capability overlay stayed off throughout. Both temporary processes
stopped normally with helper exit zero; all three fixture ports were free.

This qualifies same-version stopped restoration of a completed retirement on
the recorded macOS artifact. It does not establish cross-version upgrade,
Windows/Linux execution or interrupted-retirement restoration. No desktop
testing or inference resumed. Exact commands, manifests, inventories, hashes
and HTTP receipts are copied into the existing portable evidence record.

The published Astro documentation also built from `6d070f083` with 36 pages and
all internal links valid. This is the website build scope, not a claim that the
plan files are published routes. Desktop QA remains user-paused; production
SSO remains explicitly deferred.

## Summary-source audit and existing-title heading gap, 2026-11-09

A source/receipt review of the retained synthetic notes-only job found no
demonstrated migration cause for its inaccurate, overlong output. The four
shared template assets match baseline `a6a8f001d` and executed server `080764fe20`.
Both paths omit summary temperature and pass an 8,192-token ceiling and
`max_retries=4`. Their transport retry policies are deliberately not identical:
the shared HTTP adapter retries explicit 429 rejection only and reports ambiguous
dispatch interruption without replay. Both allow at most two heading-validation
attempts. The preserved provider log records temperature one, 577 output tokens
and no truncation, with a cancelled heading attempt and retry in one durable job.
No raw historical provider request was retained, so this is not byte-for-byte
prompt reconstruction. Model accuracy and conciseness remain failed acceptance.

The review did identify a separate presentation mismatch. Legacy
`ensureMarkdownFirstLineTitle` prepends the current existing session title when
the generated H1 differs. Shared generated publication currently retains the
provider heading, while the optional title stage returns AlreadyNamed. This
does not explain fabricated facts. A source-only fix is assigned to restore
the existing-title rule under checked publication, preserving document/title
conflicts, bounds and compound task/tag recovery. It is not yet integrated or
qualified. The portable record preserves concrete template/source hashes,
provider-log provenance, limits and the open gap. No new inference or desktop
testing was performed for this audit.

The completed second physical batch's six owned canonical files and four existing
log/analysis files are now copied into an immutable pause snapshot, verified by
manifest SHA-256 `f6e453031d044dafe3b549435ba13e5adc0a55a44846b1410da7cd0d876cec34`.
The earlier first-capture snapshot remains separate. The provider log used by
the source audit also has a frozen copy, preserving its observed hash if future
authorized QA appends to the runtime log. These were read-only source copies;
no canonical file, desktop process or inference was started or changed.

## Existing-title generated publication correction: source checkpoint

Source through `c9d034622d` integrates the existing-title parity correction.
Generated summary and template publication captures the authoritative session
title, applies the existing Markdown stamping rule, checks the complete stamped
size and parses that Markdown outside the final exclusive write fence. The
existing body/task/tag transaction then compares both document revision and
title before new migration, intent or publication. A rename after preparation
returns Conflict; unrelated metadata edits remain accepted. Blank-title handling
and optional title-generation policy are unchanged. No second AlreadyNamed write
or new inference is introduced.

Raw retained model results remain raw. Canonical content and any body-applied
receipt identify the stamped bytes; interrupted compound recovery uses the
already-admitted bytes and preserves later user metadata edits. Source adds ten
regression functions, including a four-row real-worker target/policy matrix,
rename races, no legacy migration on conflict, stamped bounds and task extraction.
Existing crash fixtures distinguish generated from published bytes. Formatting
and source review are complete; execution and integrated checks remain pending.
Desktop app testing stays paused, all activation gates remain off, and observed
model-quality failures remain unchanged.

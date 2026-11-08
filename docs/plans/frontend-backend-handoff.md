# Frontend/backend separation handoff

## Status: paused for transfer to another workstation

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

## Current behavior and important limitations

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
are retained; automatic device cleanup is disabled.

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

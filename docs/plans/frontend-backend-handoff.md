# Frontend/backend separation continuation

## Current state

The user resumed all work. Goal: fully implement, test and validate
`docs/plans/frontend-backend-separation.md`. This checkpoint does not claim
completion. All seven delivery phases remain open.

- Integration worktree: `/Volumes/T7/src/loofah-backend-separation`.
- Branch: `refactor/frontend-backend-separation`.
- Last source checkpoint: `dc32e18570d3d7dfa9d42324b75956ba2cf165c9`.
- Last pushed head: `66b88b660e86efb53a5938e45e872769d0f74153`.
- Draft PR: <https://github.com/bart6114/loofah/pull/123>; no merge or release.
- Original `/Volumes/T7/src/free-meeting-transcriber` remains on main with tracked
  files untouched. Preserve its pre-existing untracked documentation.

Verify live HEAD, status, processes, agents and CI before resuming. Default local
use remains standalone, offline and login-free, including solo use. Preserve
macOS and Windows 11 x64/ARM64; Linux is headless only. Exclude bases, CRDTs,
general remote-vault offline editing, mobile and real-time collaborative editing.
Every local build artifact, cache, temporary file, log and model stays physically
on T7. Use isolated worktrees.

## Implemented since the previous pushed batch

The main plan records earlier extracted operations, HTTPS/SSE, scoped clients and
drafts, checked editing, media/attachments, processing and CLI/MCP remote reads.
New integrated work includes:

- Lossless title metadata patches, optional durable summary/title orchestration,
  exact-policy device recovery, capability gates and actual generated contracts.
  Preserve omits the new field exactly; title failure never discards a successful
  body. Retained generated output can differ from the subsequently titled body.
- Verified streaming WAV source and absent-only recording publication, complete
  bounded receipt discovery/recovery, Core media readiness gates and fresh
  authorization through canonical commit. HTTP/SDK publication is integrated.
  Source audio is retained; independent canonical reads are still required before
  any future device cleanup policy.
- CPU speech resource ownership around the existing kernel: explicit verified
  operator model, bounded batch/live admission, actual native/source/model drain,
  lossy optional batch progress and complete authoritative terminal result.
  This is not a public STT job or completed typed live inference feature.
- Actual main New Note, slim linked-session summaries, balanced Scope QueryClient
  mounting, checked Date editing, shared author metadata reads and scoped derived
  transcript/export caches. Legacy source reads and rich main editor paths still
  require migration; never replace their local facade with a global active router.
- A capture test now waits for the specific retryable spool lease release after
  shutdown. It preserves every original manifest/hash assertion.

Important evidence under `/Volumes/T7/loofah-backend-build/`:

| Check                     | Evidence                                                                                                                                                                                                         |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Optional title stage      | `agents/title-stage/logs/core-stage-test-provenance-final.json`; final jobs 128 pass plus 8 crash children                                                                                                       |
| Lossless title metadata   | `agents/title-metadata/logs/test-provenance.json`; 101 focused pass                                                                                                                                              |
| Recording recovery        | `agents/capture/publication-recovery-verification.json`; 44 focused pass                                                                                                                                         |
| Core capture publication  | `agents/capture-publication/logs/verification-final.json`; 58 selected pass                                                                                                                                      |
| CPU resource owner        | `agents/capture/logs/stt-resources-verification.json`; 17 ownership cases                                                                                                                                        |
| HTTP publication          | `logs/capture-publication-api-provenance.json`; 6 pass including lost reply/replay/independent read at 50/150/500 ms RTT                                                                                         |
| Title HTTP/protocol       | `logs/generated-title-api-provenance.json`; real lost-reply title behavior and actual export checks                                                                                                              |
| Main frontend             | `logs/main-session-headers-integrated-tests.log`, `main-session-date-integrated-tests.log`, `main-session-author-integrated-tests.log`, `transcript-cache-integrated-tests.log`, `date-review-wording-tests.log` |
| Normal native check       | `logs/publication-speech-native-check.log`; pass with 31 known unwired capture warnings                                                                                                                          |
| Headless dependency guard | `logs/publication-speech-headless-dependencies.txt`; no Tauri/device dependencies                                                                                                                                |

Actual generated files at this checkpoint:

- `apps/desktop/src/types/tauri.gen.ts` SHA256
  `2fe77c7e8ea0bdc67730062c5955bb3e322d0a4dcf535e3f9183e3c70c4a5ab4`.
- `apps/desktop/src/shared/application/protocol.gen.ts` SHA256
  `3537e8aba7ffaba920ddfbfbae0f42486cf8cdbee4e05dd2981339f3a87e7624`.

Do not hand-edit generated bindings. For TitlePolicy, serde `default` must follow
its custom `skip_serializing_if` attribute so Specta exports the field optional.

## Active distribution and build windows

- A (`frontend_distribution`): overview API integrated as `dfe918246`, actual
  generated contracts as `238c02a66`, frontend as `dc32e1857`. Worker 196 tests and
  actual-binding typecheck pass; Root 93 targeted UI tests, 10 API tests, one
  native login-free test, all-target desktop/CLI/Core/client/server checks pass.
  A now audits a coherent main Enhanced editor/task migration read-only.
- B (`backend_distribution`): first transcript publication integrated as
  `87997f6d5`; Store 83 selected tests pass. Source-only lifecycle fix is frozen
  in `/Volumes/T7/src/loofah-runtime-ownership`, branch
  `fix/backend-runtime-ownership`, base `66b88b660`. B has the exclusive existing
  target-tauri validation window. Root must not run native-cache Cargo until B
  explicitly releases it. C's target-root window remains independent.
- C (`capture_distribution`): typed f32 live facade in
  `/Volumes/T7/src/loofah-separation-typed-live`, branch
  `refactor/backend-typed-live`. Final frozen source snapshot
  `41222a235741ca71a81396d504084f99c31dbfab117d0c348ed4effc4c23f4c4` passed
  69 unit tests/eight ignored and one opt-in CPU test with two independent live
  sessions. Final check/Clippy remain in progress; no integration or public STT
  claim yet. C exclusively owns target-root until explicitly released.
- Root: reviewed integration, exports, frontend checks/docs/CI. Portable mapper
  extraction integrated as `9f11e7a16`: production mapping text unchanged, six
  pure mapping cases plus original Soniqo integration case pass, package checks
  pass. No new endpoint. Source Cargo edges add fs-format and serde to existing
  transcribe-core. Root currently has no Cargo or test process running.

New proof files under `/Volumes/T7/loofah-backend-build/logs/`:
`restore-test-provenance.json` (stopped full-storage restore, cold cache, preserved
identities/access/receipts/unknown files and resumed sealed publication),
`transcript-publication-integrated-provenance.json`,
`transcript-overview-test-provenance.json`,
`transcript-overview-native-provenance.json`,
`transcript-overview-check-provenance.json`,
`transcript-overview-integrated-ui-tests.log` (90 cases),
`transcript-overview-integrated-export-tests.log` (three cases),
`transcript-overview-integrated-ui-typecheck.log`,
`shared-transcript-mapper-provenance.json`.
Worker overview logs: `agents/transcript-overview/logs/`.

Source must stay frozen through each dependent build. Require compiler artifact
`fresh=false`, own source path and dep-file, exact nonzero new-test inventory,
source hashes and executable SHA. A zero-match test result is not verification.
Never cancel healthy builds or wipe/bypass shared caches to avoid locks. macOS
binary assessment can stall compilation or launch for minutes; inspect first.

## CI and next integration batch

The last fully green head remains `93dada10bb592c824ae59eeff72187b5079a9558`,
including Windows x64/ARM64 installers. The aade batch finished with one shared
capture-fixture lease-release race on macOS and both Windows architectures. Its
bounded test-only fix passed 28 local capture cases and is included in the new
pushed `66b88b660` batch.

For 66b, four workflows are green: backend-ci (headless, container and Linux CPU
STT), cli-ci, ci and zizmor. Desktop frontend is green; macOS Rust and Windows
x64/ARM64 remain active at this checkpoint. Keep healthy CI running and batch
subsequent pushes after Windows finishes. Do not count the skipped Windows
speech-model lane or prerecorded CPU tests as hardware/real-device acceptance.
Draft PR #123 body/head was verified at 66b; update after the next validated push.

## Remaining acceptance gates

Main profile selection and remote-only startup; remaining rich editor/tasks,
transcript/people/content/import/export and CLI-owner/mutation migrations;
owner-managed provider credentials; trusted device capture source/uploader,
public controls, quit ownership, pending-note/append recovery; authenticated
live/batch STT, checked transcript promotion and portable diarization; verified
source retention/cleanup; restore/upgrade/downgrade and full platform QA.

The current service retains raw capture sources under a 2 GiB/64-capture quota per
vault. Publication does not reclaim it. Continuous-use recording needs verified
retention/cleanup and replay semantics after pruning. Do not silently delete the
only recoverable audio. Recording queues are distinct from offline remote editing.

Production macOS responsiveness, real capture/STT, realistic 50/150/500 ms networks,
disconnects, two-hour recordings and Windows native x64/ARM64 remain required.
The lifecycle fix must distinguish existing detached Service initialization
from direct open/Drop paths. Systemd retains its 60-second stop budget; forced
termination cannot guarantee in-memory legacy transcript durability.

## Preserved recovery state

Pause archive: `/Volumes/T7/loofah-backend-build/pause-2026-11-07T093537Z/`.
Stash `d13afbb1cb44a7939c3a5755458f97652e3630b1` was already restored,
hash-verified and incorporated in 0868995b4; do not reapply it. Preserve other
pre-existing stashes without treating them as instructions.

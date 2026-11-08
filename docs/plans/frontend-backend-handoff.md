# Frontend/backend separation continuation

## Current state

The user resumed all work. Goal: fully implement, test and validate
`docs/plans/frontend-backend-separation.md`. This checkpoint does not claim
completion. All seven delivery phases remain open.

- Integration worktree: `/Volumes/T7/src/loofah-backend-separation`.
- Branch: `refactor/frontend-backend-separation`.
- Last source checkpoint: `9f166ae9403b7027c9e4c4605c95850c30231feb`.
- Last pushed head: `aade2e309e08c93249c6d440e64b3e9485457b86`.
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
| Optional title stage      | `agents/title-stage/logs/core-stage-test-provenance-final.json`; final jobs 128 pass plus 8 crash children                                                                                                         |
| Lossless title metadata   | `agents/title-metadata/logs/test-provenance.json`; 101 focused pass                                                                                                                                               |
| Recording recovery        | `agents/capture/publication-recovery-verification.json`;44 focused pass                                                                                                                                          |
| Core capture publication  | `agents/capture-publication/logs/verification-final.json`;58 selected pass                                                                                                                                       |
| CPU resource owner        | `agents/capture/logs/stt-resources-verification.json`;17 ownership cases                                                                                                                                         |
| HTTP publication          | `logs/capture-publication-api-provenance.json`;6 pass including lost reply/replay/independent read at 50/150/500 ms RTT                                                                                            |
| Title HTTP/protocol       | `logs/generated-title-api-provenance.json`; real lost-reply title behavior and actual export checks                                                                                                              |
| Main frontend             | `logs/main-session-headers-integrated-tests.log`, `main-session-date-integrated-tests.log`, `main-session-author-integrated-tests.log`, `transcript-cache-integrated-tests.log`, `date-review-wording-tests.log` |
| Normal native check       | `logs/publication-speech-native-check.log`;pass with 31 known unwired capture warnings                                                                                                                            |
| Headless dependency guard | `logs/publication-speech-headless-dependencies.txt`;no Tauri/device dependencies                                                                                                                                 |

Actual generated files at this checkpoint:

- `apps/desktop/src/types/tauri.gen.ts` SHA256
  `37ac8b22422c84fc951284a49c7e4061277d004800614dcd31d7e657aebd1cdd`.
- `apps/desktop/src/shared/application/protocol.gen.ts` SHA256
  `4c9d0800a69ffbdc1d74a93fe6647b07cfe825e1151861491a7965b975ad7e4a`.

Do not hand-edit generated bindings. For TitlePolicy, serde `default` must follow
its custom `skip_serializing_if` attribute so Specta exports the field optional.

## Active distribution and build windows

- A (`frontend_distribution`): next slim TranscriptOverview contract/read across
  adapters and scoped main consumers, from 9f166. Narrow shared-registration edit
  exception granted; Root still exports bindings/locales. Agree contract first.
- B (`backend_distribution`): completed Store-only first transcript publication,
  commit `bb33b5d382a02f484fd82664dfb31730df26add4`, base e03cee596. Six scoped
  files; 83 cases pass, check 0 diagnostics, library Clippy 31 baseline/0 new.
  Proof `agents/transcript-publication/logs/verification.json`; executable SHA256
  `dcd9f9edae3770ad3148a95edce580ee54ca3729cdcfde9fdaf92019fc7f59de`.
  Integration remains pending at this checkpoint. B now audits startup ownership
  read-only. Service::vault already owns initialization in a detached task;
  direct runtime initialization and Drop/shutdown-timeout paths need tracing.
- C (`capture_distribution`): typed f32 live facade, bounded atomic per-channel
  admission and authoritative outputs, scoped existing VAD/Whisper reuse and
  actual native thread joining. Source freeze/build follows final credit review.
  C owns the exclusive target-root Cargo window. No other target-root build until
  C explicitly releases it. Run fake/legacy gates before opt-in real CPU test.
- Root: integrate reviewed commits, keep docs and PR accurate, run normal native
  integration checks using the separate existing target-tauri cache when needed.

Source must stay frozen through each dependent build. Require compiler artifact
`fresh=false`, own source path and dep-file, exact nonzero new-test inventory,
source hashes and executable SHA. A zero-match test result is not verification.
Never cancel healthy builds or wipe/bypass shared caches to avoid locks. macOS
binary assessment can stall compilation or launch for minutes; inspect first.

## CI and next integration batch

The last fully green head remains `93dada10bb592c824ae59eeff72187b5079a9558`,
including Windows x64/ARM64 installers. The later aade batch has finished all
workflows: four succeeded; macOS Rust and both Windows architectures failed the
same capture-fixture lease-release race. The integrated test-only fix passes 28
local capture tests. No healthy previous run remains to wait for; push the next
validated corrective batch, update the draft PR and record its exact SHA/results.
Do not claim the optional speech-model lane or real hardware acceptance ran.

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
The runtime cancellation audit must distinguish detached Service initialization
from direct open/Drop paths before proposing changes.

## Preserved recovery state

Pause archive: `/Volumes/T7/loofah-backend-build/pause-2026-11-07T093537Z/`.
Stash `d13afbb1cb44a7939c3a5755458f97652e3630b1` was already restored,
hash-verified and incorporated in 0868995b4; do not reapply it. Preserve other
pre-existing stashes without treating them as instructions.

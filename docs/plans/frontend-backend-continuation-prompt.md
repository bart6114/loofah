# Continuation prompt for another workstation

Copy the following prompt into a new coding session with access to the repository
and the T7 drive. It authorizes resuming the paused implementation. The source and
handoff are on [draft PR123](https://github.com/bart6114/loofah/pull/123), branch
`refactor/frontend-backend-separation`; no old workstation session is required.

---

Resume Loofah's frontend/backend separation implementation from draft PR123:
https://github.com/bart6114/loofah/pull/123
Repository: https://github.com/bart6114/loofah
Branch: refactor/frontend-backend-separation

The objective is to fully implement, test and validate
`docs/plans/frontend-backend-separation.md`. This is an unfinished implementation,
not a new plan-only task. It was explicitly paused on the previous workstation
for this transfer. Continue autonomously; do not narrow the goal or claim it done.
Do not merge the PR, publish a release, bump versions or modify the user's vault.

First fetch the current PR branch and establish a clean ISOLATED worktree. Inspect
existing checkouts before creating anything; preserve user changes. Do not work
on main. Read repository/nested AGENTS.md instructions, then these files:

1. `docs/plans/frontend-backend-handoff.md` — authoritative current state,
   integrated commits, validation gaps, exact pending work and machine differences.
2. `docs/plans/frontend-backend-separation.md` — full target and release gates.
3. `docs/plans/frontend-backend-distribution.md` — prior ownership/distribution.
4. `docs/plans/evidence/frontend-backend-checkpoint-2026-11-08.json` — portable
   copied validation records and exact source/provenance. Old absolute artifact
   paths and original agent commit IDs in these records are historical, not
   dependencies or instructions to reapply already-integrated changes.

Preserve these constraints throughout:

- The default single-person/same-device Local desktop stays standalone, offline
  and LOGIN-FREE. No account, SSO or separately installed service is required.
- Explicit Remote uses HTTPS plus authenticated notifications and NEVER falls
  back to a Local vault. Keep UI requests/subscriptions transport-independent.
- One shared Rust application layer; thin IPC and HTTPS adapters. Vault files
  remain the source of truth. Preserve format, atomicity, revision/conflict
  checks, attachment ownership and unknown/dot-file exclusion.
- Preserve macOS and Windows 11 x64/ARM64. Linux is for the headless server only.
  No bases, CRDTs, mobile, real-time collaboration or general remote offline sync.
- ALL build artifacts, caches, temp files, models, test output and logs must
  physically reside on T7. Inspect its mount path on this workstation; do not
  silently build on the internal disk. Set LOOFAH_TARGET_CACHE to a T7 directory
  before `scripts/setup-shared-target.sh` and verify both target destinations.
  Set TMPDIR, NPM_CONFIG_CACHE, DPRINT_CACHE_DIR, CLANG_MODULE_CACHE_PATH and
  SWIFTPM_MODULECACHE_OVERRIDE on T7 as applicable. Adapt paths to this platform.
- Use one stable absolute Native build worktree/feature graph and reuse caches.
  Freeze dependent source during builds. Do not run competing Native builds.
  Batch focused Rust test filters after `--` into one run; libtest OR-matches them.
  Use Cargo's runtime environment for Native tests, including the existing Codex
  probes; do not skip failing tests or infer Windows validity from macOS.

The latest source checkpoint includes every agent's owned work; DO NOT reapply
agent commits or restore old stashes. There are explicit unfinished source slices:

- New processing SDK adapters/tests: integrated `e2958c78d`, SOURCE ONLY, four TLS
  cases unexecuted. New Local CLI/MCP routing: integrated `4c819d8bc`, its own
  no-run compile passed; new test/check/Clippy execution remains pending. Import/
  transcribe parity and Native owner endpoint composition remain incomplete.
- Core/server capture processing is integrated and independently validated, but
  Native/device durable declaration, upload trigger, processing/caption UI and
  capability/export pairing are unfinished.
- Generated protocol/Native bindings must be regenerated with the actual
  exporters after the latest SDK enum/export-registration changes; never
  hand-edit generated files. Oxfmt Native/plugin `.gen.ts` directly; the protocol
  exporter owns its own output. Full integrated-head validation is still needed.
- Local shared delete/undo activation is still DEFAULT OFF pending complete
  Native producer/persistence and Windows acceptance. Do not enable it just
  because a source checkpoint or no-run compile exists.

Start with live PR/CI and the pending SDK/CLI/native validation, using the
run-tests and axum-test-transport skills. The prior full frontend cohort passed
2,563 tests/279 files plus typecheck/locales; that is scoped historical evidence,
not a pass for every later integrated change. The final combined run was stopped
for the pause after compilation and 46 observed passing cases; it is not a full
pass. Diagnose current failures and complete the pending checks before broadening
changes. The handoff documents the corrected macOS/Windows fixture failures.
Use `--locked`; do not require `--offline` when this workstation lacks deps.

Then finish the full plan, prioritizing durable explicit device processing
intent/SDK integration, complete Local CLI MP3/import/transcript replacement and
managed owner composition, provider/settings parity, main Remote activation,
retention/backups/upgrades and realistic capture/network/platform acceptance.
Raw recording capture/upload must survive optional AI/network failure. Never
infer an AI declaration for historical recordings, replay unknown paid work,
retarget a capture or overwrite changed canonical content.

Use up to three subagents for disjoint reviewed work packages if available.
Agree file ownership and coordinate shared Cargo caches; new agents must read
this handoff and use isolated worktrees on T7. Do not let parallel work create
competing application logic or duplicate inference/validation loops. Keep
concise progress updates and save meaningful checkpoints with honest validation.
Ask only for an actual blocker that cannot be resolved from repository/context.
No credentials, models, binaries, running agents or raw logs from the previous
workstation are required to begin. The optional real CPU model/hash/reproduction
is documented in the handoff; configure it separately when that test is needed.

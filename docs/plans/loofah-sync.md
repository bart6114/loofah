# Loofah Sync: invited signup and desktop staging

## Recorded status — 17 September 2026

This is an internal implementation plan, outside the public documentation navigation.
The following is the supplied baseline; verification below distinguishes newly checked results.

- [PR #73](https://github.com/bart6114/loofah/pull/73) is a draft; all CI checks passed.
- The account frontend and API are deployed at `staging-app.loofah.io`; signup is disabled.
- EU D1 and private R2 resources exist for staging and production. Production application deployment and migrations are outstanding.
- Cloudflare Email Sending delivered a message from `accounts@notify.loofah.io` to Bart’s Gmail inbox.
- Foundations include encrypted objects/manifests, quota reservations, conditional revision commits, enrollment signatures, coherent snapshots, journaled local application and reusable recording objects.
- Previous verification passed the staging upload/edit/restore roundtrip and the 7.6 GB / 9,000-file local restore fixture.
- Desktop orchestration, trusted-device pairing, history/conflict UI and full browser signup are unfinished. Operational recovery, account deletion and external-beta review remain later gates.

## Milestone outcome

Bart can install Loofah Staging alongside normal Loofah, create an invited account, enroll two Macs using recovery import or trusted-device pairing, sync copied test data, resolve conflicts and restore history. Production signup and external invitations remain closed.

## Delivery milestones

1. Record status and a dated verification checklist here.
2. Enable invitation-only staging signup: retain Better Auth, database-enforced invitation consumption and the 25-account cap; add an authenticated administrator CLI for invitations and capacity; use hashed 256-bit tokens, normalized emails, seven-day expiry and fragment links; reissue invalidates unused invites and sending is explicit. Provide friendly invitation and verification states, generic reset/resend responses, idempotent activation and a signup kill switch. Verify staging bindings, secrets and migrations, deploy, and create Bart’s invitation.
3. Connect the staging desktop only: separate settings, Keychain, sync state and updater; explicit consent in Settings → Sync; typed Rust-owned credentials, enrollment, I/O and network commands, coalesced display-safe status, browser authorization and cancellable polling. Disconnect preserves local content. First enrollment requires saving and reimporting a recovery kit and durable Keychain storage; recovery import uses signed one-time challenges and never replaces missing roots automatically.
4. Trusted-Mac pairing: use the Rust Magic Wormhole implementation and its mailbox protocol over an authenticated Worker WebSocket and explicitly EU-jurisdiction Durable Object. Require the same verified account and an active enrolled approver; bind approval to the exact pending identity. Transfer keys/checkpoint through the encrypted channel, persist before acknowledgement, expire after ten minutes, bound attempts/messages and permit one completion. Reject substitution/replay/cross-account/revoked approvers; revocation invalidates bound sessions and approvals.
5. Persistent synchronization: versioned atomic state/journals outside the vault, account/vault binding, baselines, operations, spool, checkpoints and cursor. Recover apply journals before indexing; publish coherent changes before cursor advancement. Schedule committed changes, finalized recordings and explicit deletions; reconcile on startup/reconnect/wake/every five minutes and poll every ten seconds with bounded backoff. Preserve ten-second idle/sixty-second continuous checkpoints, two transfers and 8 MiB multipart parts. Hash/encrypt/network outside the transaction guard. Preserve all supported components, global records, unknown JSON fields and excluded files. Missing state/files never imply deletion; initial divergence creates conflicts; changing vault/account suspends work.
6. Conflicts and history: display progress, last success, quota, conflicts and actionable errors; preserve local work on all failures and permit downloads/recovery at quota. Recovery-generation mismatch pauses for explicit reconciliation. Preview/export and explicitly select local/cloud after preserving both revisions, handling globals separately. Session history fetches on demand and shows time/device/locally computed changes; whole-session restore preserves unsynced work, verifies content, conditionally commits a new head and guarded-applies it, leaving globals unchanged and surfacing missing references. Logical expiry/purge preserves current/conflict pins; physical deletion stays disabled.
7. Run checks and focused integration tests, dispatch the staging workflow from the implementation branch, and provide the artifact, invitation link and two-Mac walkthrough here. Run the large restore fixture and responsiveness QA in the signed staging build.

## Verification checklist — 17 September 2026

Unchecked items are unverified, not completed.

- [x] Invitations reject missing, mismatched, expired, revoked and replayed tokens.
- [x] Concurrent signup cannot reuse invitations or exceed capacity.
- [ ] Real staging browser signup, verification, login, resend and reset.
- [ ] Both enrollment methods across two Macs, including cancellation, expiry, restart and Keychain failure.
- [x] Note edits reuse recordings; concurrent edits preserve both revisions; restore creates a new head (two authenticated staging clients on this Mac).
- [ ] Desktop/CLI creation, explicit deletion, global registry changes and excluded-file preservation.
- [ ] Lost events, missing metadata, interrupted uploads/applies, disk exhaustion and changed recovery generations preserve content.
- [x] Full-quota deletion and metadata-only restore; logical purge frees quota without physical deletion (automated cloud storage tests).
- [ ] Tampered ciphertext, unsafe paths, cross-account requests, replayed enrollment and revoked sessions are rejected.
- [x] Formatting, applicable TypeScript/Rust checks, cloud/runtime and integration tests.
- [x] Repeat representative 7.6 GB / 9,000-file restore fixture.
- [x] Recording, transcription and search responsiveness in the signed staging build (synthetic 1,000-session vault; see evidence below).
- [ ] Staging artifact, private invitation delivery and two-Mac walkthrough.

## Deferred gates

Production deployment, external invitations, account deletion, physical garbage collection, backup/export and disaster recovery, operational monitoring, independent security review and external-beta QA. Billing and root rotation are outside this milestone.

## Implementation log — 17 September 2026

### Invited signup

- Added `pnpm -F @loofah/cloud admin staging invite EMAIL [DAYS]`, `list`, `revoke HASH`, `capacity [LIMIT]` and explicit `send EMAIL < private-link-file`. Administration uses existing Wrangler credentials and the Cloudflare administrator API; production administration is rejected by this milestone’s CLI.
- Migration `0007_invitation_revocation.sql` is applied to staging. Invitation reissue revokes earlier unused invitations in the same database transaction; consumed invitations remain unchanged.
- Staging was deployed with invited registration enabled, version `7cde7575-3343-4ec1-aaaf-40d8cdebe7cb`. Production remains closed. Staging bindings include EU vault/recovery R2 buckets, the account sender, and all three required secrets.
- Capacity was checked before invitation creation: 0 accounts / 25 allowed. An unused, seven-day invitation for Bart was delivered privately in the conversation. No invitation email was sent; the secret URL is deliberately absent from this tracked note.
- The deployed signup page was opened in Chrome; the invitation fragment populated the form and was removed from the address bar. Turnstile completed automatically. Password entry/submission and subsequent email verification are awaiting Bart’s browser handoff.
- Cloud TypeScript checks, frontend build, 21 cloud tests and the Worker runtime test passed at this checkpoint. Tests cover concurrent capacity/admission, token replay, expired/revoked/mismatched invitations, idempotent activation and generic reset/resend responses. Later changes require another verification pass.

### Desktop and pairing implementation

- Settings → Sync is available only in the staging feature with the `io.loofah.staging` application identifier. Connection requires an explicit action; the legacy setting is not consent. Rust owns browser authorization, Keychain credentials, enrollment, transfers and native recovery/export dialogs. Display-safe status is coalesced for React.
- First-Mac enrollment requires saving and reimporting the recovery kit. Existing vaults require that kit or trusted-Mac pairing. Received keys and the original signed approval are persisted before enrollment acknowledgement; an interrupted approval is never converted into unsigned enrollment.
- Pairing uses `magic-wormhole` 0.8.1 and an authenticated Worker WebSocket backed by an explicitly EU-jurisdiction Durable Object. The mailbox is limited to ten minutes, bounded messages and attempts, distinct sessions of the same verified account, and an active enrolled approver. Revocation invalidates device-bound sessions and pending approvals.
- Migrations 0008 and 0009 are applied to staging. Current staging Worker version: `cefbddd7-cf7e-4c8e-9b46-db24da5b2c2b`. The runtime regression test exercises the actual Hono WebSocket upgrade; ordinary security headers remain enabled on HTTP routes.
- The scheduled Worker now performs bounded logical history expiry while preserving current/conflict/snapshot pins. Its regression test exercises the real scheduled entry point; physical object deletion remains disabled.
- The persistent engine stores bindings, baselines, encrypted upload spools, pending applies, coherent snapshot/checkpoint state, conflicts and cursors outside the vault. Apply journals recover before indexing. Account/vault changes and recovery-generation changes require explicit reconciliation. Cancellation preserves pending work; already-started local journal application finishes before disconnect.
- Desktop and CLI session deletion record explicit intent. Missing files or metadata cannot become remote deletions. Guarded creation/deletion and restore preserve excluded files. Registry additions preserve unknown JSON fields and refuse corrupt source files.
- Conflicts provide preview/export and explicit saved/cloud selection, including global registries. Session history has on-demand previews, capture/sync times, originating device, changed components relative to the local baseline, missing-reference warnings, whole-session restore and logical purge. Restore preserves unsynced edits and uses expected-revision checks; physical object deletion remains disabled.

### Verification evidence

- Cloud TypeScript, account frontend production build, all 23 cloud tests and the Worker runtime test passed. Coverage includes invitation/cap concurrency, replay and revocation, activation retries, exact pairing approvals, bounded mailbox attempts, quota control operations, history pins and ciphertext retention.
- All 1,363 desktop frontend tests passed after the native QA fixes, including four sync UI tests and two restore-ordering/failure tests. Desktop TypeScript and both staging/stable Rust checks passed again for these fixes.
- CLI tests and the existing vault-write suite passed. Both staging and stable desktop Rust checks passed, as did desktop TypeScript and lint (warnings only). The desktop Rust suite passed 98 tests initially; two existing five-second Codex-version probes failed during heavy disk contention, then both passed isolated reruns (100 passed in total, one pre-existing ignored test). All 225 vault-write tests passed (one pre-existing ignored test), including unknown-field preservation and refusal to overwrite corrupt registries. All seven local-consistency tests passed. The representative 7.6 GB / 9,000-file restore fixture passed, including excluded-file preservation. It took 3,528 seconds with deliberate pauses and substantial external-drive contention; this is correctness evidence, not a signed-app responsiveness result. GitHub CI passed for implementation commit `4676c6cf3`: formatting/lint, security workflow checks, CLI, desktop frontend and desktop/workspace Rust suites.
- Two authenticated staging clients completed the real Magic Wormhole transport exchange on this Mac, including wrong-code rejection. All 23 final vault-sync unit tests passed, including persisted ciphertext/snapshot state across restart. The final two-replica staging integration passed creation, note editing with recording reuse, simultaneous edit preservation, resolution, preview, restore to a new head, explicit deletion, restoration after deletion and excluded-file preservation.
- Physical Keychain failure, disk exhaustion and two-Mac acceptance remain pending. Bart confirmed that a second Mac is unavailable; separate synthetic vaults and authenticated clients are integration evidence, not physical two-Mac acceptance. Native Keychain enrollment and restart succeeded in the signed build as detailed below.

### Signed native QA — 17 September 2026

The signed/notarized staging build from `4676c6cf3` was installed alongside normal Loofah. Checks used a disposable verified staging account and a separate synthetic 1,000-session vault; normal Loofah’s vault was untouched.

- Native note creation, editing, search and restart persistence passed. Live recording and transcription passed while editing; a second recording remained usable during encrypted uploads with two transfers active. Importing a 25-second speech fixture produced a transcript, and transcript search found the expected phrase. Intelligence was disabled for this test.
- Browser-code cancellation and API approval of the disposable account worked. Saving and reimporting the same recovery kit completed enrollment through native dialogs and Keychain. Pause, quit, restart and resume retained the account, enrolled device and pending work. Disconnect returned to the disconnected UI, removed the account session from Keychain, retained the vault keys, and left all 2,008 snapshotted note/registry/excluded-attachment files unchanged. This does not replace Bart’s real browser signup/login acceptance.
- Native history preview and export passed. Restoring an earlier note preserved its unsynced edit as a separate version, committed a new head, and restored the original file. Exporting the preserved version recovered the unsynced marker. Global registries and excluded attachments were unchanged.
- QA found and fixed a stale authorization code after cancellation, a missing cached authority immediately after first enrollment, a stale open editor after restore, and a Tantivy related-document panic when term counts included unmerged deletions. The latter has a regression test that failed before the fix; all 20 Tantivy unit tests and its integration test passed afterward. Restore now flushes pending editor writes and reloads the session/editor/audio player; focused tests cover the ordering and failed-write behavior. The corrected signed build still needs its targeted native recheck.
- Workspace CI exposed a race between the lock-release test and a concurrently spawning subprocess test. It reproduced locally on the third concurrent run. Serializing those two tests leaves production locking unchanged; all 29 vault-read tests and 100 repeated concurrent transaction-test runs then passed. The updated CI run is pending.

### Installation and two-Mac walkthrough

The initial signed staging build completed at [workflow 35202201416](https://github.com/bart6114/loofah/actions/runs/35202201416), from implementation commit `4676c6cf3`. SHA-256, strict code-signature, Gatekeeper notarization and stapled-ticket checks passed for `io.loofah.staging`. A corrected artifact will replace it after the native QA fixes above. This remains a draft staging milestone until the outstanding acceptance checks pass. Bart’s private invitation link was supplied in the conversation and expires on 24 September; it is not stored in git.

1. Install **Loofah Staging** alongside normal Loofah. Use a disposable vault or a copy of test data. Verify the selected vault path in Settings → Sync before connecting. Keep normal Loofah’s vault untouched.
2. Open the private invitation, register `bartsmeets86@gmail.com`, verify the email and sign in. The Chrome signup handoff is waiting for Bart’s password entry. Verification resend and password reset should be tested through the browser; password reset does not restore encryption keys.
3. On the first Mac, open Settings → Sync → Connect and approve its code in the browser. Save the recovery kit outside the vault, then reimport that exact file. Confirm enrollment, click **Resume**, and wait for an initial successful sync.
4. When a second Mac is available, install the same staging build and select a fresh disposable vault. Connect to the same account. Choose either **Import recovery kit…**, or **Pair with a trusted Mac** and enter the displayed code in the trusted Mac’s Sync settings. After pairing, resume synchronization where paused.
5. Edit a note on each Mac while the other is paused, then resume both. Review and export both saved versions in the conflict controls, explicitly choose a version and verify both Macs converge. Confirm the recording object is reused for note-only edits.
6. Open the session menu → Version history, preview an earlier version, export it, then restore it. Verify a new head appears, the old versions remain, global records are unchanged and missing references are visible. Deletion/restore currently has integration-test coverage; a deleted-session browser is not yet available in the desktop UI.
7. Test pause/resume, cancellation, app restart, offline editing and reconnection. Delete only an explicitly selected test session; verify unknown attachments survive reconciliation. Revoke the second device and verify its session stops working.
8. Keep the second-Mac, Keychain failure, disk exhaustion, lost-metadata checks unchecked until observed. Do not use this draft milestone for external invitations or production data.

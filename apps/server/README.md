# Run the Loofah server

The server is experimental. It exposes authenticated vault, note, existing-content, search, attachment-upload, configured summary/title and batch-transcription jobs, capture staging/publication and change-stream APIs. Authenticated live captions and durable recording uploads are available experimentally; declared capture processing is experimental, and full recording/platform acceptance remains required. The desktop's default local mode works without an account or server.

Automatic summary requests can set `only_if_empty: true` when the server
advertises `conditional_summary_jobs`. This policy is part of the saved request
identity: admission and queued execution require the reviewed document revision
and an empty body. A filled or changed target conflicts before generation, and
canonical writes still compare the original revision. Manual requests default to
`false`; their serialized bytes and existing journal versions remain unchanged.
Conditional requests use journal schema 3 so older readers refuse the policy
rather than silently dropping it.

Configured batch jobs use the shared cancellable Whisper kernel without Tauri or device capture. One operator-provisioned CPU model and bounded native capacity are shared across vaults. Core owns authenticated admission, a durable job journal, immutable recording copies and revision-checked canonical transcript publication; native work retains its source/model owners through actual completion. Model paths and hashes come from operator configuration, never client requests. See [CPU deployment](#optional-cpu-transcription) for the explicit build and configuration.

The internal typed live facade accepts exact f32 frames with bounded per-channel admission, explicit replay/gap handling, authoritative captions and separate bounded UI snapshots. Cancellation and finish retain native ownership through actual thread join. Authenticated capture live endpoints and SDK subscriptions use this facade; it is not a substitute for durable raw recording uploads. The existing s16 WebSocket adapter remains compatible.

An internal Core worker now tails the existing durable capture chunks through the configured CPU host shared with batch jobs. Provisional finalized segments and their bounded partial snapshots have a private journal outside canonical sessions; replay does not restart inference, and service restart interrupts unfinished generations. Complete coverage requires persisted captions, actual native/source drain and the matching full raw-source seal. Canonical promotion additionally requires the matching published recording and fresh membership authorization. The initial limits are 64 generations per vault, 65,536 captions and 64 MiB of captions per generation. Processing interrupts when durable audio is more than 60 seconds ahead of native admission, or after 120 seconds waiting for input/publication. Errors retain raw capture data. Configured transcription hosts advertise `capture_live_transcription`. Authenticated `/captures/{capture}/live` starts a generation; its `/{generation}` status, `/captions`, `/result`, `/cancel` and `/events` routes recover results and deliver bounded SSE pages. Generations and captions stay private to their submitting member. Reconnect uses the original generation and last admitted sequence; it does not restart inference. `GET /vaults/{vault}/transcription/policy` reports only the public model alias and current availability without opening a cold vault. Notes-only hosts return disabled policy and omit the live capability. Configured transcription hosts also advertise `capture_processing`: an explicit immutable capture declaration coordinates live coverage and canonical batch repair after safe drain. Optional summaries require the original declaration and a configured summary provider. This capability is absent without operator transcription configuration; an unavailable model is reported through policy/status without a fallback or a new inference request. Historical raw captures do not acquire processing merely by being uploaded. Two-hour outage and platform acceptance remain separate gates.

Canonical batch conversion lives in `transcribe-core::transcript`; the existing `listener2_core::transcript` path re-exports it unchanged. The configured server uses the same timing, spacing, IDs, channel preparation and speaker-hint mapping without importing listener orchestration.

The experimental saved-server desktop workspace can record audio for an existing
note that has no recording, or start a pending recording using its saved destination
before signing in. A pending recording creates a note only after a verified upload
and current server authorization; it does not create an editable offline note. Capture stays on the device, uses durable private
chunks, and uploads independently of editing or server connectivity. The device
recording queue stays outside the signed-in vault view, so Stop and retained
capture discovery do not require a live server session. A lost start reply keeps
the same request UUID; closing a view does not stop recording. Reconnect to the
original account/vault to resume a retained upload. Device audio is retained after
independent canonical hash verification; automatic deletion is not enabled.

The device queue also supports **Export saved audio** without sign-in. Export
verifies the exact saved PCM prefix and preserves channel order/float bits in a
WAV, using bounded chunks and an atomic publish that never overwrites an existing
file. It retains the original capture. An interrupted timed capture can explicitly
**Prepare saved audio for upload** after its device producer has stopped; this
verifies and seals only the durable prefix, without restarting the microphone,
inventing timestamps or creating a vault note. Untimed older captures can export
but cannot invent the producer observation needed for upload. Resume remains an
explicit action against the original authorized destination.

This is not complete recording parity: automated live transcription/repair and
summaries, append and long-duration/platform acceptance remain release gates. The shared main workspace
adapter is implemented and tested but is not yet the selected remote startup UI.

## Desktop startup and server profiles

Existing installations start in standalone mode: the existing local vault opens,
works offline and requires no Loofah login. Startup does not contact an identity
provider or provision a server profile. Before loading or migrating vault settings,
the desktop acquires the canonical vault ownership lease. Settings writes, shared
Store operations and the application context retain the same original lease through
actual filesystem completion. A second independent owner is refused. Local CLI
forwarding through the desktop owner is still being implemented.

Use **Startup options** in the server workspace settings to add an HTTPS origin
and choose a saved server connection for the next app start. The same control is
available when the local vault cannot open. Selection is device metadata; it does
not change the vault format or move vault contents. Changing it does not reroute
an open editor or recording. Restart Loofah to apply it.

Remote startup opens the experimental server notes workspace directly. It does
not open a local vault, build its search index or start its local speech model.
Company sign-in uses the system browser only when saved credentials cannot be
restored. Each saved profile has a separate OS credential namespace. The saved
member and vault are restored only when they still match the authenticated
account and available catalog; otherwise choose a vault explicitly. A removed,
replaced or unsupported profile fails visibly rather than opening local data.

Saved Remote startup profiles open the shared main desktop shell, including
scoped timeline/search, editor, summary jobs and capability-selected controls.
The optional server-notes dialog retains its smaller preview workspace. Device
recording and recovery remain bound to the selected original destination; a
failed startup never opens Local data. Full recording append, retention and
platform acceptance remain release gates. Startup preferences use revision
checks and apply only at the next process start.

Capture identity, format, chunk, seal and receipt types live in the shared protocol with the unchanged version-1 fingerprint and checksum-chain functions. The neutral `capture-store` crate stores durable staging on the device and service. A first-audio producer observation upgrades a capture manifest to schema 2 without changing its audio hashes; older readers refuse that manifest. Existing untimed schema-1 captures remain readable without an invented timestamp.

The experimental `capture_ingestion` capability exposes authenticated staging APIs below. Staging retains private upload chunks. Canonical publication uses the separate capability below; transcription, remote recording controls and device cleanup remain distinct operations. Local desktop capture remains login-free. The separate experimental `capture_publication` capability adds verified publication for an existing note with no recording or transcript, or a new note bound to a pending capture. The SDK checks this capability before credentials; staging support alone cannot dispatch publication.

| Method | Path below `/api/v1/vaults/{vault}`     | Result                                                                                                        |
| ------ | --------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| POST   | `/captures`                             | Admit an immutable identity, PCM format and first-audio observation; replay the same declaration safely.      |
| PUT    | `/captures/{capture}/chunks/{sequence}` | Admit an octet-stream PCM chunk with its JSON `X-Capture-Chunk` descriptor; return the exact accepted prefix. |
| GET    | `/captures/{capture}`                   | Recover the durable prefix and seal after reconnect or an unknown reply.                                      |
| POST   | `/captures/{capture}/seal`              | Finalize only if the full declared seal matches the admitted audio.                                           |
| POST   | `/captures/{capture}/publish`           | Verify the sealed source and publish absent-only canonical WAV; return its bound checksum/size observation.   |
| PUT / GET | `/captures/{capture}/processing`     | Declare or recover explicit processing intent over the original capture identity. |
| POST | `/captures/{capture}/processing/cancel` | Request processing cancellation without stopping capture, uploads or recording publication. |

Processing declarations use the original capture request UUID in `X-Request-ID`.
They pin the first admitted operator model alias and model checksum; an omitted
model chooses the operator default once. Optional languages/keywords and summary
target/title policy are immutable on replay. An omitted summary never requests
one. The server owns live processing while an existing capture is open, then uses
one stable batch repair job when coverage is missing or incomplete after verified
recording publication. Pending captures wait for their published session binding
before batch processing. A matching complete transcript can start one checked
summary job; changing its recording, transcript or target retains the generated
result for review instead of overwriting newer data. Verified complete empty STT
can summarize saved notes only when requested, binding the absent transcript
alongside the recording checksum; no note input fails before any provider call.
`Empty` may therefore include a completed summary job without inventing a
transcript publication. No client principal, model
path or provider credentials enter this declaration.

Private `capture-processing/` intents and existing child-job journals belong in
backups. Only explicit declarations are reconciled after restart. Interrupted or
uncertain native/provider attempts are never silently rerun; a model checksum
change requires review. Cancellation requests retain child ownership until actual
drain and may leave admitted commits or retained results. Raw capture/upload progress is
independent of processing availability and cancellation. The new orchestration
capability remains unadvertised until its paired device acceptance is complete.
Pipeline children carry an atomic private origin marker binding the capture,
intent, exact request and resolved model checksum. A missing parent or coordinator
cannot turn one into a manual job. Older binaries reject marked child records;
rollback requires a complete pre-upgrade vault and operation-state backup, not
removing the new intents. Ordinary manual job encodings and canonical files are
unchanged.

Publication accepts the original `CaptureSeal` and a UUID trace header. Its durable
operation identity stays the original capture declaration UUID. It checks current
membership again after expensive verification/preparation, then retains the grant
and vault ownership through real commit and durable capture observation. It keeps
an existing start time, otherwise using the producer's original first-audio time.
No end time is invented. A pending capture creates its note only after current
authentication and verification of sealed, nonempty audio. Its immutable creation
UUID resolves one checked note identity, and the original producer observation
sets the note's creation time even if uploading happens later. The separate
one-time session binding survives retry and restart without rewriting raw capture
identity or audio. A lost publication response resumes through the same binding;
the SDK verifies canonical bytes independently before confirming publication.
Pending uploads require the distinct `capture_pending_publication` capability
before credentials or chunks are sent; older existing-note-only servers leave
them queued with an upgrade-needed error and preserve all device audio.
This raw recording queue does not permit offline remote-note editing. Pending
live transcription and automatic summaries remain separate work; explicit batch
transcription can process published audio. Append/replacement and any existing
`transcript.json` remain refused unchanged.

Private WAV/result stages and receipts recover exact admitted writes before vault
readiness. Same-process gates prevent playback, recording metadata/waveform reads
or deletion from observing an incomplete publication; gates recheck after awaited
reads. Known uncertain sessions remain gated; an ambiguous directory scan gates
recordings vault-wide while notes remain usable. Completed receipts do not recreate
later-deleted recordings. Unknown attachments and unclaimed stages are preserved.

Session lifecycle admission also protects actual note creation, capture staging,
binding and publication work. Checked session changes use a non-waiting exclusive
permit and refuse retained unpublished captures, ambiguous publication evidence
or unfinished live generations, including after raw recording publication. This
checks the service's private vault-wide staging inventory without exposing other
members' captures or removing raw files. Retained live ledgers are checked without
loading a model or rewriting recovery state when the CPU service is disabled.

The shared internal metadata-creation operation accepts the existing local CLI
timestamps, tags, author, skill and initial note in one durable decision. It uses
the same deterministic request identity and preserves version-1 creation replay.
Its version-2 receipts live in `session-creations/{session}.json`; incomplete
decisions block checked session changes. Notes/metadata, extracted tasks and tag
registration are separate checked file steps, so a later failure reports the
identified pending note and retains its decision for review or exact retry.
Completed replay returns the original accepted metadata without overwriting user
edits, recreating deleted notes or re-registering subsequently deleted tags.
Keep these receipts in backups; older binaries cannot replay this internal
operation. The HTTPS creation contract is unchanged, and remote CLI metadata
flags require an explicit future contract rather than being silently dropped.

A published response contains the exact source seal, session ID, canonical SHA-256
and byte count. Verify a separate authenticated canonical artifact read before
considering device cleanup. Neither upload acknowledgment nor this response alone
authorizes deleting device audio; server raw source is retained. A lost reply is
recovered through status and explicit identical retry without another file publish.
These APIs are internal building blocks; remote recording controls, uploader
lifecycle and authenticated live-caption delivery remain in progress. The current service spool
retains raw bytes with a default 2 GiB quota per vault and 64 capture entries;
publication does not reclaim that quota. Verified retention/cleanup and sustained
recording acceptance remain required before treating this as a continuous-use
recording deployment.

Back up the recording-publications bookkeeping with its vault. Resolve pending
intents with the current reader before rollback; older executables can ignore this
new private directory and cannot safely finish those transactions. Restore a
matching binary and complete storage backup rather than deleting uncertain
receipts. Atomic exclusive moves preserve destinations on macOS, Linux and
Windows; the inherited directory-sync helper is Unix-only, so Windows power-loss
durability has not been established as equivalent.

The service derives the server, member and vault scope from its configuration and current authorization. Staging is private to its contributing member, including in shared vaults. Editors and owners can upload; a demoted reader can inspect their own staged capture while they retain vault membership. Each operation checks access again after startup before admitting a read or mutation. Status polling supplies reconnect recovery; canonical change streams keep their existing event types.

Chunks must declare 16 kHz PCM f32 little-endian with explicit channel ownership, be at most 1 MiB, and extend the accepted prefix or exactly replay an earlier chunk. A wrong descriptor or seal fails before admitting the mutation. The initial per-vault staging limits are 2 GiB, 64 captures and 32,768 chunks; capacity errors preserve existing bytes. Files live under persistent `runtime/{vault}/creates/captures`, outside canonical sessions, and belong in service backups. Verified staging cleanup and durable client upload orchestration remain required before enabling remote recording. These endpoints are not a remote-vault editing queue.

The shared Rust core contains a durable job journal and document-body worker, with cooperative cancellation, authorization grants, retained generated drafts and revision-checked application. `VaultRuntime::enable_processing` attaches a configured generator and authorizer to a vault runtime; shutdown flushes transcripts before waiting for processing within the existing deadline. Timed-out processing retains the vault lock until its actual work stops. Optional operator configuration wires allowed providers, host environment credentials and current catalog membership into authenticated document-body job APIs. The opt-in notes workspace has summary controls, existing template-output regeneration and durable device submission recovery. Shared processing settings/model selection are available in the opt-in workspace. Optional generated titles and owner-managed vault keys are implemented; streaming previews remain unfinished. Configured CPU batch transcription uses the separate job APIs below. The existing local AI flow remains active.

The shared summary generator reuses the existing Rust templates and handles early-validation retry, reasoning-block filtering, streamed tag-footer extraction and bullet normalization through injected source/model adapters. It requires an explicit model completion before applying output and drains cancelled attempts before releasing their resources. Confirmed late output after cancellation is retained privately without canonical application. Valid tag suggestions are retained with generated results for recovery. `VaultSummarySource` prepares canonical notes, transcripts, tag context and referenced managed images without a desktop session; it preserves local-owner behavior and distinguishes authenticated job members from recording owners. `ConfiguredSummaryGenerator` resolves the current vault's model once per job and preserves it across validation retries. Shared admission bounds running and waiting work across vaults, including work still draining after caller cancellation. Resilient desktop recording uploads and the remaining main desktop AI workflow migration are unfinished; server capture staging/publication and configured batch transcription are separate implemented operations. Native IPC jobs use the same worker through the existing local vault context.

Summary and existing template-output jobs apply generated tag suggestions with the current vault auto-apply policy through the same revision-checked operation as the body and stable task reconciliation. Recovery intents guard only the owned tag projection and merge registry additions against fresh canonical files, preserving unrelated metadata and unknown registry fields. Template regeneration preserves raw frontmatter, comments, title, template identity and order; missing outputs and legacy summaries cannot be regenerated through the template operation. Keep per-vault operation storage in backups: generated-summary and generated-template intents use schema 3, while earlier document and note intents retain schemas 1/2. Older executables cannot recover the new template intent kind. Recover or drain new intents with the current version before downgrading, or restore a complete pre-upgrade vault and operation-state backup. The canonical vault format is unchanged.

`loofah_core::jobs::providers::HttpModel` supplies the shared HTTP model transport for Responses (OpenAI/Azure OpenAI), Chat Completions (OpenRouter, Ollama, Azure AI and compatible providers), Anthropic Messages and Google streaming generation. Its host-only configuration receives an endpoint, opaque model ID and optional secret; job requests cannot choose these. Images are sent inline. Keys stay in headers, redirects are disabled, and errors retain fixed failure codes rather than provider bodies or URLs. Streams require a provider completion signal and reject refusals, unexpected tool calls and malformed output. Byte/event/output budgets, connection/read/total deadlines and cancellation bound the owned HTTP work. Only explicit 429 rejection is automatically retried, up to the request's four-retry budget; ambiguous disconnects never repeat a generation automatically. Responses requests set `store: false` to disable response storage for later retrieval. The native local host reuses existing Keychain/Credential Manager entries and the ChatGPT subscription runtime through injected adapters. Vault-owner secret updates use the private credential API below; live-provider validation remains required.

For development, set `VITE_REMOTE_PREVIEW=true` when starting the desktop frontend. **Settings → Storage → Try server notes** opens the opt-in notes preview. Connect to an HTTPS server, sign in through the company browser flow, and select a personal or shared vault. The preview supports note creation, search, revision conflicts and device draft recovery. Servers advertising `note_editor_read` and `note_editor_write` use the existing rich editor for notes, with title and explicit task state saved together. Tables, formatting, task identities, statuses and dates survive these saves. Older servers retain Markdown source editing. An existing source-editor recovery stays on its original editing path until resolved; it is never silently converted into task intent. Existing summaries, template outputs, transcripts and tasks can be viewed as text. With the `document_edit` capability, **Outputs → Edit** edits summary/template bodies, and **Add summary** opens a missing summary. These body saves reconcile tasks in Rust; template titles and metadata stay unchanged. The separate transcript/task lists remain read-only; task checkboxes inside a rich note are editable by vault editors. It keeps tokens in native Rust and the OS credential store. Editing pauses during a disconnect; saved recovery drafts stay on the device. Readers cannot edit.

**Device recovery drafts** opens a separate read-only dialog for the selected account/vault. Storage settings also open an inventory across signed-out accounts and removed vaults before server login. These views find saved note and output copies from closed windows even when their original resource has been deleted. Open a copy and use **Copy saved text** to recover its exact Markdown. Reloading and copying work during a network interruption. These views neither recreate a resource nor write to the server, and they keep the original recovery records. The selected-vault view closes when access is lost; the device inventory remains available without a connection.

Copies containing explicit editor task state also offer **Copy all recovery data**, including both revision baselines, task identities, statuses, dates and rich bodies. These device records use schema 2; existing text-only copies remain schema 1. Older apps refuse schema 2 instead of discarding task intent. A text-only editor cannot replace a task-aware recovery record. The notes preview mounts the compound draft controller with the existing rich editor. It retains task changes during pending saves, lost responses and reviewed conflicts. The default local note editor now uses the same checked note/task operations through IPC, preserving its native file actions and login-free startup. Size admission is chosen by the host: the server keeps the bounds below, while the trusted local desktop and CLI retain their previous note/task capacity. Device recovery storage has a separate quota and reports copy failures. The full remote main desktop/profile migration remains in progress.

Use **Create shared vault** for a separate company vault. **Vault members** shows its members; owners can add existing company members, set reader/editor/owner roles, remove members, rename the vault or explicitly delete its access. The last owner cannot be removed or downgraded. Personal vaults have no sharing controls. Catalog conflicts require a reload before another change. An uncertain creation retries its original request rather than silently creating another vault.

This preview runs inside the existing local desktop host. It does not yet replace the main desktop shell or provide remote-only startup or recording. Rich note content renders owned attachments through client resources; the Attachments tab supports bounded listing, original-copy actions and checked removal. The preview supports file uploads and native drops; the existing mention-insertion picker still needs migration. Do not use it as a complete remote deployment of Loofah. Keep `VITE_REMOTE_PREVIEW` unset for the existing standalone experience.

## Configure company SSO

Use one company OIDC issuer with JWT access tokens and advertised S256 PKCE support. Configure a public native OAuth client and a separate API audience. Register the loopback redirect `http://127.0.0.1:{port}/oauth/callback` with dynamic ports, following your issuer's native-app registration rules. Give authorized company members the API access scope. An ID token is not an API credential.

Copy [deploy/server.json](deploy/server.json) to `/etc/loofah/server.json` and replace the example domains and OIDC identifiers:

- `required_scope` is the scope required in the signed access token's `scope` or `scp` claim.
- `login_scopes` are the scopes requested by the native client. Providers with qualified resource scopes may require a different spelling here than in `required_scope`.
- `access_token_types` defaults to the JWT access-token profile's `at+jwt` variants. Explicitly configure `JWT` for a provider using that header type; issuer, API audience, signature, expiry and scope checks still apply.
- Signing keys come only from the configured issuer's discovery document. RS256 and ES256 are supported. The service refuses invalid discovery metadata and ambiguous signing-key IDs.
- `public_url` uses HTTPS. Keep the HTTP listener private behind TLS termination. The development-only `allow_insecure_loopback` option permits HTTP only for literal loopback URLs and does not disable authentication.

The server stores identities by issuer and subject. Successful provisioning creates one personal vault. Shared-vault owners can add existing company members as readers, editors or owners. Personal vaults cannot be shared, and a shared vault must retain an owner. Deleting a shared vault immediately removes access; its files remain for operator recovery.

## Build and start

Build from the repository root with `cargo build --locked --release -p loofah-server`. The version comes from the root `package.json`; API-major negotiation is separate from the application version.

Run `loofah-server --config /etc/loofah/server.json`. Startup validates the configuration, acquires exclusive service ownership, loads the catalog, and discovers the issuer. Vaults open on demand, with an exclusive lifetime lock and the shared Rust startup and indexing logic.

Linux x64 is the initial deployment platform. [Dockerfile](Dockerfile) builds the same executable without Tauri. Build it from the repository root with `docker build --platform linux/amd64 -f apps/server/Dockerfile -t loofah-server .`.

For a native Linux installation, create a dedicated `loofah` system user, install the executable at `/usr/local/bin/loofah-server`, and install [deploy/loofah-server.service](deploy/loofah-server.service). The unit creates private state/cache directories and handles SIGTERM shutdown. Install [deploy/Caddyfile](deploy/Caddyfile) in an existing Caddy service with `LOOFAH_DOMAIN` set to the public hostname. It forwards SSE without buffering and supports WebSocket upgrades for the later audio ingestion endpoint.

The container runs as UID/GID `10001`. Mount persistent volumes at `/var/lib/loofah` and `/var/cache/loofah`, and mount the configuration read-only at `/etc/loofah/server.json`. The supplied configuration binds loopback; when Caddy runs on the host, Linux host networking preserves that private listener. Do not expose the backend port publicly.

Shutdown closes new admission and waits for owned startup, indexing, media,
processing and storage work to actually finish. A five-second vault response
deadline does not release ownership while work continues. Dirty transcript
flushes retry with bounded backoff and retain the vault lock until successful.
The example systemd unit still enforces `TimeoutStopSec=60`: a permanent storage
failure can reach that external forced-stop limit. Allow more time when needed
and check service errors before treating a backup as cleanly stopped.

## Offline backup, restore and upgrades

Run maintenance commands as the service account, using persistent local storage
with working file locks, atomic rename and directory sync. Stop the service and
wait for its process to exit before taking a snapshot. `backup` acquires the
existing service lock and every existing vault lock; it refuses a still-running
service or another vault owner. These commands do not load server configuration,
contact the identity provider, load models or create a cache.

Create two absent output directories under existing, private backup parents:

```bash
loofah-server backup \
  --storage-root /var/lib/loofah \
  --output /srv/loofah-backups/before-upgrade \
  --secrets-output /srv/loofah-private-backups/before-upgrade
loofah-server verify-backup \
  --snapshot /srv/loofah-backups/before-upgrade \
  --secrets /srv/loofah-private-backups/before-upgrade
```

The main bundle contains the catalog, entire raw vault trees and required runtime
state, including jobs, upload/capture state and mutation receipts. This explicit
operator backup preserves unknown attachments, dotfiles and empty directories;
it does not make them application content. The ephemeral service lock is omitted.
Provider credentials under `runtime/<vault>/creates/provider-credentials/` appear
only in the separate private bundle. Both bundles use owner-only permissions.
The private manifest contains file digests, not credential values, and binds to
the snapshot UUID and exact main manifest hash. If credentials were present,
verification and restore require the matching private bundle. Keep it separately
access-controlled and encrypt it with your backup system. The main bundle still
contains private meeting data and must also remain protected. SHA-256 checks
detect corruption; they do not authenticate an untrusted backup source.

Back up the operator configuration, environment credentials and TLS/identity
provider deployment separately. External caches and downloaded models are not
part of `storage_root` and can be regenerated. Do not put their directories, or
operator environment files, inside `storage_root`. Backup copies all unknown
storage-root files, so such misplaced files would otherwise be included.

Restore into a new, nonexistent storage directory on the same filesystem as its
staging parent:

```bash
loofah-server restore \
  --snapshot /srv/loofah-backups/before-upgrade \
  --secrets /srv/loofah-private-backups/before-upgrade \
  --storage-root /var/lib/loofah-restored
```

Restore verifies complete inventories, file digests, bundle pairing and catalog
schema before atomically publishing the new directory. Existing destinations
are never replaced. An interruption may leave a protected
`.loofah-maintenance-<uuid>.partial` sibling, or a completed private bundle before
the main bundle was published. These are not live storage or complete backup
pairs. Preserve them for diagnosis; retry to absent destinations using the intact
original storage or verified bundles. Remove abandoned staging directories only
after confirming no maintenance process owns them.

For an upgrade, first retain the stopped snapshot and matching executable, then
start the new executable with the intended storage and unchanged operator policy.
Shared startup recovery runs under the exclusive vault lease before content is
ready. The offline verifier validates backup/catalog formats; each vault's real
startup readers validate its runtime formats when opened. Unsupported future
runtime records remain intact and fail that vault closed. Check authenticated
canonical reads and representative writes for each required vault, along with
capture/job recovery, before reopening access. A rollback uses the matching old
executable and its pre-upgrade snapshot restored into an empty directory; do not
point an older executable at potentially upgraded runtime records. Switch the
service configuration only after the restore succeeds and ensure its systemd
write-path restrictions cover the selected storage/cache roots.

This path rejects symbolic links, hard-linked files, special files and file names
that are not portable UTF-8 relative paths; it never dereferences them into a
snapshot. No external writer may modify storage during maintenance. The current
qualification includes stopped-service restore and legacy interrupted document
recovery fixtures; it does not establish production upgrade downtime, all prior
executable versions, live-client reconnect or uninterrupted device recording.

## Optional CPU transcription

The default executable and [Dockerfile](Dockerfile) remain notes-only builds. CPU builds use `cargo build --locked --release -p loofah-server --features cpu-transcription` after the [pinned native setup](../../.github/actions/cli_native_setup/linux.sh). The explicit Linux x64 image provisions those dependencies; build it from the repository root:

```bash
docker build --platform linux/amd64 -f apps/server/Dockerfile.cpu -t loofah-server:cpu .
```

[Dockerfile.cpu](Dockerfile.cpu) uses Ubuntu 24.04 for its native builder and runtime, with the same checksum-pinned ONNX Runtime and GCC 14 setup as Linux CLI/CPU CI. The notes-only Debian image is unchanged. This initial CPU variant does not enable GPU acceleration, Linux desktop support or automatic model downloads.

Provision a model file in a read-only directory, verify its byte length and SHA-256, and add an explicit `transcription` property to `server.json`. For the existing base-q8 fixture:

```json
{
  "transcription": {
    "model": "operator-base",
    "model_path": "/models/ggml-base-q8_0.bin",
    "model_size_bytes": 81768585,
    "model_sha256": "c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9"
  }
}
```

Mount that model directory read-only at `/models`, alongside the existing state, cache and configuration mounts. Use `TMPDIR=/var/cache/loofah` so owned audio preparation has writable scratch space on the cache volume. UID/GID `10001` needs read access to the model and write access to state/cache. Keep the model bytes unchanged for the service's lifetime; replacing a model requires a service restart. Omitting `transcription` loads no model. Supplying it to a notes-only build fails configuration validation.

`batch_transcription_jobs` means this support is configured, including while a model is loading or unavailable. Notes remain available; jobs report `engine_unavailable` without a download, another provider or a local fallback. By default, one native workload is admitted across all vaults; busy capacity reports `resource_limit`. Operator bounds are `workload_groups` (default 1, maximum 8), per-vault `max_jobs` (256), `max_queued` (16) and `private_audio_bytes` (8 GiB). Increase native concurrency only after measuring CPU and memory use. Job history/source retention has no automatic cleanup API yet, so include private operation storage in backups and capacity planning.

Authenticated batch APIs are below `/api/v1/vaults/{vault}`:

| Method | Path                               | Purpose                                                                                                                                                                 |
| ------ | ---------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| POST   | `/transcription/jobs`              | Submit with a UUID `X-Request-ID`, `session_id`, `expected_recording_sha256`, `expected_transcript: "absent"`, configured `model`, and optional `languages`/`keywords`. |
| GET    | `/transcription/jobs`              | List the submitting member's bounded job history.                                                                                                                       |
| GET    | `/transcription/jobs/{job}`        | Poll durable status after a disconnect or lost response.                                                                                                                |
| GET    | `/transcription/jobs/{job}/result` | Inspect a retained result bound to its job and session.                                                                                                                 |
| POST   | `/transcription/jobs/{job}/cancel` | Request cancellation; actual source/native work still drains.                                                                                                           |
| POST   | `/transcription/jobs/{job}/resume` | Recheck and apply an existing retained result without rerunning inference.                                                                                              |

Editors/owners submit and resume; reads remain private to the submitting member while they retain vault access. Accepted work survives the original login token's expiry/logout. Membership changes are checked during work and again before publication; membership loss cancels, and service stop interrupts. Publication requires the original recording and an absent canonical transcript. A pending capture must first bind and publish its recording through the capture publication operation above. Existing transcripts, append/replacement, diarization configuration and authenticated live captions require later explicit operations. Recording files, note content, metadata and attachments are preserved. Confirmed results can be retained without publication; output discarded by native cancellation is not guaranteed recoverable.

Shutdown stops admission and requests cancellation early, retains vault/source ownership until actual Core/native/mapping work drains, then destroys the model. A response deadline reports pending work; it does not authorize process exit or resource reuse.

The Linux CPU CI job is configured to verify a cached, checksum-pinned model and a nonempty exact test inventory, build the service once, and package that same release binary via the Dockerfile's explicit `ci-runtime` target. It runs the configured Service → Core → canonical-transcript case inside the Ubuntu runtime with read-only fixtures/model, no external network and hash/ABI readback. A successful run qualifies that exact runtime artifact; [BuildKit skips the unrelated builder stage](https://docs.docker.com/build/building/multi-stage/#differences-between-legacy-builder-and-buildkit). A successful full image build remains a separate deployment gate. macOS test results do not establish Linux container support.

## Optional processing host configuration

Processing remains experimental. Configure the worker and summary-body job APIs
with a `processing` property in `server.json`:

```json
{
  "processing": {
    "providers": [
      {
        "id": "openai",
        "models": ["your-existing-model-id"],
        "api_key_env": "LOOFAH_OPENAI_API_KEY"
      }
    ],
    "concurrent_jobs": 2,
    "waiting_jobs": 32,
    "allow_custom_templates": false
  }
}
```

Replace the model placeholder with the ID already configured for your provider.
Each vault's existing `current_llm_provider`/`current_llm_model` settings must select
an allowed pair; there is no implicit model replacement. In the opt-in notes
workspace, open **Processing settings** to select an approved provider/model
without editing the server files. Readers can inspect settings; editors and
owners can save them. Vault owners can manage provider keys through the
`provider_credentials` capability:

- `GET /api/v1/vaults/{vault}/processing/providers/{provider}/credential` returns
  `provider_id`, an opaque `revision`, `source` (`vault`, `operator`, or `missing`),
  and `endpoint_matches`. Readers and editors cannot inspect this status.
- `PUT` to the same route accepts `expected_revision` and an explicit `api_key`:
  a nonempty key sets an override; `null` clears it. A stale revision returns a
  `conflict` outcome containing only current status. A request ID header is required.

Only configured provider IDs and operator-approved endpoints are accepted.
Keys are never returned, written to `config.json`, or included in errors. Private
credential records live in each vault's service runtime `creates/provider-credentials/`
directory and must be backed up separately with access restricted to the service
account. Unix directories/files use permissions `0700`/`0600`.

A vault override takes precedence over the operator environment key. Clearing it
restores that fallback. Overrides are bound to the normalized operator endpoint;
a changed endpoint leaves the old override inactive (`endpoint_matches: false`)
until an owner explicitly replaces or clears it. New model admissions resolve
the current key; already admitted generation and title work retain their original
model credentials. Failed or unknown provider work is never replayed by a key
update. Omitting `api_key_env` allows owners to supply keys per vault; providers
requiring a key remain unavailable until one is configured.

Set the referenced key in the service environment, outside canonical vault
content. The supplied systemd unit reads optional `/etc/loofah/server.env`; keep
that file private to the operator. Containers can receive the same environment
variables through their runtime. Missing, empty or invalid configured credentials
fail startup without printing their values. Restart the service to rotate these
operator credentials.

Known provider IDs match the desktop: `openai`, `anthropic`,
`google_generative_ai`, `openrouter`, `mistral`, `ollama`, `lmstudio`,
`azure_openai`, `azure_ai`, `cloudflare_workers_ai` and `custom`. Azure,
Cloudflare and custom endpoints require an explicit `base_url`. Other providers
use their existing defaults. `models` is an exact allowlist; a vault cannot select
another provider endpoint. A nonempty vault `base_url` must match operator policy.
`localhost` and literal loopback model URLs refer to the server. Other plain HTTP
model endpoints require explicit `allow_insecure_http: true`; the external Loofah
API still requires HTTPS. `include_images: false` disables image context for a
provider; it defaults to true. ChatGPT subscription inference remains local.

Limits apply across all opened vaults: 1–16 running jobs, up to 1,024 waiting
jobs, with defaults of two and 32. Each vault's durable queue remains independently
bounded. Waiting admission is cancellable, and owned work retains its capacity
through actual model/source drain. Membership and edit rights are checked before,
during and immediately before canonical application. Browser logout/token expiry
does not stop an admitted server job; membership removal/downgrade does. Arbitrary
custom summary templates are disabled by default. Enabling them explicitly trusts
the vault's custom template code; the check uses the exact snapshot rendered.

## API boundary

`GET /health`, `GET /api/v1/capabilities` and `GET /api/v1/auth/config` are public. Protected requests require `Authorization: Bearer …` and `X-Loofah-Api-Major: 1`. Creates also require a client-generated UUID in `X-Request-Id` so a lost response can be retried safely.

| Operation                              | Endpoint                                                                                                                                  |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| Provision/read the signed-in member    | `POST` / `GET /api/v1/me`                                                                                                                 |
| List existing company members          | `GET /api/v1/members`                                                                                                                     |
| List/create vaults                     | `GET` / `POST /api/v1/vaults`                                                                                                             |
| Read/rename/delete shared vault        | `GET` / `PATCH` / `DELETE /api/v1/vaults/{vault}`                                                                                         |
| Read/change membership                 | `GET /api/v1/vaults/{vault}/members`, `PUT` / `DELETE /api/v1/vaults/{vault}/members/{member}`                                            |
| List/create/read sessions              | `GET` / `POST /api/v1/vaults/{vault}/sessions`, `GET …/sessions/{session}`                                                                |
| Query lightweight session headers      | `POST /api/v1/vaults/{vault}/session-summaries/query`                                                                                     |
| Read/patch session metadata            | `GET` / `PATCH /api/v1/vaults/{vault}/sessions/{session}/metadata`                                                                        |
| Read/save a note                       | `GET` / `PUT /api/v1/vaults/{vault}/sessions/{session}/note`                                                                              |
| Read summaries/template outputs        | `GET /api/v1/vaults/{vault}/sessions/{session}/documents`                                                                                 |
| Read/replace summary Markdown          | `GET` / `PUT /api/v1/vaults/{vault}/sessions/{session}/summary`                                                                           |
| Read/replace template Markdown         | `GET` / `PUT /api/v1/vaults/{vault}/sessions/{session}/outputs/{document}`                                                                |
| Read lightweight transcript metadata   | `GET /api/v1/vaults/{vault}/sessions/{session}/transcript-overview`                                                                       |
| Read complete transcript records       | `GET /api/v1/vaults/{vault}/sessions/{session}/transcripts`                                                                               |
| Open/retry a pinned recording          | `PUT /api/v1/vaults/{vault}/sessions/{session}/audio/leases/{client-uuid}`                                                                |
| Read/close a pinned recording          | `GET` / `HEAD` / `DELETE /api/v1/vaults/{vault}/audio/leases/{client-uuid}`                                                               |
| Read its verified waveform             | `GET /api/v1/vaults/{vault}/audio/leases/{client-uuid}/peaks`                                                                             |
| Delete a checked recording             | `DELETE /api/v1/vaults/{vault}/sessions/{session}/audio?expected_sha256={sha256}`                                                         |
| Read recording metadata                | `GET /api/v1/vaults/{vault}/sessions/{session}/audio/metadata`                                                                            |
| List owned attachments                 | `GET /api/v1/vaults/{vault}/sessions/{session}/attachments?limit=100&after=...`                                                           |
| Upload an attachment                   | `POST /api/v1/vaults/{vault}/sessions/{session}/attachments?filename={name}&size_bytes={length}&sha256={sha256}`                          |
| Reconcile an upload                    | `GET /api/v1/vaults/{vault}/sessions/{session}/attachment-uploads/{upload-uuid}?filename={name}&size_bytes={length}&sha256={sha256}`      |
| Review an attachment revision          | `GET /api/v1/vaults/{vault}/sessions/{session}/attachments/{attachment}/metadata`                                                         |
| Remove a checked attachment            | `DELETE /api/v1/vaults/{vault}/sessions/{session}/attachments/{attachment}?expected_sha256={sha256}`                                      |
| Read lightweight attachment info       | `GET /api/v1/vaults/{vault}/sessions/{session}/attachments/{attachment}/info`                                                             |
| Open/retry an embedded attachment      | `PUT /api/v1/vaults/{vault}/sessions/{session}/attachments/{attachment}/leases/{client-uuid}`                                             |
| Read/close an embedded attachment      | `GET` / `HEAD` / `DELETE /api/v1/vaults/{vault}/attachments/leases/{client-uuid}`                                                         |
| Read session tasks                     | `GET /api/v1/vaults/{vault}/sessions/{session}/tasks`                                                                                     |
| Read/save a coherent note editor       | `GET` / `PUT /api/v1/vaults/{vault}/sessions/{session}/note/editor`                                                                       |
| Read checked task snapshot             | `GET /api/v1/vaults/{vault}/sessions/{session}/tasks/snapshot`                                                                            |
| Replace a task source                  | `PUT /api/v1/vaults/{vault}/sessions/{session}/tasks`                                                                                     |
| Read people/tag registries             | `GET /api/v1/vaults/{vault}/people`, `GET …/tags`                                                                                         |
| Search                                 | `POST /api/v1/vaults/{vault}/search`                                                                                                      |
| List/search existing agent projections | `POST /api/v1/vaults/{vault}/agent/sessions/list`, `POST …/agent/sessions/search`                                                         |
| Read agent meeting/transcript/export   | `GET /api/v1/vaults/{vault}/agent/sessions/{session}`, `GET …/{session}/transcript`, `GET …/{session}/export`, `GET …/{session}/segments` |
| Subscribe                              | `GET /api/v1/vaults/{vault}/events`                                                                                                       |

The additive `transcript_overview` capability returns `has_words`, ordered distinct `speaker_labels`, minimum `started_at` and maximum explicit `ended_at` from the in-memory index without loading words or note bodies. A session without a transcript returns an empty overview; a missing session returns 404. Remote responses admit at most 4,096 labels and 256 KiB of encoded JSON and fail rather than truncate. The trusted local IPC projection preserves its existing metadata capacity. Transcript changes and reconnect/resync invalidate the scoped frontend query.

The additive `session_summaries` capability returns only `id`, `title` and `created_at`. Send `{ "kind": "list", "limit": 55, "order": "newest_first", "titled_only": false }` for a bounded header list, `{ "kind": "title", "query": "plan", "limit": 50 }` for a title substring query across the full index, or `{ "kind": "ids", "ids": ["session-id"] }` to resolve known IDs from recovered canonical metadata. Lists and title queries allow 1–200 results. ID queries allow at most 100 input IDs, deduplicate them in requested order and omit absent/deleted sessions. Title matching trims the query and uses Unicode lowercase substring matching; queries must contain 1–4096 UTF-8 bytes without control characters. Indexed lists order by the existing `created_at` string convention with ID as a deterministic tie-breaker. This reader-accessible POST has no mutation or idempotency side effect and never returns note bodies, paths or arbitrary metadata fields. Raw response text is limited to 1 MiB, with an 8 MiB JSON envelope budget. Session/header changes and reconnect/resync invalidate the corresponding scoped frontend queries.

The additive `attachment_upload` capability accepts an `application/octet-stream` body and a client UUID in `X-Request-Id`. The declaration contains only an opaque filename, byte length and lowercase SHA-256, never a client path. The server caps attachments at 32 GiB, admits two attachment transfers independently of recording reads, and bridges bytes into Rust storage with two queued 64 KiB chunks. Uploads have a 35-second byte-progress idle deadline; healthy uploads can exceed the ordinary 30-second JSON request deadline. A declared `Content-Length` must match. Compressed request bodies are refused.

The shared application operation receives into a private hidden stage outside the vault write lock, verifies length and SHA-256, rechecks current edit access, and publishes with an exclusive move and existing filename deduplication. It does not expose partial final files or overwrite an existing attachment. The session cannot be deleted or relocated while actual staging/publication I/O is running, including after the caller abandons its future. Revocation or a downgrade to reader stops HTTP receiving without holding the company catalog lock during client backpressure.

Upload receipts live under persistent per-vault operation storage in `attachment-uploads/`. Status is scoped to the member, vault, UUID and original declaration, returning `missing`, `committed`, `rejected` or `uncertain`. Preserve the original UUID and declaration when checking a lost reply. A completed replay returns its original metadata without publishing again, even if that file was subsequently deleted. A pending receipt can only confirm the exact published generation; missing or changed evidence remains uncertain and never resumes a move. A known rejected operation may retry with the same declaration. Clients must not automatically retry uncertain mutations or replace their UUID. Keep receipts in backups and upgrades alongside other operation storage.

Receiving is not resumable yet. A disconnect before complete input cleans the owned stage; a disconnect after publication is reconciled through its receipt. Crash cleanup of abandoned incoming stages, clipboard/drop adapters, desktop upload controls and restart UX remain migration gates. These APIs do not provide offline remote-vault editing or recording ingestion. The existing local upload UI remains on its previous path and requires no login.

The native Rust SDK streams uploads through two queued 64 KiB chunks, checks source length/SHA-256 and requires explicit successful source completion. Interrupted, failed or panicking readers interrupt HTTP instead of presenting partial input as complete. It retains admission through actual reads after cancellation, uses a 35-second byte-progress idle deadline and never retries an upload automatically. A typed status preflight recovers committed results without reading the source; uncertain receipts refuse a new upload. Returned metadata is checked against the original declaration, including status responses.

The desktop host now supplies device-only path review and lazy file readers to local IPC and HTTPS over the same application operation. Review hashes a selected regular file off the UI thread, preserves selected symlink-source names and returns the declaration before sending. Device source paths stay on the client and never enter HTTPS operations or vault content. Two device transfers bound review/upload work independently of recording resources. Completed retries can recover without reopening a source that has moved or disappeared. `VaultClient` exposes shared preparation, source-upload and status methods. Browser/clipboard files stage through sequential 64 KiB native IPC chunks outside the vault. The private registry uses opaque, window-owned IDs and bounded source/byte admission. Sources close on success, explicit removal, terminal scope loss, page reload, window destruction or shutdown. The shared attachment panel now mounts picker/drop controls and pending/error/explicit retry actions. Failed attempts retain the original UUID/declaration/source while the scope remains open; reconnect never silently queues or retries an upload. The shared rich editor uses these adapters for clipboard files and native drops when no host handler is supplied. The default local main editor now sends attachment uploads through the same checked operation, while preserving its cached file resolver, native file URLs, picker/drop behavior and audio-import interception. Its host mutation state retains original retry identity and is removed when the owning vault scope closes. Crash/restart recovery, incoming-stage cleanup after process termination, remaining local read/presentation migration and recording ingestion remain open. This window-scoped cache is not a durable recording outbox.

The additive `attachment_info` capability returns lightweight `FileInfo` for an exact attachment basename: `filename`, `contentType` and `sizeBytes`. It stats an opened file without hashing its contents or retaining a download lease. A missing attachment or session returns null; inaccessible vaults remain 404. Names follow the same ownership and path validation as attachment reads. This information is descriptive, can change before download, and supplies no content revision; open a pinned lease when bytes are needed. Current access and expiry are rechecked before delivery. The same operation is exposed through local IPC and `VaultClient.attachmentInfo`, preserving login-free local access.

The headless runtime now starts a native filesystem watcher before vault initialization and feeds the same reconciliation code used by the desktop. Its input queue is limited to 256 paths, batches keep at most 1,024 paths, and a busy source cannot extend a batch beyond two seconds. Overflow or watch errors request a shallow full rescan, including the file-backed people/tag registries. Watch initialization failure prevents opening the vault. Shutdown closes ingestion and flushes the store before reporting completion. On macOS, native OS watch deallocation runs independently after ingestion stops because the FSEvents unregister RPC can stall; the closed source retains no SessionStore or vault lifetime lock. Linux is the initial production server platform; native watcher portability and production-load acceptance remain required.

The additive `attachment_metadata` capability returns the same path-free `FileMetadata` as a pinned attachment: filename, descriptive MIME, byte size and lowercase SHA-256. This explicitly requested review hashes bytes with bounded reader admission; listing stays stat-only. It does not retain a download lease, and current membership and token expiry are rechecked after hashing.

The additive `attachment_remove` capability requires editor/owner access, an explicit `x-request-id` UUID and `RemoveAttachmentRequest.expected_sha256` in the query. The shared application operation checks content and file identity, rechecks current authorization after hashing, then moves one exact owned attachment into the existing vault Trash. It never changes Markdown links, unknown files or other attachments. A changed file returns a conflict with `mutation_outcome: not_committed`. Generic failures after an earlier request may have committed report `unknown`; clients do not retry a mutation automatically.

Removal receipts live in persistent per-vault operation storage under `attachment-removals/`, outside canonical sessions. A durable pending record names the exact Trash target before the non-replacing move. A completed receipt returns its original boolean result for the same member/request/body even if someone later restores the same filename. An interrupted pending record may confirm an exact original already in Trash, but never resumes deletion against the live filename. Missing or ambiguous evidence stays uncertain. Preserve operation storage across restarts, backups and upgrades; receipts currently have no automatic expiry. Unix directory links are synced when created and after a move; native Windows behavior still requires its own platform checks, and these tests do not establish sudden power-loss durability on every filesystem.

Both local IPC and HTTPS expose the same review/removal operations. The desktop panels share one confirmation dialog, retain the reviewed revision, disable changed/stale/offline confirmations, and retain uncertain feedback even when a change event has already removed the row. Users must close and review again after a refusal or uncertain reply. Successful removal closes matching attachment resources while leaving unrelated recordings, attachments and restored files with different revisions available. The default local path uses the same operation without an account or login. The default local main editor keeps its existing presentation host, with uploads delegated to the shared operation. The shared panel/editor adapters described above support both transports without local login.

The additive `artifact_details` capability refines the existing `index_changed` event with optional `artifacts` entries. Each entry identifies a session and a recording, all attachments, one exact attachment basename, or all owned media. Existing `entity` and session `ids` remain unchanged; missing details require a broad refresh for those IDs. Coalescing preserves and deduplicates details, and oversized details fall back to the same scoped invalidation. The shared watcher ignores unknown, hidden and nested attachment files; attachment edits still notify while capture is active. Recording-only changes do not invalidate attachment resources. Reconnect replay retains details, while an expired cursor or service restart requires a full resync.

The additive `attachment_leases` capability opens exact basenames under the selected session's `attachments/` directory. Paths, dotfiles, symlinks and unknown files elsewhere in the session cannot be read through this API. Attachment IDs are URL-encoded path components; their existing spelling and vault format stay unchanged. Its lease response adds `attachment_id`, with the same `filename`, `contentType`, `sizeBytes` and SHA-256 metadata fields as recording reads. The shared contract type is now `FileMetadata`; existing recording JSON is unchanged. Extension-based MIME information is descriptive, with octet-stream fallback. Responses add `nosniff` and a sandboxed content-security policy.

Attachments reuse the recording reader's pinned descriptors, checked chunks, ranges, ETags, idle lifetime, close behavior and shared capacity. Handle UUIDs cannot cross endpoint kinds, owners, vaults, sessions or attachment IDs. Both media kinds share the existing 64-handle host/eight-handle member budgets. Eight binary streams are shared, with at most two attachment streams so file downloads leave recording capacity available. Stream admission stays owned by actual blocking reads after cancellation. Closing a session recording does not close its independent attachments.

Recording metadata and both lease-open operations release the catalog grant before hashing, retain global media admission through caller cancellation, then check expiry/current access again before delivering the result. Membership removal can therefore complete while these operations wait or hash. The native Rust SDK now supports typed attachment open, HEAD, bounded reads and close through the same media request implementation as recordings. It validates scope/metadata/headers, limits each read to 1 MiB and supports cancellation. Remote basenames remain opaque on the client, including names that are unavailable as local Windows filenames. The desktop host now provides opaque, window-scoped attachment resources and verified native preview/copy operations. Rich note image/file views and the shared attachment panel use these resources. Remaining local-host attachment flows still require migration. The local editor preserves its cached attachment resolver and native file actions.

The native SDK also streams complete attachments into caller-owned unpublished sinks. It checks exact MIME, length, ETag and resumed ranges, then verifies the full SHA-256 before returning the sink. Healthy streams use a 35-second byte-progress idle deadline rather than a total request deadline. Two transient retries can resume received bytes; one expired/restarted lease can be reopened only when its metadata still matches. Cancellation retains admission and temporary-file ownership through actual buffered writes and flushes, including when the caller abandons its future. The desktop adapter provides complete image previews up to 32 MiB through its authenticated native media protocol, with no 1 MiB truncation. Larger previews are explicitly refused; original copies stream to disk without that preview-size limit. Metadata/resource handles retain no backend download lease. Two global attachment transfers are allowed, with at most one original-copy transfer so a copy leaves preview capacity available. Save publishes a checksum-verified, synced, deduplicated original under the client Downloads/Loofah directory; open/reveal reuse that resource's existing regular local copy to preserve local edits. The original server attachment is unchanged. Paths remain native-only, and downloaded files receive standard OS download provenance where supported. Resource close, window teardown, cancellation, logout and vault revocation stop the owned work. These host operations, `VaultClient.openAttachment` and vault-scoped resource ownership are implemented. Duplicate references share a query/resource; replacement, last-observer removal, access loss and scope disposal close their handles, including late results after cancellation. The local adapter retains device file URLs and ordinary open/reveal actions; the remote adapter supplies explicit original-copy actions and localized action labels. The shared rich note editor now mounts these resources through an optional asynchronous provider. Image/file views show loading, missing-file, action and retry feedback; oversized images retain original-copy actions. Device actions use explicit downloaded-copy labels, cancel on unmount/access loss and never retry automatically. An uncertain reply tells the user a copy may already exist. Runtime URLs and action state never enter saved Markdown. The default main editor, output previews and image strip now use the same scoped per-attachment resources. Canonical attachment IDs take precedence over stored runtime URLs; legacy local nodes without IDs retain their existing presentation. Source/output editors retain Markdown editing. The shared workspace now includes a paginated attachment list/action panel. The shared review/removal dialog and upload controls are implemented, including the actual main attachment list and upload host. Lists are paged and original-copy actions are loaded on demand. Durable upload restart recovery remains open.

The `recording_leases` capability adds authenticated recording reads for native media adapters. Open with a fresh client UUID; retry that same request after a lost response to retain the same file generation. The response contains canonical filename/MIME/size/SHA-256 metadata and an opaque handle, never a server path. A missing recording returns null. Handles are scoped to the authenticated member and vault; knowing an ID grants no access. Readers may open their own handles. Active capture or unfinished import returns retryable 503. Admission exhaustion returns retryable 429.

GET supports one byte range, suffix/open-ended ranges, strong SHA-256 ETags, `If-Match` and `If-Range`. Multiple or unsatisfiable ranges return 416; a stale `If-Match` returns 412. HEAD describes the complete pinned file. A GET without Range streams the file in 64 KiB chunks. Authorization is rechecked between chunks without holding the catalog lock during client backpressure. Revocation, token expiry, shutdown or an in-place file change interrupts delivery; already-sent bytes cannot be withdrawn. Eight active streams have separate capacity from interactive JSON reads. A stalled body releases its stream worker after 30 seconds without demand.

The shared Rust handle manager allows at most 64 live handles and eight per authenticated member, including reads still finishing after cancellation. Handles expire after ten idle minutes and can be explicitly closed. Atomic file replacement preserves the opened generation. Opening collects SHA-256 hashes for fixed 64 KiB chunks during the existing full-file metadata pass. Reads verify each overlapping chunk before returning its requested bytes, so same-size edits with unchanged timestamps also fail closed. Size/mtime checks remain an early guard. The verification index uses at most 16 MiB per handle and 64 MiB across the manager; its maximum recording size is 32 GiB. A recording above that limit returns non-retryable 429 before hashing; exhausted shared capacity returns retryable 429. Admission remains held through cancelled reads.

Handles are process-local and disappear at service restart. Reopen after expiry/restart and compare the returned SHA-256 before resuming cached bytes. The native SDK requests at most 1 MiB, validates exact status/range/ETag/length and supports cancellation. HEAD validates the complete recording's headers without downloading its body.

The additive `recording_waveforms` capability provides waveform JSON bound to the lease's SHA-256 and ETag, with `If-Match` support. Decoding reads through the same verified pinned descriptor and memoizes the result in that lease. It never reads or publishes a canonical `audio.peaks.json` sidecar. Two workers across the shared lease manager bound decoding; cancellation retains admission until the actual worker exits. Decoding is limited to 30 seconds, 12 hours of audio, eight channels and 384 kHz. Responses are limited to 256 KiB. Authorization monitoring cancels work on expiry, membership removal or shutdown without holding a catalog read lock across decoding. A final authorization check precedes delivery.

The desktop's native `loofah-media` adapter now owns opaque recording URLs. Credentials and server paths stay in Rust. Resources are bound to their originating window and connection, with at most 64 resources, eight per window and eight simultaneous byte responses. Each response contains at most 1 MiB. Closing a view, losing access, disconnecting, signing out or destroying its window releases owned work. A missing server lease can be reopened once; a changed recording hash refuses continuation. Native media delivery accepts only the app's trusted origin (or its configured debug origin).

The `audio_playback` capability enables the common desktop player in the opt-in remote notes workspace. Both local and remote adapters use the same player and scoped resource/query lifecycle. Local playback retains device file URLs, cached local waveform helpers and its device capture/deletion guard; it requires no account or service. Remote playback uses authenticated native ranges and verified waveform JSON. Waveform failure uses flat precomputed peaks and media metadata, avoiding WaveSurfer's whole-file download/decode fallback. Revalidation preserves position and play state for the same remote SHA-256; a changed generation or capture transition resets playback. Resilient desktop recording uploads, authenticated live transcription, transcript repair/append and the remaining main-shell migration are unfinished. macOS native playback has been exercised at 50/150/500 ms RTT and across a backend restart; Windows WebView2 playback and production performance remain release gates.

The additive `recording_delete` capability enables the same checked deletion operation over IPC and HTTPS. Remote deletion requires editor/owner access, a UUID `X-Request-Id` and the displayed recording's lowercase SHA-256 in `expected_sha256`. Local deletion reads the current revision only when requested and retains the device queue and active-capture guard; it needs no login. The response is `true` when a recording was deleted and `false` when it was already absent. A stale revision returns 409, while active recording/import or exhausted reader capacity returns retryable 503; these precondition refusals report `not_committed`. The player never queues an offline deletion or automatically retries an uncertain result. Reload before choosing to retry. An explicit retry with the same hash cannot delete different recording bytes; request IDs are trace identifiers for this operation, not durable deletion receipts.

Before requesting a fresh authorization grant, deletion rechecks the bytes of every prepared recording descriptor outside the vault write lock. Windows shared file locks retain ordinary-write exclusion from this check through commit while allowing playback reads; conflicting locks refuse deletion as busy. File identity and metadata are checked again under the write lock before any owned path is removed. Timestamps alone cannot establish a recording revision. Failures during preparation or byte rechecking report `not_committed`; a failure after cleanup starts can have an unknown outcome.

Deletion reserves that session's audio through hashing and commit, including caller cancellation. Hashing releases the vault write lock so note editing remains available. Access and expiry are checked again after preparation, and a short catalog grant covers commit; waiting/hash work does not block membership revocation. Commit rechecks file identity/change metadata and validates the exact owned recording, waveform and transient names in the session and legacy `audio/` directory. Unknown files and dotfiles in either location survive. The selected primary recording is removed last so a partial cleanup failure leaves it available for a checked retry. Successful or already-absent deletion closes all session playback leases. This is per-file cleanup with safe retry, not an atomic multi-file transaction or a durable compound-deletion recovery journal. Native Windows validation and the broader crash-recovery gate remain required.

Contract types live in `crates/loofah-protocol`; generate TypeScript with `cargo run -p loofah-protocol --bin export-types`, or use `-- --check` to detect drift. Full note replacements require `expected_revision`. A stale replacement returns HTTP 409 with the current note and leaves that note unchanged. Vault management and membership updates also require the catalog revision; DELETE supplies it through the `expected_revision` query parameter.

SSE uses `Last-Event-ID` for bounded replay. A new connection, cursor gap or server epoch change produces `resync_required`; clients must reload authoritative snapshots. Streams close on token expiry, access removal or service shutdown. Changes are invalidations, not an offline synchronization log.

JSON requests have a 5 MiB body limit, note creation/replacement has a 4 MiB content limit and 4,096-byte title limit, and list/search pages are bounded. Checked note reads limit canonical files to 16 MiB and reject symlinks. Disconnecting does not roll back an admitted write. Creation receipts survive restarts; note revisions protect retries from overwriting a newer edit. The shared Rust path initializes tasks on creation and reconciles them on checked note saves, retaining matching IDs and metadata. Ordinary user frontmatter remains content; legacy exporter wrappers retain their existing read behavior.

The additive `metadata_edit` capability exposes authoritative session metadata snapshots and checked partial updates. PATCH accepts `patch` and a required `expected_revision`; a stale patch returns HTTP 409 with the current snapshot. Revisions cover raw `_meta.json` bytes, including fields unknown to this version. Writes reuse the existing metadata patch semantics and preserve unknown fields, notes, recordings and attachments. A title change makes an older note/title replacement stale; independent note-body edits do not invalidate a metadata revision. Missing/deleted sessions are not recreated. Fields omitted or set to null keep their existing values. Empty start/end strings retain the existing unset convention. Metadata tag assignments do not create tag-registry entries.

Metadata files are bounded to 8 MiB for these checked operations, with oversized growth rejected before commit and 16 MiB bounded HTTP responses. Text patch fields admit at most 4,096 bytes; tags admit at most 64 names of 4,096 bytes each. Supplied timestamps must be RFC 3339, except empty start/end values. Readers can inspect metadata but cannot patch it. The original desktop editing path remains in place until its transport migration; a separate metadata form is not mounted in the preview yet.

The additive `document_edit` capability exposes coherent summary/template snapshots and body replacement. A snapshot contains an optional `document` and an opaque `revision`. PUT accepts `markdown` and a required `expected_revision`; stale input returns HTTP 409 with the current snapshot. An absent summary can be created against its absent revision. An absent template output is never recreated by replacement. Output metadata, template identity and order are retained. Legacy summaries use the summary operation and the existing guarded migration path. These operations reconcile source tasks in Rust, retain stable matching task metadata, and apply explicitly changed checkboxes. Readers cannot replace content.

These document operations are experimental. Body/task persistence uses separate atomic files and a durable recovery record. Startup and shared content reads finish an admitted task projection only when the canonical body and task hashes permit safe recovery. An ambiguous external edit stops recovery for review. Later failures still report an unknown mutation outcome; clients retain unsaved input and reload canonical state. The opt-in desktop source output editor uses these body operations. The default local main rich Summary/template editor now uses the compound editor contract below; full remote main integration and wider feature parity remain open.

Checked note saves use the same recovery directory, including the possible intermediate state after the note body commits but before its title or tasks commit. Recovery completes the admitted title/task changes only against the recorded revisions. It preserves unrelated metadata and never recreates a deleted session. Metadata, note, document, task and agent reads recover under their continuous store lock; cancellation retains the lock until blocking work finishes. New note recovery records use schema 2; summary/template records retain schema 1. Older executables refuse schema 2 records rather than guessing how to recover them. Keep the binary and its matching storage backup together when rolling back.

The additive `tasks_edit` capability provides an opaque task-file revision and source-scoped replacements. A request supplies `source` (`raw_note`, `summary` or `template_output`), `tasks` and `expected_revision`. Stale input returns HTTP 409 with the current snapshot and writes nothing. Readers cannot replace tasks. Existing source files must belong to the requested session; unrelated sources, assignees and creation timestamps are preserved. Replacements allow at most 2,000 tasks and a 4 MiB serialized request. Checked task reads and prospective task files are bounded to 8 MiB; generated note/output tasks are checked before the new body commits. Snapshot/save responses are bounded to 16 MiB. This API does not yet provide cross-session task moves, and the desktop preview task list remains read-only.

The additive `content_read` capability covers the read-only content APIs. They share Rust operations and file-format types with local IPC. Four content reads can run concurrently, including response serialization on blocking workers. Serialized response limits are 1 MiB for registries, 8 MiB for tasks, and 32 MiB for documents or transcripts. Oversized results return a non-retryable `resource_limit` error; content is never silently truncated. Pagination/export for oversized content remains part of the full migration. Shutdown waits for admitted content reads as well as writes.

The additive `agent_read` capability exposes the existing CLI/MCP meeting projections through the same core, IPC and native HTTPS clients. Meeting, readable transcript, export and rendered segment responses reuse `agent-access`; notes, summaries, actions, word records, pagination, relevance and speaker labels retain their existing shapes. Transcript responses are complete within the explicit response limit and can be large. Shared-vault readers can use these read operations. Unknown attachments are never included.

Agent list/search POST requests are reads. Inputs retain the existing library limit clamps (200 list entries, 50 search hits); queries are limited to 8,192 bytes, filters to 64 tags of at most 4,096 bytes, and relevance-query offsets to 10,000. List/search responses are bounded to 8 MiB; other agent projections to 32 MiB, sharing the four content-reader permits. Composite file reads retain the store write lock until their blocking reads finish, including after request cancellation. This prevents mixing an admitted application write across one projection; it does not coordinate external filesystem writers. The CLI selects an explicit HTTPS profile with `loof --profile NAME`; `loof profiles login`, `list` and `logout` manage its device credentials. Existing read commands and all four MCP tools/resources reuse these projections. Unsupported remote operations fail explicitly without opening a local vault. Local CLI use needs no login. Local owner routing, remote writes/media/export and oversized-content export remain in progress.

The additive `note_editor_read` capability returns a note/title snapshot and its checked task snapshot under the same recovered store lock. A main editor can use one response to hydrate task IDs and metadata against a coherent baseline. Responses use the existing content-reader admission limit and a 32 MiB serialized response limit.

The editor snapshot includes only the requested note's raw-note tasks. Its opaque task revision still guards the entire canonical session task file, so a later checked save can preserve other sources.

The separate `note_editor_write` capability accepts `markdown`, optional `title`, `expected_revision`, `expected_tasks_revision` and explicit `tasks` in a compound PUT. Both revisions must match after recovery; otherwise HTTP 409 returns the current coherent editor snapshot and changes nothing. This guards task-only changes even when Markdown is unchanged. Task IDs, statuses, due dates and rich task bodies come from the editor's serialized state; existing Rust replacement rules retain assignment/creation metadata and tasks from other sources. The service does not round-trip rich content through a different Markdown converter. The shared operation bounds and validates input before committing, then uses the schema 2 note recovery record for body, title and task persistence. Metadata growth beyond 8 MiB is rejected before the note body can commit. Success and conflict responses use the same 32 MiB limit.

The native client refuses either editor operation when an older server lacks its advertised capability, before credential access or sending a request. An unsupported write is explicitly not committed. The opt-in notes workspace mounts the existing rich editor through these contracts, using the same scoped recovery/persistence lifecycle as source and output editing. It does not mount the main desktop shell or replace local startup.

### Compound summary and template editing

`document_editor_read` and `document_editor_write` add GET/PUT operations at
`sessions/{session}/summary/editor` and
`sessions/{session}/outputs/{document}/editor` under the selected vault API.
Snapshots contain the target document, session metadata and only that target's
tasks. Saves require `expected_revision`, `expected_metadata_revision` and
`expected_tasks_revision` together with Markdown, optional session title and
explicit tasks. All three revisions must match; HTTP 409 returns a coherent
current snapshot. The tasks revision covers the entire session tasks file, so
another task source changing also prevents a stale save.

These operations use the shared Rust transaction through IPC and HTTPS. They
preserve template frontmatter and output labels, unrelated documents, unknown
metadata and tasks from other sources. A session title edit does not rewrite the
raw note or another output. Summary reads may describe a missing summary; saving
creates it. A missing template output is not recreated. The existing remote
preview's source output editor remains available independently of this contract.

Compound output device recovery uses schema 3 with the physical session and
logical target, all revision baselines, explicit task state and the original
draft. Existing schemas 1/2 retain their original bytes until resolved. Older
clients reject schema 3 rather than treating it as a text-only draft. Successful
or replayed save snapshots are historical receipts: clients reread canonical
content before updating the scoped query cache. This is device recovery of
unsaved input, not offline remote editing.

New compound output intents use schema 5 in the existing `document-saves/`
directory. Preserve this directory in backups. Recover or drain these intents
with the current executable before a downgrade; an older executable cannot
recover them. Restore a complete matching pre-upgrade storage backup when
rolling back. Canonical vault files retain their existing format.

The additive `audio_metadata_read` capability returns the existing recording filename, content type, byte length and SHA-256, with the same camelCase shape as desktop metadata. It returns null when an existing session has no recording, 404 for a missing/inaccessible session, and retryable 503 while recording/import is active. Readers can use this operation. Only exact canonical recording names and the legacy `audio/` location are considered; unknown files, dotfiles and per-channel/transient captures are not selected. Symlinked recording paths are refused.

Rust opens a file descriptor under the store lock, then releases that lock before hashing through the existing portable implementation. Four per-vault reader slots bound embedded use. The server admits at most two recording metadata readers within its four global content-reader slots, keeping capacity for interactive content reads. Admission remains held through work after caller cancellation. Responses are bounded to 256 KiB and contain no server filesystem path. Scoped metadata queries invalidate on artifact changes and reconnect/resync, so note/title edits do not rehash a recording. The native client rejects an unsupported capability before credential access. This operation does not expose recording bytes, provide a playback lease or implement authenticated audio playback/downloads.

The additive `attachment_list` capability exposes GET
`/api/v1/vaults/{vault}/sessions/{session}/attachments?limit=100&after=...` to
readers. It returns `{ files: FileInfo[], next_cursor }`, ordered by exact filename,
with no server path or content hash. Pages accept 1–200 entries (default 100);
listing retains only one page plus lookahead and refuses scans above 50,000
entries. Hidden files, directories, symlinks and unknown session-root files are
not content. No directory is created for an empty list. The SDK checks page size,
name/MIME validity, ordering and advancing cursors. The shared desktop panel opens
native resources only when a row's actions are expanded; pagination performs no
media transfers. Attachment notifications and reconnects refresh all loaded
pages. The panel also mounts the shared upload and checked-removal controls described above.

## Tag editing

The `tag_editing` capability supplies the same checked tag operations over IPC and HTTPS. Readers can query registered and in-use suggestions with `POST /vaults/{vault}/tags/suggestions/query`. Editors and owners can register a tag with `POST /vaults/{vault}/tags/ensure`, or accept/dismiss a suggestion with `POST /vaults/{vault}/sessions/{session}/tag-suggestions/{accept|dismiss}`. Requests use typed shared contracts; no client filesystem paths are accepted.

Suggestion queries accept up to 4,096 query bytes, 256 exact exclusions and a result limit of 1–200. Results are bounded to 64 KiB. Tag normalization, query matching and ordering retain the current desktop behavior. Accept/dismiss requires the displayed metadata revision; a stale request returns HTTP 409 with current metadata. Manual add/remove uses the same checked metadata patch contract. These operations preserve unrelated top-level and nested metadata fields and refuse ambiguous duplicate JSON keys or corrupt registries. Full prospective file size is checked before a write.

Tag registration is append-only. If registration succeeds and a later metadata write fails, the registered tag may remain; the client must reload before explicitly deciding what to do next. An uncertain response never triggers an automatic metadata retry. The native client checks the capability before loading credentials, and default local use still requires no login.

## Empty-session cleanup

The optional `session_cleanup` capability exposes POST
`/api/v1/vaults/{vault}/session-cleanups`. Editors/owners send
`{ "session_id": "…" }` and an `x-request-id` UUID. IPC and the Rust/desktop
clients use the same application operation. Local IPC requires no login.

The operation checks recovered canonical files under the continuous deletion
guard and moves an empty session directory into the existing vault Trash. It
retains the existing emptiness rules and preserves every byte in that move.
Recording/import/upload reservations and live transcript buffers refuse cleanup;
oversized, corrupt or ambiguous content fails closed. Directory inspection is
bounded to 4,096 entries per relevant directory.

Results are `deleted`, `not_empty` or `not_found`, tagged by `kind`. They describe
the original request's decision. Repeating a UUID returns its historical result,
including after a restore; a `deleted` reply does not prove current absence and
must not trigger another deletion or current-session media closure. Reusing a
UUID with different input is a conflict. Interrupted operations only confirm the
recorded Trash generation when the source is absent; they never resume a move
against a current session. Ambiguous outcomes remain uncertain.

Keep `runtime/{vault}/creates/session-cleanups/` in consistent backups. These
schema-1 receipts have no automatic expiry. Local undo remains process-local;
this endpoint does not add remote restore. Callers must flush and check captured
editor drafts, reject reopened/retired candidates, and skip unknown pending
recording/upload state. Canonical emptiness cannot see another device's unsent
text. Main-tab automatic cleanup wiring and complete capture/STT reservation
integration are separate migration gates.

## Processing settings

A configured processing host advertises `processing_settings` alongside summary
jobs. Both the local IPC adapter and authenticated HTTPS adapter use the same
Rust settings operations. The opt-in notes workspace exposes them through
**Processing settings**. The default local app retains its existing settings,
OS credential storage and login-free experience.

| Operation                         | Endpoint                                         |
| --------------------------------- | ------------------------------------------------ |
| Inspect approved providers/models | `GET /api/v1/vaults/{vault}/processing/policy`   |
| Read vault processing settings    | `GET /api/v1/vaults/{vault}/processing/settings` |
| Save vault processing settings    | `PUT /api/v1/vaults/{vault}/processing/settings` |

The projection contains the summary provider/model/endpoint, output language,
custom summary template and high-confidence tag policy. It contains no device
appearance/capture preferences or credentials. PUT accepts `settings` and the
projection's `expected_revision`; it requires a UUID `X-Request-ID`. A stale save
returns a typed `conflict` outcome with the current snapshot and leaves it
unchanged. The UI retains its draft until you explicitly load the current
settings. It does not queue disconnected edits or automatically retry saves.

The operator's policy restricts remote providers, exact model IDs, endpoints and
custom templates. Invalid choices return validation errors without signing the
client out. Local settings preserve custom model IDs/endpoints and local-only
ChatGPT selection. Job execution still checks provider/account availability.
Host policy can be read without contacting models or reading their credentials.
There is no credential readback endpoint.

The revision covers processing fields rather than the whole config. Shared
writes read the current canonical `config.json`, preserve unrelated and unknown
fields and provider metadata, and serialize persistence/reset through one lock.
Cancellation after persistence starts cannot abandon the cache update or change
notification. Malformed and symlinked configs are not overwritten by partial
saves. `processing_settings_changed` invalidates only the selected vault's
settings; reconnect/resync reloads authoritative state. Confirmed save replies
cannot repopulate private caches after access loss.

## Summary and existing-output jobs

In the opt-in notes workspace, open a note's **Documents** tab and choose
**Summarize saved notes**. A configured, selected vault model is required;
operator allowlists alone do not select a model. The controls show private job
status, allow cancellation, and let you copy confirmed generated text after a
conflict or interruption. In an existing template output's editor, **Regenerate**
uses the same saved-note/transcript summary generator and preserves the output's
metadata. It does not recreate the retired template library or create a missing
template output. The optional **Generate a title if this note is untitled** control uses the same
saved request and shared worker. Streaming previews remain pending. The main local
AI interface is still active.

Generation first flushes the initiating session and target editors through their
existing save queues, then checks scoped device recovery before capturing the
target revision. Dirty, conflicted, uncertain or unreadable recovery state skips
generation and retains drafts. A preparation timeout leaves owned saves running.
Edits, access loss or scope replacement during preparation prevent dispatch.
This protects initiating-device drafts; another device's unsent edits remain
unknown to the service and are not merged automatically.

Before sending, the desktop saves the request UUID and declaration in its native
`job-submissions` recovery directory, scoped to the server/member/vault (or local
vault). This metadata is separate from canonical vault content and contains no
provider key or note body. Loading **Device summary requests** never submits work.
**Resume request** explicitly reuses the original UUID and input; it may start the
original request if it never reached the service. Acknowledged requests refresh
with a status read. Lost replies and failed device acknowledgement writes cannot
cause an automatic generation replay. Multiple desktop windows share a file lock. **Generate** atomically creates a
receipt only when no active receipt exists for that session and target. A stale
second window cannot resume an ambiguous request through Generate; use explicit
**Resume request** with the saved UUID instead.

**Forget device request** requires confirmation and removes only the device
receipt. It does not cancel a job or remove its server result; a new request may
run again. You can review receipts from other notes in the same vault to free the
bounded device store. Cancelling a known job uses the service operation, including
when its submitter has since become a reader. Access loss clears private result
caches and closes their views. None of this queues ordinary remote note edits.

Device receipts distinguish the session and document target: a summary and two
different existing outputs retain independent request IDs across restarts. A
duplicate active receipt for the same target is refused and retained for review.
Earlier releases accept only summary receipts; resolve template receipts with
the current version before rolling back. Recovery never resets an unreadable
receipt file or invents a new UUID.

Servers with processing configured advertise `summary_body_jobs`,
`template_output_jobs` and `generated_titles`. Summary operations require the first;
submitting a template-output job also requires the second. An optional title
request requires `generated_titles`. The Rust SDK rejects an unsupported capability
before prompting for credentials or dispatching a mutation. Native local jobs
use the same operations without a Loofah login.

| Operation                      | Endpoint                                        |
| ------------------------------ | ----------------------------------------------- |
| Submit                         | `POST /api/v1/vaults/{vault}/jobs`              |
| List the current member's jobs | `GET /api/v1/vaults/{vault}/jobs`               |
| Inspect status                 | `GET /api/v1/vaults/{vault}/jobs/{job}`         |
| Read a retained result         | `GET /api/v1/vaults/{vault}/jobs/{job}/result`  |
| Request cancellation           | `POST /api/v1/vaults/{vault}/jobs/{job}/cancel` |

Use the normal API-major and bearer headers. Submit and cancel also require a
client-generated UUID in `X-Request-Id`. Submission accepts only:

```json
{
  "session_id": "session-id",
  "target": { "kind": "summary" },
  "expected_revision": "revision-from-the-summary-snapshot"
}
```

For an existing template output, use
`"target": { "kind": "template_output", "document_id": "existing-output-id" }`
and its current snapshot revision. Both targets apply bodies, reconciled tasks
and the configured tag policy with the same revision/conflict guarantees. Missing
or legacy template targets fail before a provider call. Provider endpoints, keys
and job owners are not request fields.

Omitting `title_policy` preserves the current title and keeps legacy request bytes
unchanged. With `generated_titles`, either target also accepts
`"title_policy": "generate_if_empty"`. The worker confirms the body commit first,
then generates a title from that saved body using the same prepared model and
settings. It applies the candidate only against its captured empty-title baseline;
newer titles or captured output edits cause a title conflict. The existing title
policy also stamps a leading title heading into the captured summary/output
bodies, and into the raw note only if that note still matches its baseline. The
retained generated text remains the original model output. Unknown and legacy
metadata fields survive, and the complete prospective metadata size is checked.

The retained result's optional `title` describes this separate stage and may
include a candidate for explicit copying. Empty, failed, cancelled, conflicted or
unconfirmed title work keeps a confirmed body successful. The generated body stays
available during title work. A newer draft on the initiating device remains a
draft; result viewing never writes a title or loads an old snapshot into it.

Opt-in jobs use private job journal schema 2 and a bounded title sidecar. Preserve
jobs retain schema 1. Restart never repeats paid body or title generation: a
confirmed body remains successful, while a crash before its confirmation can leave
an interrupted result needing review. A candidate not durably recorded before a
crash is unavailable for recovery. Older executables cannot read these opt-in job
records; drain/review them with this version or restore a matching binary and
complete storage backup when rolling back. Canonical vault formats do not change.

Keep the original UUID and declaration when retrying a lost submission reply.
Receipt replay returns the same job even after its result was applied; it does
not start another provider call. A different declaration with that UUID returns
conflict. A stale document at execution produces a conflict job, and later edits
to that target body cannot be overwritten by an older generation. Job work continues when its
submission connection, subscription or frontend closes. Explicit cancellation is
cooperative; it cannot undo a committed body. Cancelling an already completed job
returns its existing terminal state.

List supports `session_id`, `active_only`, `limit` (1–200, default 50) and `after`.
Pages sort newest first with job ID tie-breaking. `next_cursor` is the last
returned job ID when another page exists. Cursors are scoped to the current
member and vault. Status, listing, cancellation and retained results are private
to the submitting member, including in shared vaults; another editor or owner
cannot obtain a member's private draft by knowing its job ID. Readers cannot
submit new work but may inspect or cancel their own earlier jobs while they retain
vault access. Removed members lose access immediately.

The result endpoint returns null when no validated result is available. Otherwise
it returns the vault/job/session/target binding, Markdown and optional tag
suggestions. Results can survive cancellation or a conflict without being applied.
The SDK checks response bindings and page bounds before returning data. Durable
status/result reads remain available after a worker exits; orphaned active jobs
are marked interrupted for review. New submission stays disabled until recovery.
An admission stop is distinct from actual worker exit, so active shutdown drain
cannot prematurely interrupt a result that is still being retained.

The existing authenticated change stream delivers `job_changed` invalidations
only to the submitting member, for both replay and live delivery. Canonical
document/index changes remain visible to authorized vault members. Filtered
private events still consume the bounded replay history; overflow requires a
normal scoped resync. Progress is queryable independently of events. Optional
response fields can be added within API major 1; request fields remain strict,
and new operations require capability negotiation. Job bookkeeping has its own
storage schema version, separate from API and canonical vault formats.

Internal generated-title storage additionally supports schema-4 recovery. Its
opaque baseline binds the owning vault context and guards the title plus all
mandatory output revisions before writing. Template frontmatter and task files
are retained; raw-note stamping may independently skip a changed or unreadable
note. Public processing jobs do not yet request generated titles. Drain any
schema-4 intents with the current version before downgrading; older readers must
not attempt their recovery.

## Storage, backups and upgrades

`storage_root` is a dedicated service directory, separate from `cache_root`:

- `catalog.json` contains identities, vault memberships and shared-vault creation receipts.
- `vaults/{uuid}/` contains canonical vault files in the existing Loofah format.
- `runtime/{uuid}/creates/` contains durable session-creation intents and receipts. Its `document-saves/` subdirectory holds pending body/task recovery records, removed after completion. These records are part of a consistent backup.
- `cache_root/search_index/` contains rebuildable search projections.

The server rejects symlinks in server-owned vaults. Vault IDs select catalog-managed directories; client requests cannot select server filesystem paths. Unknown session files remain user attachments and are never indexed as note content.

Stop the service before a consistent backup or restore. Copy the entire storage root, including unknown attachment files and runtime receipts, and back up the configuration separately. Caches can be rebuilt. Preserve directory ownership and permissions when restoring. Do not delete a live lock file to bypass ownership.

A stopped-storage integration test restores the complete storage root with an empty cache, checks file hashes, identities, access rules and creation replay, and resumes a sealed capture through publication while preserving unknown files. It validates the current binary; cross-version upgrade, downgrade and operator restore acceptance remain required.

Before upgrading, stop the service, take a backup, replace the executable and check `--version`. Startup rejects unsupported catalog versions. Restore the prior binary and its matching backup if a future migration requires rollback. Direct filesystem maintenance is performed while writes are stopped; multiple writable service instances and writable shared network mounts are unsupported.

## Isolated native UI tests

The native client has an explicit `dev-fixtures` feature for loopback HTTPS fixtures with a supplied CA. Certificate and hostname checks remain enabled. The desktop hook is available only with `--features dev`, bundle identifier `io.loofah.remote-qa`, and a native `LOOFAH_QA_ROOT_CA` file (at most 128 KiB). It is excluded from normal, staging and devtools builds. Keep its disposable debug profile and all fixture/build artifacts on T7; use synthetic identities and data. No OS trust-store change is needed. This hook is for native UI validation, not a server deployment option.

## Checked session lifecycle

The `session_lifecycle` capability exposes `GET /vaults/{vault}/sessions/{session}/lifecycle`,
`POST /vaults/{vault}/session-deletions` and `POST /vaults/{vault}/session-restorations`.
Reads return an opaque revision. Mutations require current editor/owner access,
a caller UUID and the expected revision; restore also requires the opaque Trash
token returned by deletion. Changed content returns a conflict, never a forced
overwrite. A completed replay returns the historical decision without deleting
or recreating a later session. Cross-vault tokens are rejected. Private receipts
and captures/jobs prevent lifecycle changes while work or its outcome is unknown.
The complete directory, including unknown user attachments, moves intact. No
local paths are exposed by these remote contracts.

These operations are also thin Native IPC wrappers over the same Core. The Local
frontend capability remains disabled until the legacy recording/batch persistence
barrier is installed; existing Local deletion and undo remain available during
that migration. Shared frontend lifecycle actions are being integrated.

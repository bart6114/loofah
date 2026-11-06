# Run the Loofah server

The server is experimental. It currently exposes authenticated vault, note, existing-content, search and change-stream APIs. Desktop server connections, server AI processing, media and resilient remote recording are still being implemented. The desktop's default local mode continues to work without an account or server.

For development, set `VITE_REMOTE_PREVIEW=true` when starting the desktop frontend. **Settings → Storage → Try server notes** opens the opt-in notes preview. Connect to an HTTPS server, sign in through the company browser flow, and select a personal or shared vault. The preview supports note creation, Markdown source editing, search, revision conflicts and device draft recovery. Existing summaries, template outputs, transcripts and tasks can be viewed as text. With the `document_edit` capability, **Outputs → Edit** edits summary/template bodies, and **Add summary** opens a missing summary. These body saves reconcile tasks in Rust; template titles and metadata stay unchanged. Transcripts/tasks remain read-only in this preview. It keeps tokens in native Rust and the OS credential store. Editing pauses during a disconnect; saved recovery drafts stay on the device. Readers cannot edit.

**Device recovery drafts** opens a separate read-only dialog for the selected account/vault. Storage settings also open an inventory across signed-out accounts and removed vaults before server login. These views find saved note and output copies from closed windows even when their original resource has been deleted. Open a copy and use **Copy saved text** to recover its exact Markdown. Reloading and copying work during a network interruption. These views neither recreate a resource nor write to the server, and they keep the original recovery records. The selected-vault view closes when access is lost; the device inventory remains available without a connection.

Copies containing explicit editor task state also offer **Copy all recovery data**, including both revision baselines, task identities, statuses, dates and rich bodies. These device records use schema 2; existing text-only copies remain schema 1. Older apps refuse schema 2 instead of discarding task intent. A text-only editor cannot replace a task-aware recovery record. The compound editor draft controller retains task changes during pending saves, lost responses and reviewed conflicts; mounting the main editor remains in progress.

Use **Create shared vault** for a separate company vault. **Vault members** shows its members; owners can add existing company members, set reader/editor/owner roles, remove members, rename the vault or explicitly delete its access. The last owner cannot be removed or downgraded. Personal vaults have no sharing controls. Catalog conflicts require a reload before another change. An uncertain creation retries its original request rather than silently creating another vault.

This preview runs inside the existing local desktop host. It does not yet replace the main ProseMirror editor or provide remote-only startup, attachments, processing or recording. Do not use it as a complete remote deployment of Loofah. Keep `VITE_REMOTE_PREVIEW` unset for the existing standalone experience.

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
| Read/patch session metadata            | `GET` / `PATCH /api/v1/vaults/{vault}/sessions/{session}/metadata`                                                                        |
| Read/save a note                       | `GET` / `PUT /api/v1/vaults/{vault}/sessions/{session}/note`                                                                              |
| Read summaries/template outputs        | `GET /api/v1/vaults/{vault}/sessions/{session}/documents`                                                                                 |
| Read/replace summary Markdown          | `GET` / `PUT /api/v1/vaults/{vault}/sessions/{session}/summary`                                                                           |
| Read/replace template Markdown         | `GET` / `PUT /api/v1/vaults/{vault}/sessions/{session}/outputs/{document}`                                                                |
| Read complete transcript records       | `GET /api/v1/vaults/{vault}/sessions/{session}/transcripts`                                                                               |
| Read session tasks                     | `GET /api/v1/vaults/{vault}/sessions/{session}/tasks`                                                                                     |
| Read/save a coherent note editor       | `GET` / `PUT /api/v1/vaults/{vault}/sessions/{session}/note/editor`                                                                       |
| Read checked task snapshot             | `GET /api/v1/vaults/{vault}/sessions/{session}/tasks/snapshot`                                                                            |
| Replace a task source                  | `PUT /api/v1/vaults/{vault}/sessions/{session}/tasks`                                                                                     |
| Read people/tag registries             | `GET /api/v1/vaults/{vault}/people`, `GET …/tags`                                                                                         |
| Search                                 | `POST /api/v1/vaults/{vault}/search`                                                                                                      |
| List/search existing agent projections | `POST /api/v1/vaults/{vault}/agent/sessions/list`, `POST …/agent/sessions/search`                                                         |
| Read agent meeting/transcript/export   | `GET /api/v1/vaults/{vault}/agent/sessions/{session}`, `GET …/{session}/transcript`, `GET …/{session}/export`, `GET …/{session}/segments` |
| Subscribe                              | `GET /api/v1/vaults/{vault}/events`                                                                                                       |

Contract types live in `crates/loofah-protocol`; generate TypeScript with `cargo run -p loofah-protocol --bin export-types`, or use `-- --check` to detect drift. Full note replacements require `expected_revision`. A stale replacement returns HTTP 409 with the current note and leaves that note unchanged. Vault management and membership updates also require the catalog revision; DELETE supplies it through the `expected_revision` query parameter.

SSE uses `Last-Event-ID` for bounded replay. A new connection, cursor gap or server epoch change produces `resync_required`; clients must reload authoritative snapshots. Streams close on token expiry, access removal or service shutdown. Changes are invalidations, not an offline synchronization log.

JSON requests have a 5 MiB body limit, note creation/replacement has a 4 MiB content limit and 4,096-byte title limit, and list/search pages are bounded. Checked note reads limit canonical files to 16 MiB and reject symlinks. Disconnecting does not roll back an admitted write. Creation receipts survive restarts; note revisions protect retries from overwriting a newer edit. The shared Rust path initializes tasks on creation and reconciles them on checked note saves, retaining matching IDs and metadata. Ordinary user frontmatter remains content; legacy exporter wrappers retain their existing read behavior.

The additive `metadata_edit` capability exposes authoritative session metadata snapshots and checked partial updates. PATCH accepts `patch` and a required `expected_revision`; a stale patch returns HTTP 409 with the current snapshot. Revisions cover raw `_meta.json` bytes, including fields unknown to this version. Writes reuse the existing metadata patch semantics and preserve unknown fields, notes, recordings and attachments. A title change makes an older note/title replacement stale; independent note-body edits do not invalidate a metadata revision. Missing/deleted sessions are not recreated. Fields omitted or set to null keep their existing values. Empty start/end strings retain the existing unset convention. Metadata tag assignments do not create tag-registry entries.

Metadata files are bounded to 8 MiB for these checked operations, with oversized growth rejected before commit and 16 MiB bounded HTTP responses. Text patch fields admit at most 4,096 bytes; tags admit at most 64 names of 4,096 bytes each. Supplied timestamps must be RFC 3339, except empty start/end values. Readers can inspect metadata but cannot patch it. The original desktop editing path remains in place until its transport migration; a separate metadata form is not mounted in the preview yet.

The additive `document_edit` capability exposes coherent summary/template snapshots and body replacement. A snapshot contains an optional `document` and an opaque `revision`. PUT accepts `markdown` and a required `expected_revision`; stale input returns HTTP 409 with the current snapshot. An absent summary can be created against its absent revision. An absent template output is never recreated by replacement. Output metadata, template identity and order are retained. Legacy summaries use the summary operation and the existing guarded migration path. These operations reconcile source tasks in Rust, retain stable matching task metadata, and apply explicitly changed checkboxes. Readers cannot replace content.

These document operations are experimental. Body/task persistence uses separate atomic files and a durable recovery record. Startup and shared content reads finish an admitted task projection only when the canonical body and task hashes permit safe recovery. An ambiguous external edit stops recovery for review. Later failures still report an unknown mutation outcome; clients retain unsaved input and reload canonical state. The opt-in desktop output editor uses these operations; main-editor integration and wider feature parity remain open.

Checked note saves use the same recovery directory, including the possible intermediate state after the note body commits but before its title or tasks commit. Recovery completes the admitted title/task changes only against the recorded revisions. It preserves unrelated metadata and never recreates a deleted session. Metadata, note, document, task and agent reads recover under their continuous store lock; cancellation retains the lock until blocking work finishes. New note recovery records use schema 2; summary/template records retain schema 1. Older executables refuse schema 2 records rather than guessing how to recover them. Keep the binary and its matching storage backup together when rolling back.

The additive `tasks_edit` capability provides an opaque task-file revision and source-scoped replacements. A request supplies `source` (`raw_note`, `summary` or `template_output`), `tasks` and `expected_revision`. Stale input returns HTTP 409 with the current snapshot and writes nothing. Readers cannot replace tasks. Existing source files must belong to the requested session; unrelated sources, assignees and creation timestamps are preserved. Replacements allow at most 2,000 tasks and a 4 MiB serialized request. Checked task reads and prospective task files are bounded to 8 MiB; generated note/output tasks are checked before the new body commits. Snapshot/save responses are bounded to 16 MiB. This API does not yet provide cross-session task moves, and the desktop preview task list remains read-only.

The additive `content_read` capability covers the read-only content APIs. They share Rust operations and file-format types with local IPC. Four content reads can run concurrently, including response serialization on blocking workers. Serialized response limits are 1 MiB for registries, 8 MiB for tasks, and 32 MiB for documents or transcripts. Oversized results return a non-retryable `resource_limit` error; content is never silently truncated. Pagination/export for oversized content remains part of the full migration. Shutdown waits for admitted content reads as well as writes.

The additive `agent_read` capability exposes the existing CLI/MCP meeting projections through the same core, IPC and native HTTPS clients. Meeting, readable transcript, export and rendered segment responses reuse `agent-access`; notes, summaries, actions, word records, pagination, relevance and speaker labels retain their existing shapes. Transcript responses are complete within the explicit response limit and can be large. Shared-vault readers can use these read operations. Unknown attachments are never included.

Agent list/search POST requests are reads. Inputs retain the existing library limit clamps (200 list entries, 50 search hits); queries are limited to 8,192 bytes, filters to 64 tags of at most 4,096 bytes, and relevance-query offsets to 10,000. List/search responses are bounded to 8 MiB; other agent projections to 32 MiB, sharing the four content-reader permits. Composite file reads retain the store write lock until their blocking reads finish, including after request cancellation. This prevents mixing an admitted application write across one projection; it does not coordinate external filesystem writers. The CLI selects an explicit HTTPS profile with `loof --profile NAME`; `loof profiles login`, `list` and `logout` manage its device credentials. Existing read commands and all four MCP tools/resources reuse these projections. Unsupported remote operations fail explicitly without opening a local vault. Local CLI use needs no login. Local owner routing, remote writes/media/export and oversized-content export remain in progress.

The additive `note_editor_read` capability returns a note/title snapshot and its checked task snapshot under the same recovered store lock. A main editor can use one response to hydrate task IDs and metadata against a coherent baseline. Responses use the existing content-reader admission limit and a 32 MiB serialized response limit.

The editor snapshot includes only the requested note's raw-note tasks. Its opaque task revision still guards the entire canonical session task file, so a later checked save can preserve other sources.

The separate `note_editor_write` capability accepts `markdown`, optional `title`, `expected_revision`, `expected_tasks_revision` and explicit `tasks` in a compound PUT. Both revisions must match after recovery; otherwise HTTP 409 returns the current coherent editor snapshot and changes nothing. This guards task-only changes even when Markdown is unchanged. Task IDs, statuses, due dates and rich task bodies come from the editor's serialized state; existing Rust replacement rules retain assignment/creation metadata and tasks from other sources. The service does not round-trip rich content through a different Markdown converter. The shared operation bounds and validates input before committing, then uses the schema 2 note recovery record for body, title and task persistence. Metadata growth beyond 8 MiB is rejected before the note body can commit. Success and conflict responses use the same 32 MiB limit.

The native client refuses either editor operation when an older server lacks its advertised capability, before credential access or sending a request. An unsupported write is explicitly not committed. These contracts do not yet mount the main editor; its recovery and save lifecycle still require migration.

## Storage, backups and upgrades

`storage_root` is a dedicated service directory, separate from `cache_root`:

- `catalog.json` contains identities, vault memberships and shared-vault creation receipts.
- `vaults/{uuid}/` contains canonical vault files in the existing Loofah format.
- `runtime/{uuid}/creates/` contains durable session-creation intents and receipts. Its `document-saves/` subdirectory holds pending body/task recovery records, removed after completion. These records are part of a consistent backup.
- `cache_root/search_index/` contains rebuildable search projections.

The server rejects symlinks in server-owned vaults. Vault IDs select catalog-managed directories; client requests cannot select server filesystem paths. Unknown session files remain user attachments and are never indexed as note content.

Stop the service before a consistent backup or restore. Copy the entire storage root, including unknown attachment files and runtime receipts, and back up the configuration separately. Caches can be rebuilt. Preserve directory ownership and permissions when restoring. Do not delete a live lock file to bypass ownership.

Before upgrading, stop the service, take a backup, replace the executable and check `--version`. Startup rejects unsupported catalog versions. Restore the prior binary and its matching backup if a future migration requires rollback. Direct filesystem maintenance is performed while writes are stopped; multiple writable service instances and writable shared network mounts are unsupported.

## Isolated native UI tests

The native client has an explicit `dev-fixtures` feature for loopback HTTPS fixtures with a supplied CA. Certificate and hostname checks remain enabled. The desktop hook is available only with `--features dev`, bundle identifier `io.loofah.remote-qa`, and a native `LOOFAH_QA_ROOT_CA` file (at most 128 KiB). It is excluded from normal, staging and devtools builds. Keep its disposable debug profile and all fixture/build artifacts on T7; use synthetic identities and data. No OS trust-store change is needed. This hook is for native UI validation, not a server deployment option.

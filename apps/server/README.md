# Run the Loofah server

The server is experimental. It currently exposes authenticated vault, note, existing-content, search and change-stream APIs. Desktop server connections, server AI processing, media and resilient remote recording are still being implemented. The desktop's default local mode continues to work without an account or server.

For development, set `VITE_REMOTE_PREVIEW=true` when starting the desktop frontend. **Settings → Storage → Try server notes** opens the opt-in notes preview. Connect to an HTTPS server, sign in through the company browser flow, and select a personal or shared vault. The preview supports note creation, Markdown source editing, search, revision conflicts and device draft recovery. Existing summaries, template outputs, transcripts and tasks can be viewed as text; their editing and task reconciliation are still being migrated. It keeps tokens in native Rust and the OS credential store. Editing pauses during a disconnect; saved recovery drafts stay on the device. Readers cannot edit.

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

| Operation                           | Endpoint                                                                                       |
| ----------------------------------- | ---------------------------------------------------------------------------------------------- |
| Provision/read the signed-in member | `POST` / `GET /api/v1/me`                                                                      |
| List existing company members       | `GET /api/v1/members`                                                                          |
| List/create vaults                  | `GET` / `POST /api/v1/vaults`                                                                  |
| Read/rename/delete shared vault     | `GET` / `PATCH` / `DELETE /api/v1/vaults/{vault}`                                              |
| Read/change membership              | `GET /api/v1/vaults/{vault}/members`, `PUT` / `DELETE /api/v1/vaults/{vault}/members/{member}` |
| List/create/read sessions           | `GET` / `POST /api/v1/vaults/{vault}/sessions`, `GET …/sessions/{session}`                     |
| Read/save a note                    | `GET` / `PUT /api/v1/vaults/{vault}/sessions/{session}/note`                                   |
| Read summaries/template outputs     | `GET /api/v1/vaults/{vault}/sessions/{session}/documents`                                      |
| Read/replace summary Markdown       | `GET` / `PUT /api/v1/vaults/{vault}/sessions/{session}/summary`                                |
| Read/replace template Markdown      | `GET` / `PUT /api/v1/vaults/{vault}/sessions/{session}/outputs/{document}`                     |
| Read complete transcript records    | `GET /api/v1/vaults/{vault}/sessions/{session}/transcripts`                                    |
| Read session tasks                  | `GET /api/v1/vaults/{vault}/sessions/{session}/tasks`                                          |
| Read people/tag registries          | `GET /api/v1/vaults/{vault}/people`, `GET …/tags`                                              |
| Search                              | `POST /api/v1/vaults/{vault}/search`                                                           |
| Subscribe                           | `GET /api/v1/vaults/{vault}/events`                                                            |

Contract types live in `crates/loofah-protocol`; generate TypeScript with `cargo run -p loofah-protocol --bin export-types`, or use `-- --check` to detect drift. Full note replacements require `expected_revision`. A stale replacement returns HTTP 409 with the current note and leaves that note unchanged. Vault management and membership updates also require the catalog revision; DELETE supplies it through the `expected_revision` query parameter.

SSE uses `Last-Event-ID` for bounded replay. A new connection, cursor gap or server epoch change produces `resync_required`; clients must reload authoritative snapshots. Streams close on token expiry, access removal or service shutdown. Changes are invalidations, not an offline synchronization log.

Requests and write workers are bounded. JSON requests have a 5 MiB body limit, note creation has a 4 MiB content limit, and list/search pages are bounded. Disconnecting does not roll back an admitted write. Creation receipts survive restarts; note revisions protect retries from overwriting a newer edit.

The additive `document_edit` capability exposes coherent summary/template snapshots and body replacement. A snapshot contains an optional `document` and an opaque `revision`. PUT accepts `markdown` and a required `expected_revision`; stale input returns HTTP 409 with the current snapshot. An absent summary can be created against its absent revision. An absent template output is never recreated by replacement. Output metadata, template identity and order are retained. Legacy summaries use the summary operation and the existing guarded migration path. These operations reconcile source tasks in Rust, retain stable matching task metadata, and apply explicitly changed checkboxes. Readers cannot replace content.

These document operations are experimental. Body/task persistence uses separate atomic files and a durable recovery record. Startup and shared content reads finish an admitted task projection only when the canonical body and task hashes permit safe recovery. An ambiguous external edit stops recovery for review. Later failures still report an unknown mutation outcome; clients retain unsaved input and reload canonical state. The desktop output editor and wider feature parity remain open.

The additive `content_read` capability covers the read-only content APIs. They share Rust operations and file-format types with local IPC. Four content reads can run concurrently, including response serialization on blocking workers. Serialized response limits are 1 MiB for registries, 8 MiB for tasks, and 32 MiB for documents or transcripts. Oversized results return a non-retryable `resource_limit` error; content is never silently truncated. Pagination/export for oversized content remains part of the full migration. Shutdown waits for admitted content reads as well as writes.

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

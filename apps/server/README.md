# Company vault service

`loofah-server` hosts personal and shared Loofah vaults on one company's server.
Vault files are authoritative. The service uses JSON metadata for identities,
membership, sessions, upload progress, recovery journals, and deletion archives.
Search indexes under `cache/` can be rebuilt. No database is required.

## Run

Build with `cargo build --locked --release -p loofah-server`, or build the container
from the repository root:

```sh
docker build -f apps/server/Dockerfile -t loofah-server .
```

Create a private `server.json` outside the vault directories:

```json
{
  "data_dir": "/data",
  "public_url": "https://notes.example.com",
  "issuer": "https://identity.example.com/realms/company",
  "client_id": "loofah-storage",
  "client_secret": null,
  "max_upload_bytes": 21474836480
}
```

Register an OIDC client with the exact redirect URI
`https://notes.example.com/v1/auth/callback`. Enable the authorization-code flow,
PKCE S256, and the `openid`, `profile`, and `email` scopes. Supply `client_secret`
for a confidential OIDC client. Restrict access to this client in the identity
provider to the intended company. Every permitted identity can provision a
personal vault on first login. Email changes retain the same account because
identity is mapped by issuer and subject.

Run `loofah-server --config /path/server.json serve`. The default listener is
`127.0.0.1:8947`; use `--listen 0.0.0.0:8947` when needed inside a container.
The configuration must be readable by the service account (UID 10001 in the container); keep its permissions private when it contains a client secret.
The container expects the configuration at `/config/server.json`, a writable
persistent volume at `/data` owned by UID 10001, and HTTPS termination at a
reverse proxy. Forward `/v1/`, allow request bodies of at least 8 MiB, disable
proxy buffering for `/v1/events`, and allow long-lived SSE connections. Use one
service process and one persistent filesystem; the data-directory lock rejects a
second writer. Storage must support file locks, atomic rename, hard links, and
file/directory fsync within the same filesystem. Operators must not edit live
vault files.

`GET /health` checks the process; `GET /ready` checks writable service storage.
`GET /v1/openapi.json` exposes the generated HTTP contract. Errors have `code`,
`message`, and an optional `operation_id`. RPC queries return component revisions;
replacement mutations supply those revisions in `expected`, plus a stable
`mutation_id`. A stale replacement receives 412; a missing revision receives 428.
Retry the same mutation ID to retrieve its recorded outcome. Audio and managed
attachments use bounded resumable uploads with SHA-256 verification. Media links
are short-lived, scoped to an authenticated user and artifact, and recheck access
on every request. SSE clients refresh on `reset`; cursors are process-scoped.

In desktop **Settings → Storage**, enter the server origin and complete company
sign-in in the browser. Choose the personal vault or a shared vault. Shared vault
owners can create vaults, rename them, and manage owner/editor/viewer membership.
The last owner cannot leave or be demoted. Members must sign in once before they
can be added. Desktop credentials use macOS Keychain. Pending edits and uploads
remain scoped to their original server, user, and vault; conflicts require review.
This first version edits company vaults in the main window; standalone company note windows are disabled to keep editor revision ownership unambiguous. Close local standalone note windows before switching vaults. Active transcription or summary jobs also block switching so their results stay with the original vault.
The app fetches content on demand and uses temporary files for recording and
processing. It does not maintain an offline vault mirror.

Transcription and summaries continue to use the providers selected in the client.
This service stores their results and does not invoke transcription or LLMs.

Revision baselines are conservative: background refreshes cannot advance an open
editor’s save baseline. After another client edits a document, saving may require
reviewing the conflict even if a refreshed view already shows the latest text.

## Maintenance and recovery

Stop the service before maintenance. Each command acquires the same exclusive
lock as the server:

- `loofah-server --config server.json recover` replays unfinished transactions and
  verifies that vaults can be indexed.
- `loofah-server --config server.json import /external/vault VAULT_UUID` imports
  into an existing empty vault. It migrates a copy, rejects symlinks, preserves
  unknown attachments, and excludes device `config.json`.
- `loofah-server --config server.json disable-user USER_UUID` disables an account
  and revokes its sessions. Add `--enable` to re-enable it.

For a consistent backup, stop the service and copy the entire data directory,
including `service/`, `vaults/`, and the configuration stored outside it. Restore
into an empty directory, preserve ownership, run `recover`, then start the service.
Keep the original server UUID and identity registry so clients' queued work still
addresses the same destination. Restoring an older backup also restores its old
credential state; require users to sign in again by clearing the registry's
`sessions` object while stopped. Test restores regularly. `cache/` can be omitted.
Session deletion moves the whole directory into the service's persistent trash;
`session_restore` restores it. Removed managed files are archived in `trash-files/`.

Journals retain mutation receipts, and trash retains deleted content. Plan disk
capacity and an operator retention policy before production. Incomplete uploads
and expired media tickets can be removed while stopped; never remove a pending
transaction's staged files. There is no multi-process deployment or online
external filesystem editing support in this first version. The storage adapter
boundary is in `crates/vault-access`; filesystem is the initial implementation.
WebDAV and S3 adapters, mobile integration, and processing servers are separate work.

## Verification

```sh
cargo test --locked -p loofah-server -p vault-access -p vault-write
cargo clippy --locked -p loofah-server -p vault-access --all-targets --no-deps -- -D warnings
pnpm -F desktop test:storage
cargo test --locked -p loofah-server --test scale -- --ignored --nocapture
```

The scale test creates 50,000 sessions and runs 50 simultaneous HTTP clients,
reporting cold-index time and read/write latency percentiles. It is intentionally
excluded from routine CI; `LOOFAH_PERF_SESSIONS` changes the fixture size.

For the Rust production coverage gate, install `cargo-llvm-cov` 0.9.1 and the
`llvm-tools-preview` toolchain component, then run:

```sh
cargo llvm-cov --locked -p loofah-server -p vault-access -p vault-write \
  --ignore-filename-regex '(/tests/|/tests\.rs$)' \
  --json --summary-only --output-path target/storage-coverage.json
python3 scripts/check-storage-coverage.py target/storage-coverage.json
```

CI requires 90% aggregate line coverage across the service, client, filesystem
adapter, recovery spool, native login protocol, and shared command adapter, with
at least 85% in each production file. Test bodies are excluded. The desktop UI
gate requires 90% lines/statements, 85% branches, and 80% functions in each company
storage UI file. Native Tauri IPC round-trips against the real service also run in
macOS CI, including conflict recovery and the client's tag-application setting.

The suites exercise public HTTP contracts, owner/editor/viewer/outsider access,
identity persistence, OIDC proof failures, concurrent refresh and role changes,
lost upload responses, resumed transfers, corruption, media expiry, and local
draft isolation. Journal tests interrupt writes and recovery at durability
boundaries; process tests kill and restart the binary and restore backups into a
different data directory. Filesystem write reordering and power loss need a
filesystem-level fault harness. Route tests use in-process
Axum requests; SDK, callback, SSE, and process lifecycle tests use real sockets.

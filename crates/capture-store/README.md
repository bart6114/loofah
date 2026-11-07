# Capture store

Reusable file-backed capture persistence for the desktop device spool and future headless ingestion. This crate owns leases, quotas, immutable checksummed chunks, replay, durable seals and receipt observations. It has no HTTP client, authentication, device capture or Tauri dependency.

The implementation moved from `loofah-client::capture_spool` without changing v1 storage bytes, identity/hash domains or cleanup policy. `loofah-client` reexports the original module path for existing consumers; server/application code can depend directly on `hypr-capture-store`. Wire types and digest functions remain in `loofah-protocol`.

Scope selection and authorization belong to the caller. A stored acknowledgment does not authorize deleting audio. This extraction adds no uploader, timestamps, server endpoints or general offline editing.

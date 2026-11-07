# Capture store

Reusable file-backed capture persistence for the desktop device spool and future headless ingestion. This crate owns leases, quotas, immutable checksummed chunks, replay, durable seals and receipt observations. It has no HTTP client, authentication, device capture or Tauri dependency.

The implementation moved from `loofah-client::capture_spool` without changing v1 storage bytes, identity/hash domains or cleanup policy. `loofah-client` reexports the original module path for existing consumers; server/application code can depend directly on `hypr-capture-store`. Wire types and digest functions remain in `loofah-protocol`.

Scope selection and authorization belong to the caller. A stored acknowledgment does not authorize deleting audio. This crate adds no uploader, server endpoints or general offline editing.

`CaptureWriter::record_first_audio_observation(ms)` durably records the producer's first-admitted audio wall-clock observation in UTC milliseconds (`0..=9_007_199_254_740_991`), not an ADC timestamp. The value is write-once; replaying the same value reconfirms durability, while a different value conflicts. Stop/resource observations remain outside this storage schema. No timestamp is guessed for recovered recordings.

Manifests without `first_admitted_audio_observed_at_ms` remain schema v1, with identical serialized bytes and ordinary-write behavior. Only an explicit first-observation write upgrades that manifest to v2, which requires the field to contain a valid integer. Null, duplicate keys, missing v2 times, v1 times and future schema versions fail closed and retain all files. Timing never changes identity, chunk/chain/seal/source hashes or canonical vault files.

Older readers cannot open a recovery root containing v2 manifests. Do not downgrade while those recordings need recovery or upload; use a timing-aware reader. Opening a v1 root does not migrate it automatically.

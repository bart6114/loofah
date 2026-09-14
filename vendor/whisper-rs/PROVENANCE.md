# Vendored Whisper engine

Rust binding: https://codeberg.org/tazz4843/whisper-rs, revision 129b9826 (0.16.0).
Native engine: https://github.com/ggml-org/whisper.cpp/releases/tag/v1.9.4, commit 927cfce.
Release archive SHA-256: 57e280cee375ab02425b806ad5146b99f6eb9357e3c2b31357c8a6af2e2e44ae.

Upstream source licenses are retained. Non-build examples, assets and CI files are omitted.
Local build adaptations refresh the native build copy from the checked-in tree on every build, require generated bindings
against its headers (no fallback to the old ABI), and read the new version declarations.
The native source is otherwise unmodified.

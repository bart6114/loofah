# Contributing

Contributions are welcome via pull requests. Small, concise PRs focused on a
single change are preferred over large, elaborate PRs. Briefly explain what
changed, why, and how you tested it.

Bug-fix PRs are always welcome. Feature PRs are welcome too, but please open an
issue to discuss your idea with the author before starting work. This helps avoid
spending time on a feature that may not be accepted.

See [README.md](./README.md) for setup, and the
[GitHub repository](https://github.com/bart6114/loofah) for
issues.

## Release build caching

macOS staging and stable builds use `macos-26` with Xcode 26.6, the pinned Rust
compiler, and one CI-only `CARGO_TARGET_DIR` at the repository root. The CLI builds
first so the desktop can reuse compatible dependencies. Bundles are under
`target/release/bundle/`; local development keeps its existing target layout.

The release cache preserves workspace libraries and native Swift/MLX build outputs.
Its identity includes the Rust toolchain, Cargo configuration, Xcode/SDK, native
sources and Swift package locks. Unused preinstalled Rust toolchains are removed
only on ephemeral GitHub-hosted runners. Staging and stable share a cache family;
test/debug jobs remain separate. Older tags without resource-restoration support
use dependency-only caches. Workflow helpers come from the workflow commit even
when application sources come from an older tag.

Keep `macos_release_setup` and `rust_install` as sibling workflow steps. Nested
composite post actions can lose their inputs ([runner issue #2009](https://github.com/actions/runner/issues/2009)),
which silently disables workspace-output retention when rust-cache saves. The
`release-v2` namespace replaces those incomplete caches; existing entries are
[immutable](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching).
The `release-cache-check` workflow verifies the real action's save/restore behavior
on separate runners using representative native files in an isolated cache family.

Configure **Settings → Actions → General → Cache size eviction limit** to 50 GiB
and an **Actions Cache Storage** budget of $5/month in account billing. At the
current $0.07/GiB-month rate above the included 10 GiB, full utilization is
$2.80/month before taxes. These are account settings, not changes made by the
workflow. A valid payment method is required. Extra capacity does not prevent
unused caches expiring under GitHub's retention policy.

`desktop-build` supports artifact-only benchmarks without updating releases or
updater feeds. Dispatch it on the implementation branch with the same `ref` for
both comparisons. Set `legacy-layout: true` for separate target directories and
dependency-only caching, and `false` for the shared layout and workspace caching.
Use a fresh `cache-namespace` for each cold comparison, then repeat the same inputs
on a fresh runner for a warm comparison. Both variants use the pinned toolchains;
the hosted runner image can still change, so record its version and compare several
runs. Set `require-native-cache: true` for the shared-layout warm run to require
both native libraries and Metal outputs before compilation begins.
The default namespace is reserved for normal staging/stable builds.

The `macos-*-timings` artifacts contain per-stage elapsed times, Cargo HTML reports
for the CLI and desktop, native phase timings in `native.tsv` (phase, milliseconds,
Cargo output directory), the selected toolchain, and native-output inventories
before and after compilation. Native timings are emitted
only when the native build script executes. Bundle time includes app signing and
notarization; stable release jobs separately notarize the outer DMG. Compare build
and cache time separately from runner queueing. Validate the bundled CLI, Metal
resources, signatures and local transcription before accepting a performance
change; the target is a 25% reduction in warm build plus cache overhead.

Linux and Intel macOS CLI builds run alongside the desktop build. The desktop
supplies the Apple Silicon CLI archive. CLI feed publication waits for all required
builds and verifies archive checksums. Standalone `cli-release` continues to build
all missing platforms and publish its feed. Release retries reuse complete assets.

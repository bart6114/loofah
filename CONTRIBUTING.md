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

When the two builds need separate Cargo units for Soniqo, the first publishes the
Swift archive and Metal library to the runner's temporary directory. The second
checks their SHA-256 hashes and copies them into its own `OUT_DIR`. Sharing is
limited to the same native build inputs, target, Cargo profile, and Swift
configuration in a single job. Build timing artifacts record the restore and save
steps alongside the existing native compile timings.

The release cache preserves workspace libraries and native Swift/MLX build outputs.
Its identity includes the Rust toolchain, Cargo configuration, Xcode/SDK, hosted runner image revision, native
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
Those marker-only fixtures use a fixed image identity so runner rollouts do not
prevent testing retention. Real builds isolate each image revision: Swift may
invalidate compiled outputs across images even when compiler versions match.
Each new image needs one seed build; exact cache hits are immutable and cannot
save outputs rebuilt for a different image.

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
before and after compilation. `cli/native-logs/` and `desktop/native-logs/`
preserve Soniqo build-script stdout/stderr, including successful Swift compile
output that Cargo normally hides. Native timings are emitted
only when the native build script executes. Bundle time includes app signing and
notarization; stable release jobs separately notarize the outer DMG. Compare build
and cache time separately from runner queueing. Validate the bundled CLI, Metal
resources, signatures and local transcription before accepting a performance
change; the target is a 25% reduction in warm build plus cache overhead.

Windows ARM64 runs Rust and signing verification alongside the installer build
and installed-app smoke test. Both jobs must succeed when native Windows CI is
required. The ARM64 verification job has its own Rust cache, so it can retain
debug outputs alongside the installer job's release outputs. The `bundle-only`
dispatch still runs the installer and Whisper smoke tests on both architectures.

Linux and Intel macOS CLI builds run alongside the desktop build. The desktop
supplies the Apple Silicon CLI archive. CLI feed publication waits for all required
builds and verifies archive checksums. Standalone `cli-release` continues to build
all missing platforms and publish its feed. Release retries reuse complete assets.

## iPhone capture release

The native iPhone app uses the existing `io.loofah.notes` App Store record on team
`L8GUHT98B4`. The Mac receiver uses `io.loofah.stable`. Enable iCloud Documents for
both identifiers in Apple Developer and associate both with the private
`iCloud.io.loofah.capture` container. Create a Developer ID provisioning profile
for the Mac app with the production container and store its base64 representation
in the `MACOS_CAPTURE_PROFILE_BASE64` Actions secret. The stable Mac workflow
embeds the profile and checks the signed entitlements.
[Apple provisioning profile reference](https://developer.apple.com/documentation/technotes/tn3125-inside-code-signing-provisioning-profiles)

In `apps/mobile`, run `xcodegen generate` and open `Loofah.xcodeproj`. For a
signed device test, the team’s iOS provisioning profile must include the same
container. The TestFlight workflow exports the App Store build with the repository’s
App Store Connect credentials and checks the signed entitlements before upload.

Ship the signed Mac receiver first through the desktop release workflow. Verify
the published receiver has production iCloud signing. Use that release tag and an
unused iOS build number to dispatch
`mobile-testflight`. Run the following checks on the release-signed Mac app and
the TestFlight build before submitting the store listing in `apps/mobile/app-store`.

| Scenario | Verify |
| --- | --- |
| Audio and notes, audio only, text only, and an empty draft | Intact captures and original dates, one vault session per finished capture, and no session for an empty draft |
| Offline capture, phone reconnects while Mac sleeps | The phone retains the capture. After opening Loofah on Mac, the vault receives it, then the phone and iCloud inbox clear |
| Quota exhausted, partial download, unavailable vault | The phone retains the capture and the Mac waits for a complete download and writable vault |
| Apple Account changes | A pending capture stays on the phone and is sent only through its originating account |
| Two Macs on one account | Only the designated Mac imports. Explicitly change the receiver and verify pending captures go to the selected vault |
| Mac quits during download, after vault commit, or during cleanup | Restart produces one intact vault session, no duplicate saved transcript or summary, and eventual removal of the capture and receipt |
| Intelligence off, local summary model, hosted summary model | Compare to a Mac audio import with identical settings. Verify both responses to the hosted sharing prompt and that title generation follows the Mac settings |

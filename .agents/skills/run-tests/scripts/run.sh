#!/usr/bin/env bash

set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

readonly rust_excludes=(
  desktop
  email
  mac
  notification-macos
  notification-macos2
  tcc
  apple-note
  notification-linux
  am
  aec
  agc
  whisper
  whisper-local
  whisper-local-model
  vad
  vad-masking
  onnx
  pyannote-local
  bundle
  host
  intercept
  frontmatter
  audio
  audio-device
  transcribe-whisper-local
  device-monitor
  local-llm-core
  local-stt-server
  tauri-plugin-deeplink2
  tauri-plugin-detect
  tauri-plugin-fs-sync
  tauri-plugin-fs2
  tauri-plugin-hooks
  tauri-plugin-icon
  tauri-plugin-local-stt
  tauri-plugin-misc
  tauri-plugin-notification
  tauri-plugin-notify
  tauri-plugin-opener2
  tauri-plugin-permissions
  tauri-plugin-settings
  tauri-plugin-sidecar2
  tauri-plugin-store2
  tauri-plugin-tantivy
  tauri-plugin-template
  tauri-plugin-tracing
  tauri-plugin-updater2
  tauri-plugin-windows
)

run_tests_tmp=""

cleanup() {
  local temp_root="${TMPDIR:-/tmp}/loofah-run-tests."
  if [[ -n "$run_tests_tmp" && "$run_tests_tmp" == "$temp_root"* && -d "$run_tests_tmp" ]]; then
    rm -rf -- "$run_tests_tmp"
  fi
}

trap cleanup EXIT

usage() {
  echo "usage: $0 [all|fmt|lint|frontend|rust|cli|workflows]..." >&2
}

run_step() {
  local label="$1"
  shift
  echo "==> $label"
  "$@"
}

setup_node() {
  run_step "install locked pnpm dependencies" pnpm install --frozen-lockfile
}

run_fmt() {
  run_step "formatting" pnpm fmt:check
}

run_lint() {
  run_step "desktop lint" pnpm exec oxlint --quiet --format=github apps/desktop/src/
}

run_frontend() {
  run_step "UI build" pnpm -F ui build
  run_step "desktop typecheck" pnpm -F desktop typecheck
  run_step "desktop frontend tests" pnpm -F desktop test
  run_step "desktop locale catalogs" pnpm -F desktop i18n:check
}

run_rust() {
  if command -v xcrun >/dev/null 2>&1; then
    export SDKROOT
    SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"
  fi

  run_step "ChatGPT model catalog" node apps/desktop/src-tauri/scripts/prepare-codex-models.mjs
  run_step "desktop Rust check" cargo check -p desktop
  run_step "desktop Rust tests" cargo test -p desktop

  local workspace_args=(cargo test --workspace)
  local package
  for package in "${rust_excludes[@]}"; do
    workspace_args+=(--exclude "$package")
  done
  run_step "workspace Rust tests" "${workspace_args[@]}"
}

run_cli() {
  run_step "agent-access tests" cargo test --locked -p agent-access
  run_step "CLI tests" cargo test --locked -p loof-cli
  run_step "TipTap tests" cargo test --locked -p tiptap
  run_step "CLI clippy" cargo clippy --locked \
    -p agent-access \
    -p loof-cli \
    -p tiptap \
    --all-targets \
    --no-deps \
    -- \
    -D warnings
  run_step "release CLI build" cargo build --locked --release -p loof-cli

  run_step "CLI help" target/release/loof --help
  run_step "CLI version" target/release/loof --version
  run_step "sessions help" target/release/loof sessions --help
  run_step "MCP help" target/release/loof mcp --help

  run_tests_tmp="$(mktemp -d "${TMPDIR:-/tmp}/loofah-run-tests.XXXXXX")"

  local doctor_output
  if doctor_output="$(target/release/loof --json --vault-path "$run_tests_tmp/missing-vault" doctor)"; then
    echo "doctor unexpectedly reported a missing vault as ready" >&2
    return 1
  fi
  grep -q '"ready": false' <<<"$doctor_output"

  local invalid_output
  if invalid_output="$(target/release/loof --json sessions list --limit 0 2>&1)"; then
    echo "invalid arguments unexpectedly succeeded" >&2
    return 1
  fi
  grep -q '"code": "invalid_arguments"' <<<"$invalid_output"

  test -s skills/loofah/SKILL.md
  test "$(sed -n '1p' skills/loofah/SKILL.md)" = "---"
  grep -Eq '^name:[[:space:]]+loofah[[:space:]]*$' skills/loofah/SKILL.md
  grep -Eq '^description:[[:space:]]*(>|\||[^[:space:]])' skills/loofah/SKILL.md
  awk 'NR > 1 && $0 == "---" { found = 1; exit } END { if (!found) exit 1 }' skills/loofah/SKILL.md
  test "$(readlink docs/.mintlify/skills)" = "../../skills"
  test -f docs/.mintlify/skills/loofah/SKILL.md

  local skill_list
  skill_list="$(npx --yes skills@1.5.16 add . --list)"
  echo "$skill_list"
  grep -q 'loofah' <<<"$skill_list"
  if grep -q 'add-plugin' <<<"$skill_list"; then
    echo "repository-only skills leaked into the published skill list" >&2
    return 1
  fi

  run_step "docs dependency install" pnpm install --frozen-lockfile --filter @hypr/docs
  run_step "docs build" pnpm --filter @hypr/docs build
}

run_workflows() {
  command -v uvx >/dev/null 2>&1 || {
    echo "uvx is required to run the zizmor workflow check" >&2
    return 1
  }
  run_step "GitHub Actions security" uvx zizmor --min-severity high .github/workflows/
}

if (($# == 0)); then
  set -- all
fi

declare -a groups
if [[ " $* " == *" all "* ]]; then
  groups=(fmt lint frontend rust cli workflows)
else
  groups=("$@")
fi

needs_node=false
for group in "${groups[@]}"; do
  case "$group" in
    fmt | lint | frontend | cli) needs_node=true ;;
    rust | workflows) ;;
    *)
      usage
      exit 2
      ;;
  esac
done

if [[ "$needs_node" == true ]]; then
  setup_node
fi

for group in "${groups[@]}"; do
  "run_$group"
done

#!/usr/bin/env bash
set -euo pipefail

readonly codex_setup_repo="bart6114/loofah"
readonly codex_setup_env="codex-subscription"
codex_setup_home="$(mktemp -d "${TMPDIR:-/tmp}/loofah-codex-setup.XXXXXX")"
trap 'rm -rf -- "$codex_setup_home"' EXIT

if [[ "$(gh api user --jq .login)" != "bart6114" ]]; then
  echo "Sign gh in as bart6114 before configuring this automation." >&2
  exit 1
fi

case "${1:-setup}" in
  enable)
    pilot="$(gh run list --repo "$codex_setup_repo" --workflow codex-issues --branch main \
      --event workflow_dispatch --limit 100 \
      --json databaseId,displayTitle,conclusion --jq '[.[] | select(.displayTitle == "codex-issues: pilot")][0] | select(.conclusion == "success") | .databaseId // empty')"
    if [[ -z "$pilot" ]]; then
      echo "A successful authentication pilot on main is required before activation." >&2
      exit 1
    fi
    existing_since="$(gh variable list --repo "$codex_setup_repo" --json name,value --jq '.[] | select(.name == "CODEX_ISSUES_SINCE") | .value')"
    if [[ -z "$existing_since" ]]; then
      gh variable set CODEX_ISSUES_SINCE --repo "$codex_setup_repo" --body "$(date -u +%FT%TZ)"
    fi
    gh variable set CODEX_ISSUES_ENABLED --repo "$codex_setup_repo" --body true
    echo "Enabled for new bart6114 issues. Older issues can be selected with manual dispatch."
    exit 0
    ;;
  disable)
    gh variable set CODEX_ISSUES_ENABLED --repo "$codex_setup_repo" --body false
    exit 0
    ;;
  setup) ;;
  *) echo "Usage: $0 [setup|enable|disable]" >&2; exit 1 ;;
esac

gh variable set CODEX_ISSUES_ENABLED --repo "$codex_setup_repo" --body false
gh api --method PUT "repos/$codex_setup_repo/environments/$codex_setup_env" --input - >/dev/null <<'JSON'
{"deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}}
JSON
if ! gh api "repos/$codex_setup_repo/environments/$codex_setup_env/deployment-branch-policies" \
  --jq '.branch_policies[] | select(.name == "main" and .type == "branch") | .name' | grep -qx main; then
  gh api --method POST "repos/$codex_setup_repo/environments/$codex_setup_env/deployment-branch-policies" \
    -f name=main -f type=branch >/dev/null
fi

existing_secrets="$(gh secret list --repo "$codex_setup_repo" --env "$codex_setup_env" --json name --jq '.[].name')"
for token in CODEX_ENV_TOKEN CODEX_PUBLISH_TOKEN; do
  if grep -qx "$token" <<< "$existing_secrets"; then
    echo "Reusing $token from the environment."
    continue
  fi
  echo "Create a fine-grained GitHub token restricted to bart6114/loofah."
  if [[ "$token" == "CODEX_ENV_TOKEN" ]]; then
    echo "$token: Environments read/write."
  else
    echo "$token: Contents, Pull requests, and Workflows read/write."
  fi
  echo "Enter it at the hidden gh prompt; never paste it into an issue or chat."
  gh secret set "$token" --repo "$codex_setup_repo" --env "$codex_setup_env"
done

echo "Starting a separate subscription login for CI; your existing Codex login is untouched."
env CODEX_HOME="$codex_setup_home" codex -c 'cli_auth_credentials_store="file"' login --device-auth
node --input-type=module - "$codex_setup_home/auth.json" <<'JS'
import { readFileSync } from 'node:fs';
const auth = JSON.parse(readFileSync(process.argv[2], 'utf8'));
if (auth.auth_mode !== 'chatgpt' || auth.OPENAI_API_KEY || !auth.tokens?.refresh_token) {
  throw new Error('Expected a ChatGPT subscription login with refresh credentials');
}
JS
gh secret set CODEX_AUTH_JSON --repo "$codex_setup_repo" --env "$codex_setup_env" < "$codex_setup_home/auth.json"
gh workflow run codex-issues --repo "$codex_setup_repo" --ref main -f mode=pilot
echo "Authentication pilot dispatched. After it succeeds, run this script with enable."

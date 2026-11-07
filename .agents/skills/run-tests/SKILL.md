---
name: run-tests
description: Run relevant local checks or the repository's full pull-request CI suite. Use before pushing changes, when diagnosing a GitHub Actions failure, or when explicitly asked to run tests or verify a branch.
---

# Run Tests

By default, run only the groups relevant to the changed files from the repository root:

```bash
bash .agents/skills/run-tests/scripts/run.sh fmt frontend
```

Available groups are `fmt`, `lint`, `frontend`, `rust`, `cli`, and `workflows`; pass one or more. The `frontend` group includes shared editor typechecking/tests as well as the desktop frontend and locale checks. Choose them from the scope of the change rather than running every group automatically. Always include `fmt` after edits, then add the affected area. For cross-cutting Cargo or workspace changes, include every affected Rust group.

Commit, push, and open the pull request after the relevant local checks pass. The full GitHub Actions result is the merge gate; do not duplicate it locally merely because a branch is ready to push or a pull request is being opened.

Use `all` when the user explicitly requests the full local suite, when reproducing broad CI failures, or when changes span enough of the repository that targeted selection would provide little benefit:

```bash
bash .agents/skills/run-tests/scripts/run.sh all
```

`all` mirrors the validation jobs from `ci.yaml`, `cli_ci.yaml`, `desktop_ci.yaml`, and `zizmor.yaml`: formatting, desktop lint/frontend/Rust checks, CLI checks and smoke tests, docs build, and workflow security analysis. It intentionally excludes deployment, release, signing, and desktop E2E workflows.

The Rust CI job is macOS-specific. On a fresh worktree, run `scripts/setup-shared-target.sh` first. The script installs locked pnpm dependencies and fails fast on the first failing CI group. If a CI workflow or its local composite actions changed, compare the script with the changed workflow before trusting parity and update both together when needed.

Do not fix unrelated failures automatically. Report the failing group and decisive output; make changes only when the user requested implementation or the failure is caused by the current task.

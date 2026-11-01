---
name: desktop-release
description: Use when cutting/shipping a stable desktop release, building the signed DMG, running the desktop-release workflow, or writing/backfilling release notes ("What's new" changelog) for a version.
---

# Desktop Release

Releases are two-stage: every push to `main` auto-creates a tagged GitHub release
(`release.yaml` — version bump from conventional commits, `--generate-notes`, no binary).
The signed + notarized DMG and updater artifacts are built on demand by the
`desktop-release` workflow. This skill runs that second stage; prepare release
notes manually before triggering the workflow.

## 1. Preflight

```bash
git status --short                 # must be clean
git pull --rebase origin main      # bot bump commits mean local main is always behind
gh auth status
```

Never tag or bump versions by hand — the tag for pushed work already exists.

**All CI on the release commit must be green before triggering the release build.**
Check every workflow run for the commit the target tag points at (bot bump commits
carry `[skip ci]`, so the runs usually live on its parent — walk back to the first
commit that has runs):

```bash
gh run list --commit $(git rev-parse <tag>^{commit}) \
  --json name,status,conclusion \
  --jq '.[] | "\(.name): \(.status) \(.conclusion // "")"'
```

Every run must show `completed success`. If any run failed, is still in progress,
or the commit has no runs (check the parent), stop and resolve that first — do not
`gh workflow run desktop-release` on a commit with red or pending CI.

## 2. Resolve the target tag

User-specified, or the latest release:

```bash
gh release view --json tagName --jq .tagName
```

### Major-version confirmation gate

Before triggering `desktop-release`, compare the target version with the version
currently published in the updater feed:

```bash
target_version="${tag#v}"
shipped_version="$(gh release download updater --pattern latest.json -O - | jq -r .version)"
printf 'shipped=%s target=%s\n' "$shipped_version" "$target_version"
```

If the target's major component is greater than the shipped version's major
component, stop and manually inspect the commits that selected the major bump.
Tell the user the exact version transition and triggering commit(s), then ask for
explicit confirmation that they intend to ship that major version. A general
request to run a release is not sufficient confirmation of an unexpected major
bump. Do not trigger the workflow until the user explicitly acknowledges the
major increment.

## 3. Prepare release notes manually

Write or fix `packages/changelog/content/<version>.md` (no leading `v`) before
triggering the release build. Read `packages/changelog/content/AGENTS.md` and the
two or three highest-numbered entries for authoring rules and style. Include
quoted `date` and `summary` frontmatter and a non-empty Markdown body.

Notes must cover **everything since the last shipped version** (the updater-feed
version), not since the previous git tag — many tags never ship. Review
`git log <last-shipped-tag>..<tag>` and inspect individual commits as needed.
For backfills, re-ships, or a missing feed, use the previous `v*` tag when the
updater-feed version is not older than the target or its tag is unavailable.
If there is no previous tag, review the target's full history.

Commit with `docs: changelog for vX.Y.Z [skip ci]` — the `[skip ci]` is
**required** — and push the file to `main`. The `changelog` job validates the
committed file, syncs the GitHub release body, and dispatches the docs deployment.
Missing or malformed notes fail the job and block the release build.

To publish or backfill existing notes for a tag without building:
`gh workflow run changelog -f tag=<tag>`. To fix notes, edit the content file,
commit with `[skip ci]`, push to `main`, and rerun that workflow.

The in-app "What's new in X?" dialog fetches the file at runtime from
`https://raw.githubusercontent.com/bart6114/loofah/main/packages/changelog/content/<version>.md`
— i.e. from `main`, so post-hoc fixes to the file reach users without a rebuild.

## 4. Build the DMG

```bash
gh workflow run desktop-release -f tag=<tag>
sleep 10 && gh run list --workflow=desktop-release --limit 1
gh run watch <run-id>              # full macOS build + notarization, ~15-30 min
```

## 5. Verify

All 5 assets on the release, and the updater feed advanced:

```bash
gh release view <tag> --json assets --jq '.assets[].name'
# expect: .dmg, .dmg.sha256, .app.tar.gz, .app.tar.gz.sig, latest.json
gh release download updater --pattern latest.json -O - | jq -r .version
curl -fsS https://raw.githubusercontent.com/bart6114/loofah/main/packages/changelog/content/<version>.md >/dev/null && echo notes-ok
```

## Gotchas

- **Never delete the `updater` prerelease** — it's the endpoint shipped apps poll.
  The feed is version-guarded and only moves forward (re-running an old tag won't
  downgrade it).
- The bundled changelog fast path only covers the highest-numbered content file at
  the tag; every other version depends on the runtime fetch from `main` — so notes
  must be on `main` before users first open the "What's new" dialog (it fires on
  first launch after an update). The changelog job's ship-gate guarantees this.
- Manual changelog commits need `[skip ci]` or they spawn a fresh release.
- The changelog job overwrites the GitHub release body from the content file on
  every run — edit the content file, not the release body.

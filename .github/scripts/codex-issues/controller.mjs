import { execFileSync } from "node:child_process";
import {
  appendFileSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";

import {
  bot,
  decide,
  digest,
  fingerprint,
  issueContext,
  owner,
  readState,
  repository,
  stateMarker,
  validResult,
  validatePaths,
} from "./core.mjs";
import { GitHub } from "./github.mjs";

const api = new GitHub();
const run = Number(process.env.GITHUB_RUN_ID);
const work =
  process.env.CODEX_JOB_DIR ?? join(process.env.RUNNER_TEMP, "codex-issue");
const runUrl = `https://github.com/${repository}/actions/runs/${run}`;
const event = JSON.parse(readFileSync(process.env.GITHUB_EVENT_PATH, "utf8"));
const eventName = process.env.GITHUB_EVENT_NAME;

function output(key, value) {
  if (/[\r\n]/.test(String(value))) throw new Error("Invalid workflow output");
  appendFileSync(process.env.GITHUB_OUTPUT, `${key}=${value}\n`);
}

function git(...args) {
  return execFileSync("git", args, {
    encoding: "utf8",
    maxBuffer: 25 * 1024 * 1024,
  }).trim();
}

async function saveState(number, prior, fields, message) {
  const state = { ...prior, ...fields, version: 1, run };
  delete state.commentId;
  const body = `${stateMarker}${JSON.stringify(state)} -->\n${message}\n\n[Workflow run](${runUrl})`;
  if (prior?.commentId) {
    return api.request(`issues/comments/${prior.commentId}`, "PATCH", { body });
  }
  return api.comment(number, body);
}

async function select() {
  output("mode", "none");
  if (
    process.env.GITHUB_REPOSITORY !== repository ||
    process.env.GITHUB_REF !== "refs/heads/main"
  ) {
    return;
  }
  if (eventName !== "schedule" && process.env.GITHUB_ACTOR !== owner) return;
  if (eventName === "workflow_dispatch" && event.inputs?.mode === "pilot") {
    mkdirSync(work, { recursive: true });
    writeFileSync(
      join(work, "request.json"),
      JSON.stringify({ mode: "pilot", run }),
    );
    output("mode", "pilot");
    return;
  }
  if (process.env.CODEX_ISSUES_ENABLED !== "true") return;
  if (event.issue?.pull_request) return;
  const requested = event.issue?.number ?? Number(event.inputs?.issue_number);
  const retry =
    (eventName === "workflow_dispatch" && event.inputs?.mode === "retry") ||
    (eventName === "issue_comment" &&
      event.comment?.body.trim() === "/codex retry");
  if (
    eventName === "workflow_dispatch" &&
    (!Number.isSafeInteger(requested) || requested <= 0)
  ) {
    throw new Error("A positive issue_number is required");
  }
  const candidates = requested
    ? [{ number: requested }]
    : await api.list(
        `issues?state=open&creator=${owner}&sort=created&direction=asc`,
      );
  for (const candidate of candidates) {
    const { issue, comments } = await api.thread(candidate.number);
    const prior = readState(comments);
    // Polling also recovers missed events, but never opts the old backlog in.
    if (
      !requested &&
      !prior &&
      (!process.env.CODEX_ISSUES_SINCE ||
        issue.created_at < process.env.CODEX_ISSUES_SINCE)
    ) {
      continue;
    }
    if (prior?.status === "running" && !retry) {
      const previous = await api.request(`actions/runs/${prior.run}`);
      if (previous.status !== "completed") continue;
      await saveState(
        issue.number,
        prior,
        { status: "failed" },
        "The previous run stopped before completion. Comment `/codex retry` to retry.",
      );
      continue;
    }
    const reactions =
      prior?.planId && comments.some((comment) => comment.id === prior.planId)
        ? await api.list(`issues/comments/${prior.planId}/reactions`)
        : [];
    const next = decide({ issue, comments, reactions, retry });
    if (!next) continue;
    const branch = `chore/codex-issue-${issue.number}`;
    const prs = await api.list(`pulls?state=all&head=${owner}:${branch}`);
    if (prs.length) {
      await saveState(
        issue.number,
        prior,
        { status: "done", pr: prs[0].html_url },
        `Implementation: ${prs[0].html_url}`,
      );
      continue;
    }
    const base = await api.request("git/ref/heads/main");
    const request = {
      mode: next.mode,
      number: issue.number,
      context: {
        ...issueContext(issue, comments),
        previousResponses: comments
          .filter(
            (comment) =>
              comment.user.login === bot &&
              /^(## Questions|## Implementation plan)\n/.test(comment.body),
          )
          .map(({ id, body }) => ({ id, body })),
      },
      hash: next.hash,
      plan: next.plan ?? "",
      planId: next.mode === "implement" ? prior.planId : null,
      planHash: next.mode === "implement" ? prior.planHash : null,
      planUrl: next.planUrl ?? "",
      base: base.object.sha,
      branch,
      run,
    };
    const claim = await saveState(
      issue.number,
      prior,
      {
        status: "running",
        context: next.hash,
        planId: request.planId,
        planHash: request.planHash,
      },
      next.mode === "plan"
        ? "Reading the issue and repository…"
        : "Your approved plan is being implemented…",
    );
    request.stateId = claim.id;
    mkdirSync(work, { recursive: true });
    writeFileSync(join(work, "request.json"), JSON.stringify(request));
    output("mode", next.mode);
    return;
  }
}

async function current(request, requireReaction = false) {
  const { issue, comments } = await api.thread(request.number);
  const state = readState(comments);
  if (
    state?.run !== request.run ||
    state.commentId !== request.stateId ||
    state.status !== "running"
  )
    return null;
  if (
    issue.user.login !== owner ||
    issue.state !== "open" ||
    issue.locked ||
    fingerprint(issue, comments) !== request.hash
  ) {
    await saveState(
      request.number,
      state,
      { status: "stale", planId: null, planHash: null },
      "The issue changed or closed during the run. This result was not published; an open issue will be replanned.",
    );
    return null;
  }
  if (request.planId) {
    const plan = comments.find(
      (comment) => comment.id === request.planId && comment.user.login === bot,
    );
    const reactions =
      plan && requireReaction
        ? await api.list(`issues/comments/${request.planId}/reactions`)
        : [];
    if (
      !plan ||
      digest(plan.body) !== request.planHash ||
      (requireReaction &&
        !reactions.some(
          (reaction) =>
            reaction.user.login === owner && reaction.content === "+1",
        ))
    ) {
      await saveState(
        request.number,
        state,
        { status: "stale", planId: null, planHash: null },
        "The plan or its approval changed. A new plan is needed.",
      );
      return null;
    }
  }
  return state;
}

async function check() {
  const request = JSON.parse(readFileSync(join(work, "request.json"), "utf8"));
  output(
    "valid",
    request.mode === "pilot" || Boolean(await current(request, true)),
  );
}

async function publish() {
  const request = JSON.parse(readFileSync(join(work, "request.json"), "utf8"));
  if (request.mode === "pilot") return;
  const state = await current(request, true);
  if (!state) return;
  const result = validResult(
    JSON.parse(readFileSync(join(work, "result.json"), "utf8")),
    request.mode,
  );
  if (request.mode === "plan") {
    const footer =
      result.kind === "plan"
        ? "\n\nReact 👍 to **this comment** to approve implementation. Only bart6114’s approval counts. Detection is scheduled every five minutes and can be delayed. Reply to request changes."
        : "\n\nReply here with the missing details and I’ll update the plan.";
    const comment = await api.comment(
      request.number,
      `${result.kind === "plan" ? "## Implementation plan" : "## Questions"}\n\n${result.body}${footer}`,
    );
    await saveState(
      request.number,
      state,
      {
        status: result.kind,
        planId: result.kind === "plan" ? comment.id : null,
        planHash: result.kind === "plan" ? digest(comment.body) : null,
      },
      result.kind === "plan"
        ? `Waiting for your 👍 on ${comment.html_url}`
        : `Waiting for your answers: ${comment.html_url}`,
    );
    return;
  }
  if (result.kind !== "implemented") {
    await api.comment(request.number, `${result.body}\n\n${result.validation}`);
    await saveState(
      request.number,
      state,
      { status: "failed" },
      "No PR was created. Reply with clarification or comment `/codex retry`.",
    );
    return;
  }
  const patch = join(work, "changes.patch");
  git("fetch", "origin", request.base);
  git("checkout", "--detach", request.base);
  git("apply", "--index", "--binary", patch);
  const paths = git("diff", "--cached", "--name-only", "-z")
    .split("\0")
    .filter(Boolean);
  validatePaths(paths);
  if (!paths.length) throw new Error("Implementation produced an empty patch");
  git(
    "-c",
    "user.name=github-actions[bot]",
    "-c",
    "user.email=41898282+github-actions[bot]@users.noreply.github.com",
    "-c",
    "core.hooksPath=/dev/null",
    "commit",
    "-m",
    `chore: implement issue #${request.number}`,
  );
  const publisher = new GitHub(process.env.CODEX_PUBLISH_TOKEN);
  const existing = await publisher.list(
    `pulls?state=all&head=${owner}:${request.branch}`,
  );
  if (existing.length) {
    await saveState(
      request.number,
      state,
      { status: "done", pr: existing[0].html_url },
      `Implementation: ${existing[0].html_url}`,
    );
    return;
  }
  const remote = git("ls-remote", "--heads", "origin", request.branch);
  if (remote) {
    git("fetch", "origin", request.branch);
    if (
      git("rev-parse", "HEAD^{tree}") !== git("rev-parse", "FETCH_HEAD^{tree}")
    ) {
      throw new Error(
        "Existing implementation branch differs; refusing to overwrite it",
      );
    }
  } else {
    const askpass = join(process.env.RUNNER_TEMP, "codex-git-askpass.sh");
    writeFileSync(
      askpass,
      '#!/bin/sh\ncase "$1" in *Username*) echo x-access-token ;; *) printf "%s\\n" "$CODEX_PUBLISH_TOKEN" ;; esac\n',
      { mode: 0o700 },
    );
    execFileSync(
      "git",
      [
        "-c",
        "core.hooksPath=/dev/null",
        "push",
        "origin",
        `HEAD:refs/heads/${request.branch}`,
      ],
      {
        env: { ...process.env, GIT_ASKPASS: askpass, GIT_TERMINAL_PROMPT: "0" },
        stdio: "pipe",
      },
    );
  }
  const pr = await publisher.request("pulls", "POST", {
    base: "main",
    head: request.branch,
    title: result.title || `Implement issue #${request.number}`,
    body: `Closes #${request.number}\n\nApproved plan: ${request.planUrl}\n\n${result.body}\n\n### Validation\n\n${result.validation}\n\n[Automation run](${runUrl})`,
    draft: !result.checksPassed,
  });
  await api.comment(request.number, `Implementation PR: ${pr.html_url}`);
  await saveState(
    request.number,
    state,
    { status: "done", pr: pr.html_url },
    `Implementation: ${pr.html_url}`,
  );
}

async function failed() {
  const request = JSON.parse(readFileSync(join(work, "request.json"), "utf8"));
  if (request.mode === "pilot") return;
  const { comments } = await api.thread(request.number);
  const state = readState(comments);
  if (state?.run !== request.run || state.status !== "running") return;
  await saveState(
    request.number,
    state,
    { status: "failed" },
    "The run failed. Check the linked workflow for the failing step, repair credentials if needed, then comment `/codex retry`. Automatic retries are disabled.",
  );
}

try {
  const commands = { select, check, publish, failed };
  if (!commands[process.argv[2]]) throw new Error("Unknown controller command");
  await commands[process.argv[2]]();
} catch (error) {
  // Child-process errors may include credential-bearing output.
  console.error(
    error.status === undefined
      ? error.message
      : "Git command failed; no subprocess output was published.",
  );
  process.exitCode = 1;
}

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
  fingerprint,
  issueContext,
  owner,
  readState,
  repository,
  stateMarker,
  validResult,
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

async function saveState(number, prior, fields, message) {
  const state = {
    context: prior?.context,
    status: prior?.status,
    ...fields,
    version: 1,
    run,
  };
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
    const next = decide({ issue, comments, retry });
    if (!next) continue;
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
              /^(## Questions|## Issue description)\n/.test(comment.body),
          )
          .map(({ id, body }) => ({ id, body })),
      },
      hash: next.hash,
      base: base.object.sha,
      run,
    };
    const claim = await saveState(
      issue.number,
      prior,
      {
        status: "running",
        context: next.hash,
      },
      "Checking the issue description against the repository…",
    );
    request.stateId = claim.id;
    mkdirSync(work, { recursive: true });
    writeFileSync(join(work, "request.json"), JSON.stringify(request));
    output("mode", next.mode);
    return;
  }
}

async function current(request) {
  if (request.mode !== "review") throw new Error("Unsupported issue mode");
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
      { status: "stale" },
      "The issue changed or closed during the run. This result was not published; an open issue will be reviewed again.",
    );
    return null;
  }
  return state;
}

async function check() {
  const request = JSON.parse(readFileSync(join(work, "request.json"), "utf8"));
  output("valid", request.mode === "pilot" || Boolean(await current(request)));
}

async function publish() {
  const request = JSON.parse(readFileSync(join(work, "request.json"), "utf8"));
  if (request.mode === "pilot") return;
  const state = await current(request);
  if (!state) return;
  const result = validResult(
    JSON.parse(readFileSync(join(work, "result.json"), "utf8")),
  );
  const comment = await api.comment(
    request.number,
    result.kind === "questions"
      ? `## Questions\n\n${result.body}\n\nReply here with the missing details and I’ll review the issue again.`
      : `## Issue description\n\n${result.body}`,
  );
  await saveState(
    request.number,
    state,
    { status: result.kind },
    result.kind === "questions"
      ? `Waiting for your answers: ${comment.html_url}`
      : `The issue is sufficiently described: ${comment.html_url}`,
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
  console.error(error.message);
  process.exitCode = 1;
}

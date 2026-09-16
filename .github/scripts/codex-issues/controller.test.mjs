import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";

import { bot, fingerprint, owner, repository, stateMarker } from "./core.mjs";

const controller = resolve(".github/scripts/codex-issues/controller.mjs");
const issue = {
  number: 42,
  title: "Sorting",
  body: "Sort notes by date",
  state: "open",
  created_at: "2026-09-16T00:00:00Z",
  user: { login: owner },
};
function status(fields = {}) {
  return {
    id: 11,
    user: { login: bot },
    body: `${stateMarker}${JSON.stringify({ version: 1, run: 100, status: "running", context: fingerprint(issue, []), ...fields })} -->`,
  };
}

function run(
  command,
  {
    event = { issue },
    eventName = "issues",
    actor = owner,
    routes = {},
    request,
    result,
    env = {},
  } = {},
) {
  const directory = mkdtempSync(join(tmpdir(), "codex-controller-test-"));
  try {
    writeFileSync(join(directory, "event.json"), JSON.stringify(event));
    writeFileSync(join(directory, "routes.json"), JSON.stringify(routes));
    writeFileSync(join(directory, "calls.jsonl"), "");
    writeFileSync(join(directory, "outputs"), "");
    if (request)
      writeFileSync(join(directory, "request.json"), JSON.stringify(request));
    if (result)
      writeFileSync(join(directory, "result.json"), JSON.stringify(result));
    writeFileSync(
      join(directory, "mock.mjs"),
      `
      import { readFileSync, appendFileSync } from 'node:fs';
      const routes = JSON.parse(readFileSync(process.env.TEST_DIR + '/routes.json', 'utf8'));
      globalThis.fetch = async (url, options) => {
        const path = new URL(url).pathname.replace('/repos/${repository}/', '');
        const method = options.method;
        const body = options.body ? JSON.parse(options.body) : null;
        appendFileSync(process.env.TEST_DIR + '/calls.jsonl', JSON.stringify({path, method, body}) + '\\n');
        const data = routes[method + ' ' + path];
        if (data === undefined) throw new Error('Unexpected API call: ' + method + ' ' + path);
        return {ok:true, status:200, json: async () => data === '$comment' ? {id:99, body:body.body, user:{login:'${bot}'}, html_url:'https://example.test/comment/99'} : data};
      };
    `,
    );
    let code = 0;
    try {
      execFileSync(
        process.execPath,
        ["--import", join(directory, "mock.mjs"), controller, command],
        {
          env: {
            PATH: process.env.PATH,
            GH_TOKEN: "test-token",
            TEST_DIR: directory,
            RUNNER_TEMP: directory,
            CODEX_JOB_DIR: directory,
            GITHUB_RUN_ID: "100",
            GITHUB_REPOSITORY: repository,
            GITHUB_REF: "refs/heads/main",
            GITHUB_ACTOR: actor,
            GITHUB_EVENT_NAME: eventName,
            GITHUB_EVENT_PATH: join(directory, "event.json"),
            GITHUB_OUTPUT: join(directory, "outputs"),
            CODEX_ISSUES_ENABLED: "true",
            ...env,
          },
          stdio: "pipe",
        },
      );
    } catch (error) {
      code = error.status;
    }
    const calls = readFileSync(join(directory, "calls.jsonl"), "utf8")
      .trim()
      .split("\n")
      .filter(Boolean)
      .map(JSON.parse);
    return {
      code,
      calls,
      outputs: readFileSync(join(directory, "outputs"), "utf8"),
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test("untrusted events and disabled automation never fetch issue data or credentials", () => {
  for (const input of [
    { actor: "outsider" },
    { env: { CODEX_ISSUES_ENABLED: "false" } },
    { env: { GITHUB_REF: "refs/heads/other" } },
  ]) {
    const result = run("select", input);
    assert.equal(result.code, 0);
    assert.deepEqual(result.calls, []);
    assert.equal(result.outputs, "mode=none\n");
  }
});

test("pilot works while issue automation is disabled", () => {
  const result = run("select", {
    eventName: "workflow_dispatch",
    event: { inputs: { mode: "pilot" } },
    env: { CODEX_ISSUES_ENABLED: "false" },
  });
  assert.equal(result.code, 0);
  assert.match(result.outputs, /mode=pilot/);
  assert.deepEqual(result.calls, []);
});

test("a new issue claims work exactly once before agent execution", () => {
  const result = run("select", {
    routes: {
      "GET issues/42": issue,
      "GET issues/42/comments": [],
      "GET git/ref/heads/main": { object: { sha: "a".repeat(40) } },
      "POST issues/42/comments": "$comment",
    },
  });
  assert.equal(result.code, 0);
  assert.match(result.outputs, /mode=review/);
  assert.equal(result.calls.filter((call) => call.method === "POST").length, 1);
  assert.match(result.calls.at(-1).body.body, /"status":"running"/);
});

test("an interrupted prior run fails visibly instead of duplicating work", () => {
  const result = run("select", {
    routes: {
      "GET issues/42": issue,
      "GET issues/42/comments": [status({ run: 99 })],
      "GET actions/runs/99": { status: "completed" },
      "PATCH issues/comments/11": {},
    },
  });
  assert.equal(result.code, 0);
  assert.equal(result.outputs, "mode=none\n");
  assert.match(result.calls.at(-1).body.body, /"status":"failed"/);
});

test("legacy implementation requests are rejected before any API calls", () => {
  for (const command of ["check", "publish"]) {
    const result = run(command, { request: { mode: "implement", number: 42 } });
    assert.equal(result.code, 1);
    assert.deepEqual(result.calls, []);
  }
});

test("a changed issue prevents publication of an old result", () => {
  const result = run("publish", {
    request: {
      number: 42,
      run: 100,
      stateId: 11,
      mode: "review",
      hash: fingerprint(issue, []),
    },
    routes: {
      "GET issues/42": { ...issue, body: "Changed requirements" },
      "GET issues/42/comments": [status()],
      "PATCH issues/comments/11": {},
    },
  });
  assert.equal(result.code, 0);
  assert.equal(result.calls.filter((call) => call.method === "POST").length, 0);
  assert.match(result.calls.at(-1).body.body, /"status":"stale"/);
});

for (const kind of ["questions", "ready"]) {
  test(`a ${kind} result publishes only an issue comment and its review status`, () => {
    const result = run("publish", {
      request: {
        number: 42,
        run: 100,
        stateId: 11,
        mode: "review",
        hash: fingerprint(issue, []),
      },
      result: {
        kind,
        body:
          kind === "questions"
            ? "Which date should determine the order?"
            : "Sort notes newest first using the note date.",
      },
      routes: {
        "GET issues/42": issue,
        "GET issues/42/comments": [status()],
        "POST issues/42/comments": "$comment",
        "PATCH issues/comments/11": {},
      },
    });
    assert.equal(result.code, 0);
    const comment = result.calls.find((call) => call.method === "POST");
    assert.match(
      comment.body.body,
      kind === "questions" ? /## Questions/ : /## Issue description/,
    );
    assert.doesNotMatch(
      comment.body.body,
      /approve|React|plan|implementation/i,
    );
    assert.equal(result.calls.length, 4);
    assert.ok(result.calls.at(-1).body.body.includes(`"status":"${kind}"`));
  });
}

test("a failed review records a retryable failure", () => {
  const result = run("failed", {
    request: { number: 42, run: 100, mode: "review" },
    routes: {
      "GET issues/42": issue,
      "GET issues/42/comments": [status()],
      "PATCH issues/comments/11": {},
    },
  });
  assert.equal(result.code, 0);
  assert.match(result.calls.at(-1).body.body, /"status":"failed"/);
});

test("an existing plan is replaced by review without fetching reactions or PRs", () => {
  const result = run("select", {
    routes: {
      "GET issues/42": issue,
      "GET issues/42/comments": [
        status({ status: "plan", planId: 10, planHash: "old" }),
      ],
      "GET git/ref/heads/main": { object: { sha: "a".repeat(40) } },
      "PATCH issues/comments/11": { id: 11 },
    },
  });
  assert.equal(result.code, 0);
  assert.match(result.outputs, /mode=review/);
  assert.doesNotMatch(result.calls.at(-1).body.body, /planId|planHash/);
});

test("rerunning a completed publication does not create a second description", () => {
  const result = run("publish", {
    request: {
      number: 42,
      run: 100,
      stateId: 11,
      mode: "review",
      hash: fingerprint(issue, []),
    },
    routes: {
      "GET issues/42": issue,
      "GET issues/42/comments": [status({ status: "ready" })],
    },
  });
  assert.equal(result.code, 0);
  assert.equal(result.calls.length, 2);
});

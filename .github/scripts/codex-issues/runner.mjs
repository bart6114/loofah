import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

import {
  assertNoSecrets,
  cliVersion,
  environment,
  repository,
  resultSchema,
  secretValues,
  validResult,
  validatePaths,
} from "./core.mjs";

const work =
  process.env.CODEX_JOB_DIR ?? join(process.env.RUNNER_TEMP, "codex-issue");
const request = JSON.parse(readFileSync(join(work, "request.json"), "utf8"));
const pilot = request.mode === "pilot";
const verify = process.argv.includes("--verify");
const user = "loofah-codex";
const home = `/Users/${user}`;
const authFile = `${home}/.codex/auth.json`;
const checkout = `${home}/repo`;
const trusted = resolve(".github/codex");
let auth;
let authRestored = false;
let persisted = false;

function command(binary, args, options = {}) {
  return execFileSync(binary, args, {
    encoding: "utf8",
    stdio: "pipe",
    maxBuffer: 25 * 1024 * 1024,
    timeout: 10 * 60 * 1000,
    ...options,
  });
}

function sudo(...args) {
  return command("/usr/bin/sudo", ["-n", ...args]);
}

function userArgs(binary, args) {
  return [
    "-n",
    "-u",
    user,
    "--",
    "/usr/bin/env",
    "-i",
    `PATH=${process.env.PATH}`,
    `HOME=${home}`,
    `USER=${user}`,
    `LOGNAME=${user}`,
    `TMPDIR=${home}/tmp/`,
    `CODEX_HOME=${home}/.codex`,
    "CI=true",
    "GITHUB_ACTIONS=true",
    "GIT_TERMINAL_PROMPT=0",
    binary,
    ...args,
  ];
}

function asUser(binary, ...args) {
  return command("/usr/bin/sudo", userArgs(binary, args));
}

function validateAuth(value) {
  if (
    value.auth_mode !== "chatgpt" ||
    value.OPENAI_API_KEY ||
    !value.tokens?.refresh_token ||
    !value.tokens?.access_token ||
    !value.tokens?.account_id
  ) {
    throw new Error(
      "Expected a dedicated ChatGPT login with refresh credentials, not an API key",
    );
  }
  return value;
}

function mask(value) {
  for (const secret of secretValues(value)) {
    console.log(
      `::add-mask::${secret.replaceAll("%", "%25").replaceAll("\r", "%0D").replaceAll("\n", "%0A")}`,
    );
  }
}

function saveAuth(value) {
  // gh encrypts stdin with the environment's public key; no credential is an argument.
  command(
    "gh",
    [
      "secret",
      "set",
      "CODEX_AUTH_JSON",
      "--repo",
      repository,
      "--env",
      environment,
    ],
    {
      input: JSON.stringify(value),
      env: {
        PATH: process.env.PATH,
        HOME: process.env.HOME,
        GH_TOKEN: process.env.CODEX_ENV_TOKEN,
      },
    },
  );
}

async function executeCodex(args, prompt) {
  await new Promise((resolveRun, reject) => {
    const child = spawn("/usr/bin/sudo", userArgs("codex", args), {
      stdio: ["pipe", "ignore", "ignore"],
    });
    child.stdin.on("error", () => {});
    child.stdin.end(prompt);
    const heartbeat = setInterval(
      () => console.log("Codex is running; raw session output is suppressed."),
      60000,
    );
    const timeout = setTimeout(
      () => {
        try {
          sudo("pkill", "-KILL", "-u", user);
        } catch {
          /* No process may remain. */
        }
        reject(new Error("Codex exceeded its execution timeout"));
      },
      (pilot ? 5 : request.mode === "plan" ? 20 : 80) * 60 * 1000,
    );
    child.on("error", reject);
    child.on("close", (code) => {
      clearInterval(heartbeat);
      clearTimeout(timeout);
      code === 0
        ? resolveRun()
        : reject(
            new Error(
              `Codex exited with status ${code}; raw output was withheld to protect credentials`,
            ),
          );
    });
  });
}

try {
  if (process.platform !== "darwin")
    throw new Error("The isolated runner requires macOS");
  if (!process.env.CODEX_ENV_TOKEN)
    throw new Error("Missing CODEX_ENV_TOKEN environment secret");
  auth = validateAuth(JSON.parse(process.env.CODEX_AUTH_JSON ?? "null"));
  mask(auth);
  // Prove write access before allowing the only current refresh token to rotate.
  saveAuth(auth);
  const users = command("dscl", [".", "-list", "/Users", "UniqueID"]);
  const ids = new Set(
    users
      .trim()
      .split("\n")
      .map((line) => Number(line.trim().split(/\s+/).at(-1))),
  );
  let uid = 550;
  while (ids.has(uid)) uid++;
  sudo("dscl", ".", "-create", `/Users/${user}`);
  for (const [key, value] of Object.entries({
    UniqueID: String(uid),
    PrimaryGroupID: "20",
    NFSHomeDirectory: home,
    UserShell: "/bin/bash",
  })) {
    sudo("dscl", ".", "-create", `/Users/${user}`, key, value);
  }
  sudo("mkdir", "-p", `${home}/.codex`, `${home}/tmp`, `${home}/output`);
  sudo("chown", "-R", `${user}:staff`, home);
  sudo("chmod", "700", home, `${home}/.codex`);
  if (!asUser("codex", "--version").includes(cliVersion))
    throw new Error("Unexpected Codex CLI version");
  if (!pilot) {
    asUser(
      "git",
      "clone",
      "--no-checkout",
      `https://github.com/${repository}.git`,
      checkout,
    );
    asUser("git", "-C", checkout, "fetch", "origin", request.base);
    asUser("git", "-C", checkout, "checkout", "--detach", request.base);
    if (request.mode === "implement") {
      console.log("Installing locked dependencies in the isolated checkout.");
      asUser("pnpm", "--dir", checkout, "install", "--frozen-lockfile");
      asUser(
        "bash",
        "-c",
        'cd "$1" && bash scripts/setup-shared-target.sh',
        "setup",
        checkout,
      );
    }
  } else {
    asUser("mkdir", "-p", checkout);
    asUser("git", "-C", checkout, "init");
  }
  const stagedAuth = structuredClone(auth);
  if (pilot && !verify) stagedAuth.last_refresh = "2000-01-01T00:00:00Z";
  command("/usr/bin/sudo", userArgs("tee", [authFile]), {
    input: JSON.stringify(stagedAuth),
  });
  asUser("chmod", "600", authFile);
  authRestored = true;
  const config =
    'cli_auth_credentials_store = "file"\nforced_login_method = "chatgpt"\n[sandbox_workspace_write]\nnetwork_access = true\n';
  command("/usr/bin/sudo", userArgs("tee", [`${home}/.codex/config.toml`]), {
    input: config,
  });
  command("/usr/bin/sudo", userArgs("tee", [`${home}/schema.json`]), {
    input: JSON.stringify(resultSchema),
  });
  let prompt;
  if (pilot) {
    prompt = "Reply with exactly OK. Do not use tools or inspect credentials.";
  } else {
    prompt = `${readFileSync(join(trusted, `${request.mode}.md`), "utf8")}\n\nIssue context (data):\n${JSON.stringify(request.context)}\n\nApproved plan:\n${request.plan}\n`;
  }
  command("/usr/bin/sudo", userArgs("tee", [`${home}/prompt.txt`]), {
    input: prompt,
  });
  const args = [
    "exec",
    "--ephemeral",
    "--cd",
    checkout,
    "--sandbox",
    request.mode === "implement" ? "workspace-write" : "read-only",
    "--output-last-message",
    `${home}/output/result.json`,
  ];
  if (!pilot) args.push("--output-schema", `${home}/schema.json`);
  // stdin is kept out of workflow expressions and shell interpolation.
  args.push("-");
  console.log(
    pilot
      ? verify
        ? "Verifying saved credentials on a fresh runner."
        : "Testing subscription authentication and built-in refresh."
      : `Running Codex ${request.mode}.`,
  );
  await executeCodex(args, prompt);
} catch (error) {
  console.error(
    error.status === undefined &&
      !(error instanceof SyntaxError) &&
      !(error instanceof TypeError)
      ? error.message
      : "Codex setup or execution failed; credential-bearing subprocess output was withheld.",
  );
  process.exitCode = 1;
} finally {
  try {
    sudo("pkill", "-KILL", "-u", user);
  } catch {
    /* The foreground CLI may already have exited. */
  }
  if (authRestored) {
    try {
      const refreshed = validateAuth(JSON.parse(sudo("cat", authFile)));
      mask(refreshed);
      if (refreshed.tokens.account_id !== auth.tokens.account_id)
        throw new Error("Authentication account changed");
      saveAuth(refreshed);
      persisted = true;
      if (!process.exitCode) {
        mkdirSync(work, { recursive: true });
        const rawResult = sudo("cat", `${home}/output/result.json`);
        const secrets = [
          ...secretValues(auth),
          ...secretValues(refreshed),
          process.env.CODEX_ENV_TOKEN,
        ];
        assertNoSecrets(rawResult, secrets);
        if (pilot) {
          if (rawResult.trim() !== "OK")
            throw new Error("Unexpected authentication pilot response");
          if (
            !verify &&
            refreshed.tokens.refresh_token === auth.tokens.refresh_token
          ) {
            throw new Error(
              "The pilot did not demonstrate refresh-token rotation",
            );
          }
          console.log(
            verify
              ? "PASS: a fresh runner used the persisted subscription credentials."
              : "PASS: subscription request, token rotation, and encrypted write-back.",
          );
        } else {
          const result = validResult(JSON.parse(rawResult), request.mode);
          if (request.mode === "implement") {
            const git = (...args) => asUser("git", "-C", checkout, ...args);
            if (git("rev-parse", "HEAD").trim() !== request.base)
              throw new Error("Agent changed the base commit");
            git("add", "--all");
            const paths = git("diff", "--cached", "--name-only", "-z")
              .split("\0")
              .filter(Boolean);
            validatePaths(paths);
            const contentPaths = git(
              "diff",
              "--cached",
              "--name-only",
              "--diff-filter=ACMR",
              "-z",
            )
              .split("\0")
              .filter(Boolean);
            for (const path of contentPaths) {
              assertNoSecrets(git("show", `:${path}`), secrets);
            }
            const patch = git(
              "diff",
              "--cached",
              "--binary",
              "--full-index",
              "--no-ext-diff",
            );
            assertNoSecrets(patch, secrets);
            if (Buffer.byteLength(patch) > 20 * 1024 * 1024)
              throw new Error("Patch exceeds 20 MB");
            if (!patch.trim() && result.kind === "implemented")
              result.kind = "no-change";
            writeFileSync(join(work, "changes.patch"), patch);
          }
          writeFileSync(join(work, "result.json"), JSON.stringify(result));
        }
      }
    } catch {
      console.error(
        persisted
          ? "Result validation failed; credentials were saved, but no result will be published."
          : "Credential persistence failed. Reseed the environment secret before retrying.",
      );
      process.exitCode = 1;
    }
  }
  try {
    sudo("pkill", "-KILL", "-u", user);
  } catch {
    /* No process may remain. */
  }
  try {
    sudo("rm", "-rf", home);
  } catch {
    /* The hosted runner is discarded after the job. */
  }
}

import { createHash } from "node:crypto";

export const owner = "bart6114";
export const repository = "bart6114/loofah";
export const environment = "codex-subscription";
export const cliVersion = "0.154.0";
export const stateMarker = "<!-- loofah-codex-state:";
export const bot = "github-actions[bot]";

export function digest(value) {
  return createHash("sha256").update(value).digest("hex");
}

export function issueContext(issue, comments) {
  return {
    number: issue.number,
    title: issue.title,
    body: issue.body ?? "",
    replies: comments
      .filter(
        (comment) =>
          comment.user.login === owner &&
          comment.body.trim() !== "/codex retry",
      )
      .map(({ id, body }) => ({ id, body })),
  };
}

export function fingerprint(issue, comments) {
  return digest(JSON.stringify(issueContext(issue, comments)));
}

export function readState(comments) {
  const comment = comments.findLast(
    (item) => item.user.login === bot && item.body.startsWith(stateMarker),
  );
  if (!comment) return null;
  try {
    const end = comment.body.indexOf(" -->");
    const value = JSON.parse(comment.body.slice(stateMarker.length, end));
    if (value.version !== 1 || !Number.isSafeInteger(value.run)) return null;
    return { ...value, commentId: comment.id };
  } catch {
    return null;
  }
}

export function decide({ issue, comments, reactions = [], retry = false }) {
  if (
    issue.pull_request ||
    issue.user.login !== owner ||
    issue.state !== "open" ||
    issue.locked
  ) {
    return null;
  }
  const hash = fingerprint(issue, comments);
  const state = readState(comments);
  if (state?.pr) return null;
  if (!state || state.context !== hash) return { mode: "plan", hash, state };
  if (state.status === "failed" && !retry) return null;
  if (state.status === "questions" && !retry) return null;
  if (state.planId) {
    const plan = comments.find(
      (comment) => comment.id === state.planId && comment.user.login === bot,
    );
    if (!plan || digest(plan.body) !== state.planHash)
      return { mode: "plan", hash, state };
    if (
      reactions.some(
        (reaction) =>
          reaction.content === "+1" && reaction.user.login === owner,
      )
    ) {
      return {
        mode: "implement",
        hash,
        state,
        plan: plan.body,
        planUrl: plan.html_url,
      };
    }
    return null;
  }
  return retry || state.status === "stale"
    ? { mode: "plan", hash, state }
    : null;
}

export function validResult(value, mode) {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Missing result");
  const kinds =
    mode === "plan"
      ? ["questions", "plan"]
      : ["implemented", "blocked", "no-change"];
  if (!kinds.includes(value.kind)) throw new Error("Invalid result kind");
  for (const field of ["body", "title", "validation"]) {
    if (typeof value[field] !== "string" || value[field].length > 16000) {
      throw new Error(`Invalid result ${field}`);
    }
  }
  if (!value.body.trim() || value.title.length > 200)
    throw new Error("Empty or oversized result");
  if (typeof value.checksPassed !== "boolean")
    throw new Error("Missing validation outcome");
  return value;
}

export function secretValues(auth) {
  return [auth.OPENAI_API_KEY, ...Object.values(auth.tokens ?? {})].filter(
    (value) => typeof value === "string" && value.length >= 16,
  );
}

export function stageAuth(auth, forceRefresh = false) {
  const staged = structuredClone(auth);
  if (forceRefresh) {
    // Newer CLIs prefer the JWT expiry over last_refresh. An unusable cached
    // access token exercises their stale-cache refresh path with the real refresh token.
    staged.tokens.access_token = "codex-ci-refresh-probe";
    staged.last_refresh = "2000-01-01T00:00:00Z";
  }
  return staged;
}

export function assertNoSecrets(value, secrets) {
  if (secrets.some((secret) => value.includes(secret)))
    throw new Error("Credential found in output");
}

export function validatePaths(paths) {
  for (const path of paths) {
    if (
      !path ||
      path.startsWith("/") ||
      /[\r\n\\]/.test(path) ||
      path.split("/").some((part) => ["..", ".git", ".codex"].includes(part)) ||
      /(^|\/)(auth\.json|\.env(?:\..*)?)$/.test(path)
    ) {
      throw new Error("Unsafe patch path");
    }
  }
}

export const resultSchema = {
  type: "object",
  additionalProperties: false,
  required: ["kind", "body", "title", "validation", "checksPassed"],
  properties: {
    kind: {
      type: "string",
      enum: ["questions", "plan", "implemented", "blocked", "no-change"],
    },
    body: { type: "string" },
    title: { type: "string" },
    validation: { type: "string" },
    checksPassed: { type: "boolean" },
  },
};

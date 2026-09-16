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

export function decide({ issue, comments, retry = false }) {
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
  if (!state || state.context !== hash) return { mode: "review", hash };
  if (retry || ["stale", "plan"].includes(state.status))
    return { mode: "review", hash };
  return null;
}

export function validResult(value) {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Missing result");
  if (!["questions", "ready"].includes(value.kind))
    throw new Error("Invalid result kind");
  if (
    typeof value.body !== "string" ||
    !value.body.trim() ||
    value.body.length > 16000
  ) {
    throw new Error("Invalid result body");
  }
  return { kind: value.kind, body: value.body };
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

export const resultSchema = {
  type: "object",
  additionalProperties: false,
  required: ["kind", "body"],
  properties: {
    kind: { type: "string", enum: ["questions", "ready"] },
    body: { type: "string" },
  },
};

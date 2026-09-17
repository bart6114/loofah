import { execFile } from "node:child_process";
import { randomBytes, createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";
import ts from "typescript";

export function normalizeEmail(value) {
  const email = value?.trim().toLowerCase();
  if (!email || email.length > 254 || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email))
    throw new Error("Provide a valid email address.");
  return email;
}

export async function administrator() {
  // Capture credentials in memory. Never inherit stdout from `wrangler auth token`.
  let credentials;
  try {
    await promisify(execFile)(
      "pnpm",
      ["exec", "wrangler", "whoami", "--json"],
      {
        cwd: new URL("..", import.meta.url),
        env: { ...process.env, WRANGLER_SEND_METRICS: "false" },
      },
    );
    const { stdout } = await promisify(execFile)(
      "pnpm",
      ["exec", "wrangler", "auth", "token", "--json"],
      {
        cwd: new URL("..", import.meta.url),
        env: { ...process.env, WRANGLER_SEND_METRICS: "false" },
      },
    );
    credentials = JSON.parse(stdout);
  } catch {
    throw new Error(
      "Cloudflare administrator authentication failed. Run wrangler login or configure CLOUDFLARE_API_TOKEN.",
    );
  }
  const headers =
    credentials.type === "api_key"
      ? { "X-Auth-Key": credentials.key, "X-Auth-Email": credentials.email }
      : { Authorization: `Bearer ${credentials.token}` };
  return async (path, body) => {
    let response;
    try {
      response = await fetch(`https://api.cloudflare.com/client/v4${path}`, {
        method: body === undefined ? "GET" : "POST",
        headers: { ...headers, "Content-Type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(30_000),
      });
    } catch {
      throw new Error(
        "Cloudflare request failed. Inspect remote state before retrying a mutation.",
      );
    }
    const value = await response.json();
    if (!response.ok || !value.success)
      throw new Error(
        `Cloudflare request failed (${response.status}); codes: ${(value.errors ?? []).map((error) => error.code).join(",")}`,
      );
    return value.result;
  };
}

export async function run(args, api, config, input = process.stdin) {
  const [environment, command, value, extra] = args;
  if (environment !== "staging")
    throw new Error("This milestone permits administration of staging only.");
  const target = config.env[environment];
  const account = `/accounts/${config.account_id}`;
  const database = target.d1_databases.find(
    (binding) => binding.binding === "DB",
  );
  const query = async (sql, params = []) => {
    const result = await api(
      `${account}/d1/database/${database.database_id}/query`,
      { sql, params },
    );
    if (result.length !== 1 || !result[0].success)
      throw new Error("Database operation failed.");
    return result[0].results;
  };
  if (command === "invite") {
    if (args.length > 4) throw new Error("Usage: staging invite EMAIL [DAYS]");
    const email = normalizeEmail(value);
    const days = extra === undefined ? 7 : Number(extra);
    if (!Number.isInteger(days) || days < 1 || days > 30)
      throw new Error("Expiry must be 1–30 days.");
    const token = randomBytes(32).toString("hex");
    const id = createHash("sha256").update(token).digest("hex");
    await query(
      "INSERT INTO sync_invitations (id,email,expires_at) VALUES (?,?,?)",
      [id, email, Date.now() + days * 86_400_000],
    );
    return `${target.vars.ACCOUNT_ORIGIN}/signup#invitation=${token}`;
  }
  if (command === "list" && args.length === 2) {
    return query(
      `SELECT id, email, expires_at, consumed_at, revoked_at FROM sync_invitations ORDER BY email, expires_at DESC`,
    );
  }
  if (command === "revoke" && args.length === 3) {
    if (!/^[a-f0-9]{64}$/.test(value ?? ""))
      throw new Error("Provide the invitation hash shown by list.");
    return query(
      "UPDATE sync_invitations SET revoked_at = ? WHERE id = ? AND consumed_user IS NULL AND revoked_at IS NULL RETURNING id, email, revoked_at",
      [Date.now(), value],
    );
  }
  if (command === "capacity" && args.length <= 3) {
    if (value !== undefined) {
      const limit = Number(value);
      if (!Number.isSafeInteger(limit) || limit < 1)
        throw new Error("Capacity must be a positive integer.");
      await query(
        "UPDATE sync_beta_settings SET account_limit = ? WHERE id = 1",
        [limit],
      );
    }
    return query(
      "SELECT account_limit, (SELECT count(*) FROM user) AS accounts FROM sync_beta_settings WHERE id = 1",
    );
  }
  if (command === "send" && args.length === 3) {
    const email = normalizeEmail(value);
    if (input.isTTY)
      throw new Error(
        "Pass the invitation URL on stdin; never as a command argument.",
      );
    let link = "";
    for await (const chunk of input) {
      link += chunk.toString();
      if (link.length > 1024) throw new Error("Invalid invitation URL.");
    }
    const url = new URL(link.trim());
    const token = new URLSearchParams(url.hash.slice(1)).get("invitation");
    if (
      url.origin !== target.vars.ACCOUNT_ORIGIN ||
      url.pathname !== "/signup" ||
      url.search ||
      !/^[a-f0-9]{64}$/.test(token ?? "")
    )
      throw new Error("Invalid invitation URL.");
    const id = createHash("sha256").update(token).digest("hex");
    const rows = await query(
      "SELECT id FROM sync_invitations WHERE id = ? AND email = ? AND consumed_user IS NULL AND revoked_at IS NULL AND expires_at > ?",
      [id, email, Date.now()],
    );
    if (rows.length !== 1)
      throw new Error("Invitation is not available for this email.");
    await api(`${account}/email/sending/send`, {
      from: "accounts@notify.loofah.io",
      to: [email],
      subject: "Your Loofah Staging invitation",
      text: `You’re invited to try Loofah Sync in Loofah Staging. Create your account:\n\n${url.origin}/signup#invitation=${token}\n\nUse copied test data for this private staging test.`,
    });
    return "Invitation submitted for delivery.";
  }
  throw new Error(
    "Usage: admin staging invite EMAIL [DAYS] | list | revoke HASH | capacity [LIMIT] | send EMAIL < invitation-url-file",
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  try {
    const parsed = ts.parseConfigFileTextToJson(
      "wrangler.jsonc",
      await readFile(new URL("../wrangler.jsonc", import.meta.url), "utf8"),
    );
    if (parsed.error) throw new Error("Invalid Wrangler configuration.");
    const result = await run(
      process.argv.slice(2),
      await administrator(),
      parsed.config,
    );
    process.stdout.write(
      typeof result === "string"
        ? `${result}\n`
        : `${JSON.stringify(result, null, 2)}\n`,
    );
  } catch (error) {
    // Do not print raw provider errors or request bodies, which may contain secrets.
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}

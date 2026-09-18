import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";

const environment = process.argv[2];
assert.ok(
  ["staging", "production"].includes(environment),
  "Choose staging or production explicitly.",
);
const parsed = ts.parseConfigFileTextToJson(
  "wrangler.jsonc",
  await readFile(new URL("../wrangler.jsonc", import.meta.url), "utf8"),
);
assert.ok(!parsed.error, "Invalid Wrangler configuration.");
const config = parsed.config;
const target = config.env[environment];
assert.equal(
  process.env.CLOUDFLARE_ACCOUNT_ID,
  config.account_id,
  "Unexpected Cloudflare account.",
);
assert.ok(
  process.env.CLOUDFLARE_API_TOKEN,
  "CLOUDFLARE_API_TOKEN is required; this must also run with the deployment CI token.",
);

async function api(path, headers = {}) {
  const response = await fetch(`https://api.cloudflare.com/client/v4${path}`, {
    headers: {
      Authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`,
      ...headers,
    },
    signal: AbortSignal.timeout(20_000),
  });
  const data = await response.json();
  if (!response.ok || !data.success)
    throw new Error(
      `Preflight failed (${response.status}): ${path}; codes: ${(data.errors ?? []).map((error) => error.code).join(",")}`,
    );
  return data.result;
}

const account = `/accounts/${config.account_id}`;
for (const binding of target.d1_databases) {
  const database = await api(`${account}/d1/database/${binding.database_id}`);
  assert.equal(database.name, binding.database_name);
  assert.equal(
    database.jurisdiction,
    "eu",
    "D1 must have an explicit EU jurisdiction.",
  );
}
for (const binding of target.r2_buckets) {
  const headers = { "cf-r2-jurisdiction": "eu" };
  const bucket = await api(
    `${account}/r2/buckets/${binding.bucket_name}`,
    headers,
  );
  assert.equal(
    bucket.jurisdiction,
    "eu",
    "R2 must have an explicit EU jurisdiction.",
  );
  const managed = await api(
    `${account}/r2/buckets/${binding.bucket_name}/domains/managed`,
    headers,
  );
  assert.equal(managed.enabled, false, "R2 public access must be disabled.");
  const custom = await api(
    `${account}/r2/buckets/${binding.bucket_name}/domains/custom`,
    headers,
  );
  assert.equal(
    custom.domains.length,
    0,
    "Vault and recovery buckets cannot have public domains.",
  );
}
await api(`${account}/workers/scripts`);
const domains = await api(
  "/zones/481fd8bab0dddfac498d6d0ec8a43596/email/sending/subdomains",
);
const sending = (Array.isArray(domains) ? domains : domains.subdomains).find(
  (domain) => domain.name === "notify.loofah.io",
);
assert.ok(
  sending?.enabled,
  "Email Sending is not enabled for notify.loofah.io.",
);
console.log(
  `Cloudflare resource and read-permission preflight passed for ${environment}. Deployment write permissions still require a successful staging deployment.`,
);

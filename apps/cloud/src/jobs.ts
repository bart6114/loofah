export class DurableJobs {
  constructor(private readonly db: D1Database) {}

  async enqueue(id: string, kind: string, payload: string, now = Date.now()) {
    if (payload.length > 64 * 1024)
      throw new Error("job payload exceeds limit");
    await this.db
      .prepare(`INSERT INTO sync_jobs (id, kind, payload, available_at) VALUES (?, ?, ?, ?)
      ON CONFLICT (id) DO NOTHING`)
      .bind(id, kind, payload, now)
      .run();
  }

  async run(
    handlers: Record<string, (payload: string) => Promise<void>>,
    limit = 4,
    clock = Date.now,
  ) {
    for (let count = 0; count < Math.min(limit, 8); count++) {
      const token = crypto.randomUUID();
      const now = clock();
      const job = await this.db
        .prepare(`UPDATE sync_jobs SET lease_token = ?, lease_until = ?, attempts = attempts + 1
        WHERE id = (SELECT id FROM sync_jobs WHERE completed_at IS NULL AND failed_at IS NULL
          AND available_at <= ? AND (lease_until IS NULL OR lease_until < ?) ORDER BY available_at, id LIMIT 1)
        RETURNING id, kind, payload, attempts`)
        .bind(token, now + 120_000, now, now)
        .first<{
          id: string;
          kind: string;
          payload: string;
          attempts: number;
        }>();
      if (!job) break;
      try {
        const handler = handlers[job.kind];
        if (!handler) throw new Error("unsupported job");
        await handler(job.payload);
        await this.db
          .prepare(`UPDATE sync_jobs SET completed_at = ?, payload = '', lease_token = NULL, lease_until = NULL
          WHERE id = ? AND lease_token = ?`)
          .bind(clock(), job.id, token)
          .run();
      } catch {
        const terminal = job.attempts >= 12 || !handlers[job.kind];
        await this.db
          .prepare(`UPDATE sync_jobs SET failed_at = ?, failure_code = ?, available_at = ?,
          lease_token = NULL, lease_until = NULL WHERE id = ? AND lease_token = ?`)
          .bind(
            terminal ? clock() : null,
            handlers[job.kind] ? "delivery_failed" : "unsupported_job",
            clock() +
              Math.min(
                21_600_000,
                60_000 * 2 ** Math.min(job.attempts - 1, 10),
              ),
            job.id,
            token,
          )
          .run();
      }
    }
  }
}

export async function sha256(value: string) {
  return Array.from(
    new Uint8Array(
      await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value)),
    ),
    (byte) => byte.toString(16).padStart(2, "0"),
  ).join("");
}

export async function sealJob(secret: string, value: unknown) {
  const key = await jobKey(secret);
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const sealed = await crypto.subtle.encrypt(
    {
      name: "AES-GCM",
      iv: nonce,
      additionalData: new TextEncoder().encode("loofah-email-job-v1"),
    },
    key,
    new TextEncoder().encode(JSON.stringify(value)),
  );
  return JSON.stringify({
    version: 1,
    nonce: Array.from(nonce),
    sealed: Array.from(new Uint8Array(sealed)),
  });
}

export async function openJob(
  secret: string,
  payload: string,
): Promise<unknown> {
  const value = JSON.parse(payload);
  if (
    value.version !== 1 ||
    !Array.isArray(value.nonce) ||
    value.nonce.length !== 12 ||
    !Array.isArray(value.sealed)
  ) {
    throw new Error("invalid job");
  }
  const plaintext = await crypto.subtle.decrypt(
    {
      name: "AES-GCM",
      iv: new Uint8Array(value.nonce),
      additionalData: new TextEncoder().encode("loofah-email-job-v1"),
    },
    await jobKey(secret),
    new Uint8Array(value.sealed),
  );
  return JSON.parse(new TextDecoder().decode(plaintext));
}

async function jobKey(secret: string) {
  if (!/^[a-f0-9]{64}$/.test(secret)) throw new Error("invalid email job key");
  return crypto.subtle.importKey(
    "raw",
    new Uint8Array(
      secret.match(/../g)!.map((byte) => Number.parseInt(byte, 16)),
    ),
    "AES-GCM",
    false,
    ["encrypt", "decrypt"],
  );
}

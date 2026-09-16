import { createAuth } from "../src/auth.ts";
import { ObjectStorage, PART_BYTES } from "../src/objects.ts";
import { password } from "../src/password.ts";
import { VaultStorage } from "../src/storage.ts";

export default {
  async fetch(request: Request, env: { DB: D1Database; VAULT: R2Bucket }) {
    const action = new URL(request.url).pathname;
    if (action === "/auth-health") {
      const auth = createAuth({
        DB: env.DB,
        AUTH_SECRET: "runtime-test-only-0000000000000000000000000",
        EMAIL_JOB_KEY: "00".repeat(32),
        ACCOUNT_ORIGIN: "https://staging-app.loofah.io",
        SIGNUP_OPEN: "false",
      });
      return Response.json({
        session: await auth.api.getSession({ headers: new Headers() }),
      });
    }
    if (action === "/initialize") {
      const statements = await request.json<string[]>();
      await env.DB.batch(
        statements.map((statement) => env.DB.prepare(statement)),
      );
      return Response.json({ initialized: true });
    }
    if (action === "/password") {
      const start = performance.now();
      const hash = await password.hash("a worker compatibility test password");
      const hashed = performance.now();
      const valid = await password.verify({
        hash,
        password: "a worker compatibility test password",
      });
      const invalid = await password.verify({
        hash,
        password: "a different password",
      });
      return Response.json({
        valid,
        invalid,
        hashMs: hashed - start,
        verifyMs: (performance.now() - hashed) / 2,
      });
    }
    const principal = {
      vault: crypto.randomUUID(),
      device: crypto.randomUUID(),
      generation: crypto.randomUUID(),
    };
    await env.DB.prepare(
      "INSERT INTO sync_accounts (vault_id,user_id,recovery_generation) VALUES (?,?,?)",
    )
      .bind(principal.vault, crypto.randomUUID(), principal.generation)
      .run();
    const store = new VaultStorage(env.DB, principal);
    const bucket = new Proxy(env.VAULT, {
      get(target, property) {
        if (property === "resumeMultipartUpload") {
          return (key: string, uploadId: string) => {
            const upload = target.resumeMultipartUpload(key, uploadId);
            return new Proxy(upload, {
              get(target, property) {
                if (property === "complete") {
                  return async (parts: R2UploadedPart[]) => ({
                    ...(await target.complete(parts)),
                    customMetadata: undefined,
                  });
                }
                const value = Reflect.get(target, property, target);
                return typeof value === "function" ? value.bind(target) : value;
              },
            });
          };
        }
        const value = Reflect.get(target, property, target);
        return typeof value === "function" ? value.bind(target) : value;
      },
    });
    const objects = new ObjectStorage(env.DB, bucket, principal);
    const hash = async (bytes: ArrayBuffer) =>
      Array.from(
        new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)),
        (byte) => byte.toString(16).padStart(2, "0"),
      ).join("");
    const data = new Uint8Array(PART_BYTES + 17).fill(37).buffer;
    const id = crypto.randomUUID();
    await store.reserve([
      { id, bytes: data.byteLength, digest: await hash(data), kind: "file" },
    ]);
    await objects.startMultipart(id);
    const first = data.slice(0, PART_BYTES);
    const last = data.slice(PART_BYTES);
    await objects.putPart(id, 1, await hash(first), first);
    const resumed = await objects.startMultipart(id);
    if (resumed.parts.length !== 1) throw new Error("missing resumable part");
    let rejected = false;
    try {
      await objects.putPart(
        id,
        1,
        await hash(new Uint8Array(PART_BYTES).buffer),
        new Uint8Array(PART_BYTES).buffer,
      );
    } catch {
      rejected = true;
    }
    if (!rejected) throw new Error("immutable part replaced");
    await objects.putPart(id, 1, await hash(first), first);
    await objects.putPart(id, 2, await hash(last), last);
    await objects.completeMultipart(id);
    await objects.completeMultipart(id);
    const downloaded = await (await objects.get(id)).arrayBuffer();
    if ((await hash(downloaded)) !== (await hash(data)))
      throw new Error("multipart bytes changed");
    const manifest = new Uint8Array([8, 7, 6, 5]).buffer;
    const manifestId = crypto.randomUUID();
    await store.reserve([
      {
        id: manifestId,
        bytes: manifest.byteLength,
        digest: await hash(manifest),
        kind: "manifest",
      },
    ]);
    await objects.put(manifestId, manifest);
    await objects.put(manifestId, manifest);
    await store.commit({
      id: crypto.randomUUID(),
      entity: "a".repeat(64),
      expected: null,
      manifest: manifestId,
      operation: "checkpoint",
      objects: [id],
    });
    const usage = await env.DB.prepare(
      "SELECT used_bytes FROM sync_accounts WHERE vault_id = ?",
    )
      .bind(principal.vault)
      .first<{ used_bytes: number }>();
    return Response.json({
      bytes: downloaded.byteLength,
      usage: usage?.used_bytes,
      rejected,
    });
  },
} satisfies ExportedHandler<{ DB: D1Database; VAULT: R2Bucket }>;

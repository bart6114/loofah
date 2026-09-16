import { StorageError, type VaultPrincipal } from "./storage.ts";

export const PART_BYTES = 8 * 1024 * 1024;

export class ObjectStorage {
  constructor(
    private readonly db: D1Database,
    private readonly bucket: R2Bucket,
    private readonly principal: VaultPrincipal,
  ) {}

  private key(id: string) {
    if (!/^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(id))
      throw new StorageError("invalid");
    return `${this.principal.vault}/${id}`;
  }

  private async reservation(id: string) {
    this.key(id);
    const row = await this.db
      .prepare(`SELECT o.bytes, o.digest, o.state, o.multipart_id FROM sync_objects o
      JOIN sync_accounts a ON a.vault_id = o.vault_id WHERE o.vault_id = ? AND o.id = ?
      AND a.active = 1 AND a.recovery_generation = ? AND o.state IN ('reserved', 'uploaded')`)
      .bind(this.principal.vault, id, this.principal.generation)
      .first<{
        bytes: number;
        digest: string;
        state: "reserved" | "uploaded";
        multipart_id: string | null;
      }>();
    if (!row) throw new StorageError("unavailable");
    return row;
  }

  async put(id: string, body: ArrayBuffer) {
    const object = await this.reservation(id);
    if (
      body.byteLength > PART_BYTES ||
      body.byteLength !== object.bytes ||
      object.multipart_id
    )
      throw new StorageError("invalid");
    if ((await digest(body)) !== object.digest)
      throw new StorageError("invalid");
    if (object.state === "uploaded") return;
    await this.bucket.put(this.key(id), body, {
      onlyIf: { etagDoesNotMatch: "*" },
      sha256: object.digest,
      customMetadata: { digest: object.digest },
      httpMetadata: { contentType: "application/octet-stream" },
    });
    const head = await this.bucket.head(this.key(id));
    if (
      !head ||
      head.size !== object.bytes ||
      head.customMetadata?.digest !== object.digest
    )
      throw new StorageError("unavailable");
    await this.available(id);
  }

  async startMultipart(id: string) {
    const object = await this.reservation(id);
    if (
      object.bytes <= PART_BYTES ||
      Math.ceil(object.bytes / PART_BYTES) > 10000
    )
      throw new StorageError("invalid");
    if (object.state === "uploaded") return { complete: true, parts: [] };
    if (!object.multipart_id) {
      const upload = await this.bucket.createMultipartUpload(this.key(id), {
        customMetadata: { digest: object.digest },
        httpMetadata: { contentType: "application/octet-stream" },
      });
      const saved = await this.db
        .prepare(`UPDATE sync_objects SET multipart_id = ? WHERE vault_id = ? AND id = ?
        AND state = 'reserved' AND multipart_id IS NULL RETURNING id`)
        .bind(upload.uploadId, this.principal.vault, id)
        .first();
      if (!saved) await upload.abort();
    }
    const parts = await this.db
      .prepare(`SELECT number, bytes, digest FROM sync_upload_parts
      WHERE vault_id = ? AND object_id = ? AND etag IS NOT NULL ORDER BY number`)
      .bind(this.principal.vault, id)
      .all();
    return { complete: false, parts: parts.results };
  }

  async putPart(
    id: string,
    number: number,
    expectedDigest: string,
    body: ArrayBuffer,
  ) {
    const object = await this.reservation(id);
    const count = Math.ceil(object.bytes / PART_BYTES);
    const size =
      number === count ? object.bytes - PART_BYTES * (count - 1) : PART_BYTES;
    if (
      object.state !== "reserved" ||
      !object.multipart_id ||
      !Number.isInteger(number) ||
      number < 1 ||
      number > count ||
      size !== body.byteLength ||
      !/^[a-f0-9]{64}$/.test(expectedDigest) ||
      (await digest(body)) !== expectedDigest
    )
      throw new StorageError("invalid");
    try {
      await this.db
        .prepare(
          `INSERT INTO sync_upload_parts (vault_id, object_id, number, bytes, digest) VALUES (?, ?, ?, ?, ?)`,
        )
        .bind(this.principal.vault, id, number, size, expectedDigest)
        .run();
    } catch {
      throw new StorageError("conflict");
    }
    const part = await this.bucket
      .resumeMultipartUpload(this.key(id), object.multipart_id)
      .uploadPart(number, body);
    await this.db
      .prepare(
        `UPDATE sync_upload_parts SET etag = ? WHERE vault_id = ? AND object_id = ? AND number = ?`,
      )
      .bind(part.etag, this.principal.vault, id, number)
      .run();
  }

  async completeMultipart(id: string) {
    const object = await this.reservation(id);
    if (object.state === "uploaded") return;
    if (!object.multipart_id) throw new StorageError("invalid");
    let head = await this.bucket.head(this.key(id));
    if (!head) {
      const parts = await this.db
        .prepare(`SELECT number, bytes, etag FROM sync_upload_parts
        WHERE vault_id = ? AND object_id = ? ORDER BY number`)
        .bind(this.principal.vault, id)
        .all<{ number: number; bytes: number; etag: string | null }>();
      const rows = parts.results;
      if (
        rows.length !== Math.ceil(object.bytes / PART_BYTES) ||
        rows.some((part, index) => !part.etag || part.number !== index + 1) ||
        rows.reduce((sum, part) => sum + part.bytes, 0) !== object.bytes
      )
        throw new StorageError("invalid");
      await this.bucket
        .resumeMultipartUpload(this.key(id), object.multipart_id)
        .complete(
          rows.map((part) => ({ partNumber: part.number, etag: part.etag! })),
        );
      // Completion responses may omit metadata; verify the stored object itself.
      head = await this.bucket.head(this.key(id));
    }
    if (
      !head ||
      head.size !== object.bytes ||
      head.customMetadata?.digest !== object.digest
    )
      throw new StorageError("unavailable");
    await this.available(id);
  }

  private async available(id: string) {
    const row = await this.db
      .prepare(`UPDATE sync_objects SET state = 'uploaded' WHERE vault_id = ? AND id = ?
      AND state IN ('reserved', 'uploaded') RETURNING id`)
      .bind(this.principal.vault, id)
      .first();
    if (!row) throw new StorageError("unavailable");
  }

  async get(id: string) {
    const object = await this.reservation(id);
    if (object.state !== "uploaded") throw new StorageError("unavailable");
    const body = await this.bucket.get(this.key(id));
    if (!body || body.size !== object.bytes)
      throw new StorageError("unavailable");
    return new Response(body.body, {
      headers: {
        "Content-Type": "application/octet-stream",
        "Content-Length": String(body.size),
        "Cache-Control": "no-store",
        "X-Content-Type-Options": "nosniff",
        "X-Loofah-SHA256": object.digest,
      },
    });
  }
}

async function digest(bytes: ArrayBuffer) {
  return Array.from(
    new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)),
    (byte) => byte.toString(16).padStart(2, "0"),
  ).join("");
}

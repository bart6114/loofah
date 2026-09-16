export const HISTORY_MS = 30 * 24 * 60 * 60 * 1000;
export const RECOVERY_RETENTION_MS = 35 * 24 * 60 * 60 * 1000;
const UPLOAD_TTL_MS = 24 * 60 * 60 * 1000;

export type VaultPrincipal = {
  vault: string;
  device: string;
  generation: string;
};

export type RevisionInput = {
  id: string;
  entity: string;
  expected: string | null;
  manifest: string;
  operation: "checkpoint" | "delete" | "restore" | "resolve" | "conflict";
  objects: string[];
  conflicts?: string[];
};

export class StorageError extends Error {
  constructor(
    public readonly code:
      | "invalid"
      | "conflict"
      | "quota"
      | "unavailable"
      | "recovery",
  ) {
    super(code);
  }
}

export class VaultStorage {
  constructor(
    private readonly db: D1Database,
    private readonly principal: VaultPrincipal,
  ) {}

  async reserve(
    input: {
      id: string;
      bytes: number;
      digest: string;
      kind: "file" | "manifest" | "control_manifest";
    }[],
    now = Date.now(),
  ) {
    if (
      !input.length ||
      input.length > 256 ||
      new Set(input.map((item) => item.id)).size !== input.length
    ) {
      throw new StorageError("invalid");
    }
    const statements = input.map((item) => {
      identifier(item.id);
      if (
        !Number.isSafeInteger(item.bytes) ||
        item.bytes <= 0 ||
        !/^[0-9a-f]{64}$/.test(item.digest)
      ) {
        throw new StorageError("invalid");
      }
      return this.db
        .prepare(`INSERT INTO sync_objects
        (vault_id, id, bytes, digest, kind, state, created_at, expires_at)
        VALUES (?, ?, ?, ?, ?, 'reserved', ?, ?)`)
        .bind(
          this.principal.vault,
          item.id,
          item.bytes,
          item.digest,
          item.kind,
          now,
          now + UPLOAD_TTL_MS,
        );
    });
    try {
      await this.db.batch(statements);
    } catch (error) {
      throw classify(error);
    }
  }

  async commit(input: RevisionInput, now = Date.now()) {
    identifier(input.id);
    identifier(input.manifest);
    if (input.expected !== null) identifier(input.expected);
    if (!/^[0-9a-f]{64}$/.test(input.entity) || input.objects.length > 100_000)
      throw new StorageError("invalid");
    const objects = [...new Set(input.objects)].sort();
    if (
      objects.length !== input.objects.length ||
      objects.includes(input.manifest)
    )
      throw new StorageError("invalid");
    objects.forEach(identifier);
    const conflicts = [...new Set(input.conflicts ?? [])].sort();
    if (
      conflicts.length !== (input.conflicts?.length ?? 0) ||
      conflicts.length > 100 ||
      (input.operation !== "resolve" && conflicts.length > 0) ||
      (input.operation === "resolve" && conflicts.length === 0)
    )
      throw new StorageError("invalid");
    conflicts.forEach(identifier);
    const requestHash = await digest(
      JSON.stringify({
        id: input.id,
        entity: input.entity,
        expected: input.expected,
        manifest: input.manifest,
        operation: input.operation,
        objects,
        device: this.principal.device,
        generation: this.principal.generation,
        conflicts,
      }),
    );
    const existing = await this.receipt(input.id);
    if (existing) {
      if (existing.request_hash !== requestHash)
        throw new StorageError("conflict");
      return input.id;
    }
    const statements = [
      this.db
        .prepare(`INSERT INTO sync_revisions
      (vault_id, id, entity, expected_revision, manifest_object, operation, device_id, recovery_generation, request_hash, created_at, pinned)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`)
        .bind(
          this.principal.vault,
          input.id,
          input.entity,
          input.expected,
          input.manifest,
          input.operation,
          this.principal.device,
          this.principal.generation,
          requestHash,
          now,
          input.operation === "conflict" ? 1 : 0,
        ),
    ];
    for (let index = 0; index < objects.length; index += 1000) {
      statements.push(
        this.db
          .prepare(`INSERT INTO sync_membership (vault_id, revision, object)
        SELECT ?, ?, value FROM json_each(?)`)
          .bind(
            this.principal.vault,
            input.id,
            JSON.stringify(objects.slice(index, index + 1000)),
          ),
      );
    }
    if (conflicts.length)
      statements.push(
        this.db
          .prepare(`INSERT INTO sync_resolutions (vault_id, revision, conflict)
      SELECT ?, ?, value FROM json_each(?)`)
          .bind(this.principal.vault, input.id, JSON.stringify(conflicts)),
      );
    try {
      await this.db.batch(statements);
    } catch (error) {
      const committed = await this.receipt(input.id);
      if (committed?.request_hash === requestHash) return input.id;
      throw classify(error);
    }
    return input.id;
  }

  private receipt(id: string) {
    return this.db
      .prepare(`SELECT r.request_hash FROM sync_revisions r JOIN sync_accounts a ON a.vault_id = r.vault_id
      WHERE r.vault_id = ? AND r.id = ? AND a.active = 1 AND a.recovery_generation = ?`)
      .bind(this.principal.vault, id, this.principal.generation)
      .first<{ request_hash: string }>();
  }

  async history(
    entity: string,
    cursor?: { at: number; id: string },
    limit = 50,
  ) {
    if (
      !/^[0-9a-f]{64}$/.test(entity) ||
      (cursor && !Number.isSafeInteger(cursor.at)) ||
      !Number.isInteger(limit) ||
      limit < 1 ||
      limit > 100
    )
      throw new StorageError("invalid");
    if (cursor) identifier(cursor.id);
    const before = cursor?.at ?? Number.MAX_SAFE_INTEGER;
    return this.db
      .prepare(`SELECT r.id, r.operation, r.device_id, r.created_at, r.superseded_at, r.pinned,
      r.manifest_object, h.revision = r.id AS current FROM sync_revisions r
      LEFT JOIN sync_heads h ON h.vault_id = r.vault_id AND h.entity = r.entity
      WHERE r.vault_id = ? AND r.entity = ? AND (r.created_at < ? OR (r.created_at = ? AND r.id < ?))
      ORDER BY r.created_at DESC, r.id DESC LIMIT ?`)
      .bind(
        this.principal.vault,
        entity,
        before,
        before,
        cursor?.id ?? "",
        limit,
      )
      .all();
  }

  async purge(revisions: string[]) {
    if (!revisions.length || revisions.length > 100)
      throw new StorageError("invalid");
    revisions.forEach(identifier);
    try {
      await this.db.batch(
        revisions.map((id) =>
          this.db
            .prepare("DELETE FROM sync_revisions WHERE vault_id = ? AND id = ?")
            .bind(this.principal.vault, id),
        ),
      );
    } catch (error) {
      throw classify(error);
    }
  }
}

export async function expireUploads(db: D1Database, now = Date.now()) {
  return db
    .prepare(`UPDATE sync_objects SET state = 'deleting', charged = 0 WHERE (vault_id, id) IN (
    SELECT vault_id, id FROM sync_objects WHERE state IN ('reserved', 'uploaded') AND refs = 0 AND expires_at < ?
    ORDER BY expires_at LIMIT 100) RETURNING vault_id, id`)
    .bind(now)
    .all<{ vault_id: string; id: string }>();
}

export async function expireHistory(db: D1Database, now = Date.now()) {
  return db
    .prepare(`DELETE FROM sync_revisions WHERE (vault_id, id) IN (
    SELECT r.vault_id, r.id FROM sync_revisions r
    WHERE r.pinned = 0 AND r.superseded_at IS NOT NULL AND r.superseded_at < ?
      AND NOT EXISTS (SELECT 1 FROM sync_heads h WHERE h.vault_id = r.vault_id AND h.revision = r.id)
      AND NOT EXISTS (SELECT 1 FROM sync_snapshot_items s WHERE s.vault_id = r.vault_id AND s.revision = r.id)
    ORDER BY r.superseded_at LIMIT 100)`)
    .bind(now - HISTORY_MS)
    .run();
}

export class VaultSnapshots {
  constructor(
    private readonly db: D1Database,
    private readonly principal: VaultPrincipal,
  ) {}

  async begin(now = Date.now()) {
    const id = crypto.randomUUID();
    return this.db
      .prepare(`INSERT INTO sync_snapshots (id, vault_id, generation, watermark, created_at, expires_at)
      VALUES (?, ?, ?, (SELECT coalesce(max(sequence), 0) FROM sync_changes WHERE vault_id = ?), ?, ?)
      RETURNING id, watermark, generation, expires_at`)
      .bind(
        id,
        this.principal.vault,
        this.principal.generation,
        this.principal.vault,
        now,
        now + 900_000,
      )
      .first();
  }

  async page(id: string, after = "", now = Date.now()) {
    identifier(id);
    const snapshot = await this.db
      .prepare(`UPDATE sync_snapshots SET expires_at = min(?, created_at + 86400000)
      WHERE id = ? AND vault_id = ? AND generation = ? AND expires_at > ? RETURNING watermark, generation, expires_at`)
      .bind(
        now + 900_000,
        id,
        this.principal.vault,
        this.principal.generation,
        now,
      )
      .first();
    if (!snapshot) throw new StorageError("unavailable");
    const items = await this.db
      .prepare(`SELECT i.entity, i.revision, r.operation, r.pinned, r.created_at FROM sync_snapshot_items i
      JOIN sync_revisions r ON r.vault_id = i.vault_id AND r.id = i.revision
      WHERE i.snapshot = ? AND i.entity || ':' || i.revision > ? ORDER BY i.entity, i.revision LIMIT 100`)
      .bind(id, after)
      .all<{ entity: string; revision: string }>();
    const last = items.results.at(-1);
    return {
      snapshot,
      items: items.results,
      next:
        items.results.length === 100 && last
          ? `${last.entity}:${last.revision}`
          : null,
    };
  }

  async release(id: string) {
    identifier(id);
    await this.db
      .prepare("DELETE FROM sync_snapshots WHERE id = ? AND vault_id = ?")
      .bind(id, this.principal.vault)
      .run();
  }
}

export async function markGarbage(db: D1Database, now = Date.now()) {
  return db
    .prepare(`UPDATE sync_objects SET state = 'deleting' WHERE (vault_id, id) IN (
    SELECT vault_id, id FROM sync_objects WHERE state = 'unreferenced' AND refs = 0 AND unreferenced_at < ?
    ORDER BY unreferenced_at LIMIT 100) RETURNING vault_id, id`)
    .bind(now - RECOVERY_RETENTION_MS)
    .all<{ vault_id: string; id: string }>();
}

function identifier(value: string) {
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
      value,
    )
  )
    throw new StorageError("invalid");
}

async function digest(value: string) {
  const result = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(value),
  );
  return [...new Uint8Array(result)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

function classify(error: unknown) {
  const message = error instanceof Error ? error.message : "";
  if (
    message.includes("revision conflict") ||
    message.includes("protected revision") ||
    message.includes("UNIQUE constraint")
  )
    return new StorageError("conflict");
  if (message.includes("used_bytes") || message.includes("control_bytes"))
    return new StorageError("quota");
  if (message.includes("recovery generation"))
    return new StorageError("recovery");
  return new StorageError("unavailable");
}

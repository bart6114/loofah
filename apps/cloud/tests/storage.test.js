import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { afterEach, test } from "node:test";

import {
  VaultStorage,
  VaultSnapshots,
  expireHistory,
  markGarbage,
  HISTORY_MS,
  RECOVERY_RETENTION_MS,
} from "../src/storage.ts";

const databases = [];
afterEach(() => databases.splice(0).forEach((db) => db.close()));

function database() {
  const sql = new DatabaseSync(":memory:");
  databases.push(sql);
  const directory = new URL("../migrations/", import.meta.url);
  for (const file of readdirSync(directory)
    .filter((name) => name.endsWith(".sql"))
    .sort()) {
    sql.exec(readFileSync(new URL(file, directory), "utf8"));
  }
  const db = {
    prepare(query) {
      const statement = (args = []) => ({
        bind: (...values) => statement(values),
        first: async () => sql.prepare(query).get(...args) ?? null,
        all: async () => ({
          results: sql.prepare(query).all(...args),
          success: true,
        }),
        run: async () => ({
          meta: sql.prepare(query).run(...args),
          success: true,
        }),
        execute: () => ({
          meta: sql.prepare(query).run(...args),
          success: true,
        }),
      });
      return statement();
    },
    async batch(statements) {
      sql.exec("BEGIN IMMEDIATE");
      try {
        const result = statements.map((statement) => statement.execute());
        sql.exec("COMMIT");
        return result;
      } catch (error) {
        sql.exec("ROLLBACK");
        throw error;
      }
    },
  };
  const principal = {
    vault: randomUUID(),
    device: randomUUID(),
    generation: randomUUID(),
  };
  sql
    .prepare(
      "INSERT INTO sync_accounts (vault_id, user_id, recovery_generation) VALUES (?, ?, ?)",
    )
    .run(principal.vault, randomUUID(), principal.generation);
  const store = new VaultStorage(db, principal);
  const entity = "a".repeat(64);
  const now = Date.now();
  const usage = () =>
    sql
      .prepare(
        "SELECT used_bytes, control_bytes FROM sync_accounts WHERE vault_id = ?",
      )
      .get(principal.vault);
  async function object(kind = "file", bytes = 100) {
    const item = { id: randomUUID(), bytes, digest: "b".repeat(64), kind };
    await store.reserve([item], now);
    sql
      .prepare(
        "UPDATE sync_objects SET state = 'uploaded' WHERE vault_id = ? AND id = ?",
      )
      .run(principal.vault, item.id);
    return item;
  }
  async function revision(
    objects,
    expected = null,
    operation = "checkpoint",
    at = now,
  ) {
    const manifest = await object(
      ["delete", "restore", "resolve"].includes(operation)
        ? "control_manifest"
        : "manifest",
      10,
    );
    const input = {
      id: randomUUID(),
      entity,
      expected,
      manifest: manifest.id,
      operation,
      objects,
    };
    await store.commit(input, at);
    return input;
  }
  return { sql, db, principal, store, entity, now, usage, object, revision };
}

test("recording reuse counts ciphertext once and retries do not reserve or commit twice", async () => {
  const f = database();
  const recording = await f.object();
  await f.store.reserve([recording]);
  const first = await f.revision([recording.id]);
  const second = await f.revision([recording.id], first.id);
  await f.store.commit(second);
  assert.equal(f.usage().used_bytes, 120);
  assert.equal(
    f.sql
      .prepare("SELECT refs FROM sync_objects WHERE id = ?")
      .get(recording.id).refs,
    2,
  );
  assert.equal(
    f.sql.prepare("SELECT count(*) AS n FROM sync_changes").get().n,
    2,
  );
});

test("concurrent commits have one winner and roll back every loser reference and change", async () => {
  const f = database();
  const first = await f.revision([]);
  const manifests = await Promise.all([
    f.object("manifest", 10),
    f.object("manifest", 10),
  ]);
  const inputs = manifests.map((m) => ({
    ...first,
    id: randomUUID(),
    expected: first.id,
    manifest: m.id,
  }));
  const outcomes = await Promise.allSettled(
    inputs.map((input) => f.store.commit(input)),
  );
  assert.equal(
    outcomes.filter((outcome) => outcome.status === "fulfilled").length,
    1,
  );
  assert.equal(
    f.sql.prepare("SELECT count(*) AS n FROM sync_changes").get().n,
    2,
  );
  const loser =
    inputs[outcomes.findIndex((outcome) => outcome.status === "rejected")];
  assert.equal(
    f.sql
      .prepare("SELECT count(*) AS n FROM sync_membership WHERE revision = ?")
      .get(loser.id).n,
    0,
  );
  assert.equal(f.usage().used_bytes, 30);
});

test("a late membership error rolls back the head, history timestamp, usage and change", async () => {
  const f = database();
  const first = await f.revision([]);
  const manifest = await f.object("manifest", 10);
  const before = f.usage();
  await assert.rejects(
    f.store.commit({
      ...first,
      id: randomUUID(),
      expected: first.id,
      manifest: manifest.id,
      objects: [randomUUID()],
    }),
    { code: "unavailable" },
  );
  assert.equal(
    f.sql.prepare("SELECT revision FROM sync_heads").get().revision,
    first.id,
  );
  assert.equal(
    f.sql
      .prepare("SELECT superseded_at FROM sync_revisions WHERE id = ?")
      .get(first.id).superseded_at,
    null,
  );
  assert.deepEqual(f.usage(), before);
  assert.equal(
    f.sql.prepare("SELECT count(*) AS n FROM sync_changes").get().n,
    1,
  );
});

test("full-quota deletion and restore preserve previous heads through control capacity", async () => {
  const f = database();
  f.sql.exec("UPDATE sync_accounts SET quota_bytes = 1000");
  const file = await f.object("file", 990);
  const first = await f.revision([file.id]);
  await assert.rejects(f.object(), { code: "quota" });
  const deleted = await f.revision([], first.id, "delete");
  const restored = await f.revision([file.id], deleted.id, "restore");
  assert.deepEqual({ ...f.usage() }, { used_bytes: 1000, control_bytes: 20 });
  assert.equal(
    f.sql.prepare("SELECT revision FROM sync_heads").get().revision,
    restored.id,
  );
  assert.equal(
    f.sql.prepare("SELECT count(*) AS n FROM sync_revisions").get().n,
    3,
  );
});

test("expiry preserves current and unresolved conflicts; purge releases quota but retains recovery bytes", async () => {
  const f = database();
  const file = await f.object();
  const old = await f.revision(
    [file.id],
    null,
    "checkpoint",
    f.now - HISTORY_MS - 100,
  );
  const head = await f.revision(
    [],
    old.id,
    "checkpoint",
    f.now - HISTORY_MS - 1,
  );
  const conflict = await f.revision(
    [file.id],
    old.id,
    "conflict",
    f.now - HISTORY_MS - 1,
  );
  await expireHistory(f.db, f.now);
  assert.deepEqual(
    f.sql
      .prepare("SELECT id FROM sync_revisions ORDER BY id")
      .all()
      .map((row) => row.id)
      .sort(),
    [head.id, conflict.id].sort(),
  );
  await assert.rejects(f.store.purge([head.id]), { code: "conflict" });
  await assert.rejects(f.store.purge([conflict.id]), { code: "conflict" });
  f.sql
    .prepare(
      "UPDATE sync_revisions SET pinned = 0, superseded_at = ? WHERE id = ?",
    )
    .run(f.now, conflict.id);
  await f.store.purge([conflict.id]);
  assert.equal(f.usage().used_bytes, 10);
  assert.equal((await markGarbage(f.db, f.now)).results.length, 0);
  assert.equal(
    (await markGarbage(f.db, f.now + RECOVERY_RETENTION_MS + 2000)).results
      .length,
    3,
  );
});

test("cleanup makes an object unavailable before physical deletion, and cross-account references fail", async () => {
  const f = database();
  const file = await f.object();
  const old = await f.revision([file.id]);
  const head = await f.revision([], old.id);
  await f.store.purge([old.id]);
  await markGarbage(f.db, f.now + RECOVERY_RETENTION_MS + 2000);
  const manifest = await f.object("manifest", 10);
  await assert.rejects(
    f.store.commit({
      ...head,
      id: randomUUID(),
      expected: head.id,
      manifest: manifest.id,
      objects: [file.id],
    }),
    { code: "unavailable" },
  );
  const other = { ...f.principal, vault: randomUUID() };
  f.sql
    .prepare(
      "INSERT INTO sync_accounts (vault_id, user_id, recovery_generation) VALUES (?, ?, ?)",
    )
    .run(other.vault, randomUUID(), other.generation);
  await assert.rejects(
    new VaultStorage(f.db, other).commit({
      ...head,
      id: randomUUID(),
      expected: null,
    }),
    { code: "unavailable" },
  );
});

test("server recovery generation changes fail closed even for an otherwise idempotent retry", async () => {
  const f = database();
  const first = await f.revision([]);
  f.sql
    .prepare("UPDATE sync_accounts SET recovery_generation = ?")
    .run(randomUUID());
  await assert.rejects(f.store.commit(first), { code: "recovery" });
});

test("snapshot watermark and pins preserve the initial heads through concurrent commits and history purge", async () => {
  const f = database();
  const first = await f.revision([]);
  const snapshots = new VaultSnapshots(f.db, f.principal);
  const snapshot = await snapshots.begin(f.now);
  const next = await f.revision([], first.id);
  const page = await snapshots.page(snapshot.id, "", f.now + 1);
  assert.equal(page.snapshot.watermark, 1);
  assert.deepEqual(
    page.items.map((item) => item.revision),
    [first.id],
  );
  await assert.rejects(f.store.purge([first.id]));
  assert.equal(
    f.sql.prepare("SELECT revision FROM sync_heads").get().revision,
    next.id,
  );
  await snapshots.release(snapshot.id);
  await f.store.purge([first.id]);
  assert.equal(
    f.sql.prepare("SELECT count(*) AS n FROM sync_revisions").get().n,
    1,
  );
});

test("resolution preserves both versions for history and failed CAS cannot unpin a conflict", async () => {
  const f = database();
  const first = await f.revision([]);
  const conflict = await f.revision([], first.id, "conflict");
  const manifest = await f.object("control_manifest", 10);
  const input = {
    id: randomUUID(),
    entity: f.entity,
    expected: randomUUID(),
    manifest: manifest.id,
    operation: "resolve",
    objects: [],
    conflicts: [conflict.id],
  };
  await assert.rejects(f.store.commit(input), { code: "conflict" });
  assert.equal(
    f.sql
      .prepare("SELECT pinned FROM sync_revisions WHERE id = ?")
      .get(conflict.id).pinned,
    1,
  );
  input.expected = first.id;
  await f.store.commit(input, f.now + 10);
  const resolved = f.sql
    .prepare("SELECT pinned, superseded_at FROM sync_revisions WHERE id = ?")
    .get(conflict.id);
  assert.equal(resolved.pinned, 0);
  assert.equal(resolved.superseded_at, f.now + 10);
  assert.equal(
    f.sql.prepare("SELECT count(*) AS n FROM sync_revisions").get().n,
    3,
  );
  await f.store.commit(input, f.now + 20);
  assert.equal(
    f.sql.prepare("SELECT count(*) AS n FROM sync_changes").get().n,
    3,
  );
});

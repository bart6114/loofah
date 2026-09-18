import assert from "node:assert/strict";
import { test } from "node:test";

import { DurableJobs, openJob, sealJob } from "../src/jobs.ts";
import { database } from "./database.js";

test("durable jobs recover expired leases, retry failures, and keep terminal failures visible", async () => {
  const { sql, db } = database();
  try {
    const jobs = new DurableJobs(db);
    let now = Date.now();
    await jobs.enqueue("a", "email", "payload", now);
    await jobs.enqueue("a", "email", "duplicate", now);
    let calls = 0;
    const handlers = {
      email: async (payload) => {
        calls++;
        assert.equal(payload, "payload");
        throw new Error("secret error details");
      },
    };
    for (let attempt = 1; attempt <= 12; attempt++) {
      await jobs.run(handlers, 1, () => now);
      const row = sql.prepare("SELECT * FROM sync_jobs WHERE id = 'a'").get();
      assert.equal(row.attempts, attempt);
      assert.equal(row.failure_code, "delivery_failed");
      now = row.available_at;
    }
    await jobs.run(handlers, 1, () => now);
    assert.equal(calls, 12);
    assert.ok(
      sql.prepare("SELECT failed_at FROM sync_jobs WHERE id = 'a'").get()
        .failed_at,
    );
    await jobs.enqueue("b", "ok", "private", now);
    sql
      .prepare(
        "UPDATE sync_jobs SET lease_token = 'crashed', lease_until = ? WHERE id = 'b'",
      )
      .run(now - 1);
    await jobs.run({ ok: async () => {} }, 1, () => now);
    const completed = sql
      .prepare("SELECT * FROM sync_jobs WHERE id = 'b'")
      .get();
    assert.equal(completed.payload, "");
    assert.equal(completed.completed_at, now);
  } finally {
    sql.close();
  }
});

test("concurrent job processors acquire one lease and stale processors cannot complete a new lease", async () => {
  const { sql, db } = database();
  try {
    const jobs = new DurableJobs(db);
    const now = Date.now();
    await jobs.enqueue("a", "email", "content", now);
    let release;
    let calls = 0;
    const blocked = new Promise((resolve) => {
      release = resolve;
    });
    const handler = async () => {
      calls++;
      await blocked;
    };
    const first = jobs.run({ email: handler }, 1, () => now);
    await new Promise((resolve) => setImmediate(resolve));
    await jobs.run({ email: handler }, 1, () => now);
    assert.equal(calls, 1);
    sql.exec("UPDATE sync_jobs SET lease_token = 'replacement'");
    release();
    await first;
    assert.equal(
      sql.prepare("SELECT completed_at FROM sync_jobs").get().completed_at,
      null,
    );
  } finally {
    sql.close();
  }
});

test("email jobs authenticate their encrypted payload and reject another key", async () => {
  const payload = { to: "test@example.com", url: "https://example.com/token" };
  const sealed = await sealJob("ab".repeat(32), payload);
  assert.ok(!sealed.includes("test@example.com"));
  assert.deepEqual(await openJob("ab".repeat(32), sealed), payload);
  await assert.rejects(openJob("cd".repeat(32), sealed));
});

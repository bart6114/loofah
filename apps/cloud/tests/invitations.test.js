import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";

import { run } from "../scripts/admin.mjs";
import { invitationStatus } from "../src/invitations.ts";
import { database } from "./database.js";

const config = {
  account_id: "test",
  env: {
    staging: {
      vars: { ACCOUNT_ORIGIN: "https://staging-app.loofah.io" },
      d1_databases: [{ binding: "DB", database_id: "test" }],
    },
  },
};

function fixture() {
  const { db, sql } = database();
  const api = async (path, body) => {
    assert.ok(path.endsWith("/query"), "creation must not send email");
    return [
      { success: true, results: sql.prepare(body.sql).all(...body.params) },
    ];
  };
  const issue = async (email = " Invitee@Example.COM ") => {
    const url = await run(["staging", "invite", email], api, config);
    const parsed = new URL(url);
    assert.equal(parsed.search, "");
    const token = new URLSearchParams(parsed.hash.slice(1)).get("invitation");
    assert.match(token, /^[a-f0-9]{64}$/);
    return token;
  };
  return { db, sql, api, issue };
}

test("administrator invitation lifecycle stores hashes, normalizes email and atomically revokes previous invitations", async () => {
  const { db, sql, api, issue } = fixture();
  try {
    const first = await issue();
    let row = sql.prepare("SELECT * FROM sync_invitations").get();
    assert.equal(row.id, createHash("sha256").update(first).digest("hex"));
    assert.equal(row.email, "invitee@example.com");
    assert.ok(Math.abs(row.expires_at - Date.now() - 7 * 86_400_000) < 1000);
    assert.equal(
      await invitationStatus(db, first, "invitee@example.com"),
      null,
    );
    assert.equal(
      await invitationStatus(db, first, "other@example.com"),
      "invitation_email",
    );
    assert.equal(
      await invitationStatus(db, first, row.email, row.expires_at),
      "invitation_expired",
    );
    const second = await issue();
    assert.equal(
      await invitationStatus(db, first, row.email),
      "invitation_invalid",
    );
    assert.equal(await invitationStatus(db, second, row.email), null);
    assert.equal(
      await invitationStatus(db, undefined, row.email),
      "invitation_invalid",
    );
    assert.equal(
      await invitationStatus(db, "f".repeat(64), row.email),
      "invitation_invalid",
    );
    row = sql
      .prepare("SELECT * FROM sync_invitations WHERE revoked_at IS NULL")
      .get();
    await run(["staging", "revoke", row.id], api, config);
    assert.equal(
      await invitationStatus(db, second, row.email),
      "invitation_invalid",
    );
    assert.throws(
      () =>
        sql
          .prepare(
            `INSERT INTO user (id,name,email,emailVerified,createdAt,updatedAt,invitationHash) VALUES ('blocked','Name',?,0,0,0,?)`,
          )
          .run(row.email, row.id),
      /invitation unavailable/,
    );
    await run(["staging", "capacity", "10"], api, config);
    assert.deepEqual(
      JSON.parse(
        JSON.stringify(await run(["staging", "capacity"], api, config)),
      ),
      [{ account_limit: 10, accounts: 0 }],
    );
    await assert.rejects(
      run(["production", "invite", row.email], api, config),
      /staging only/,
    );
  } finally {
    sql.close();
  }
});

test("capacity and consumed states preserve invitations and reissue never changes a consumed invitation", async () => {
  const { db, sql, api, issue } = fixture();
  try {
    const first = await issue();
    const hash = createHash("sha256").update(first).digest("hex");
    sql
      .prepare(
        `INSERT INTO user (id,name,email,emailVerified,createdAt,updatedAt,invitationHash) VALUES ('user','Name','invitee@example.com',0,0,0,?)`,
      )
      .run(hash);
    assert.equal(
      await invitationStatus(db, first, "invitee@example.com"),
      "invitation_consumed",
    );
    await issue();
    assert.equal(
      sql
        .prepare("SELECT revoked_at FROM sync_invitations WHERE id = ?")
        .get(hash).revoked_at,
      null,
    );
    const second = await issue("second@example.com");
    await run(["staging", "capacity", "1"], api, config);
    assert.equal(
      await invitationStatus(db, second, "second@example.com"),
      "capacity_reached",
    );
    await run(["staging", "capacity", "2"], api, config);
    assert.equal(
      await invitationStatus(db, second, "second@example.com"),
      null,
    );
  } finally {
    sql.close();
  }
});

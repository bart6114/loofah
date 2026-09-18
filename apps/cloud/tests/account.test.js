import assert from "node:assert/strict";
import { randomBytes, randomUUID } from "node:crypto";
import { test } from "node:test";

import worker from "../src/index.ts";
import { database } from "./database.js";

const origin = "https://account.example.com";

test("account identifies the signed-in user and names only active devices in the enrolled account", async () => {
  const { db, sql } = database();
  const env = {
    DB: db,
    AUTH_SECRET: "a".repeat(64),
    EMAIL_JOB_KEY: "b".repeat(64),
    ACCOUNT_ORIGIN: origin,
    SIGNUP_OPEN: "false",
  };
  const request = (path, token, body) =>
    worker.fetch(
      new Request(`${origin}/api${path}`, {
        method: body === undefined ? "GET" : "POST",
        headers: {
          authorization: `Bearer ${token}`,
          "content-type": "application/json",
        },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      }),
      env,
      { waitUntil() {} },
    );
  const seed = (email) => {
    const user = randomUUID(),
      vault = randomUUID(),
      device = randomUUID(),
      session = randomUUID(),
      token = randomUUID(),
      invite = randomBytes(32).toString("hex"),
      now = Date.now();
    sql
      .prepare(
        "INSERT INTO sync_invitations (id,email,expires_at) VALUES (?,?,?)",
      )
      .run(invite, email, now + 60_000);
    sql
      .prepare(
        "INSERT INTO user (id,name,email,emailVerified,createdAt,updatedAt,invitationHash) VALUES (?,'Test',?,1,?,?,?)",
      )
      .run(user, email, now, now, invite);
    sql
      .prepare(
        "INSERT INTO session (id,userId,token,expiresAt,createdAt,updatedAt,userAgent) VALUES (?,?,?,?,?,?,?)",
      )
      .run(session, user, token, now + 60_000, now, now, "Loofah");
    sql
      .prepare(
        "INSERT INTO sync_accounts (vault_id,user_id,recovery_generation) VALUES (?,?,?)",
      )
      .run(vault, user, randomUUID());
    sql
      .prepare(
        "INSERT INTO sync_devices (vault_id,id,public_key,enrolled_at) VALUES (?,?,?,?)",
      )
      .run(vault, device, "a".repeat(64), now);
    sql
      .prepare(
        "INSERT INTO sync_device_sessions (session_id,vault_id,device_id) VALUES (?,?,?)",
      )
      .run(session, vault, device);
    return { user, vault, device, session, token };
  };
  try {
    const own = seed("first@example.test"),
      other = seed("second@example.test");
    const account = await (await request("/account", own.token)).json();
    assert.equal(account.identity.email, "first@example.test");
    assert.equal(account.identity.session, own.session);
    assert.equal(
      (
        await request(`/devices/${other.device}/name`, own.token, {
          name: "Other",
        })
      ).status,
      404,
    );
    for (const name of ["", " ", "x".repeat(81), "A\nB", 123])
      assert.equal(
        (await request(`/devices/${own.device}/name`, own.token, { name }))
          .status,
        400,
      );
    assert.equal(
      (
        await request(`/devices/${own.device}/name`, own.token, {
          name: "  Test Mac  ",
        })
      ).status,
      200,
    );
    const devices = await (await request("/devices", own.token)).json();
    assert.equal(devices.devices.length, 1);
    assert.equal(devices.devices[0].name, "Test Mac");
    assert.equal(devices.devices[0].current, true);
    const signins = await (
      await request("/account/sign-ins", own.token)
    ).json();
    assert.equal(signins.length, 1);
    assert.equal(signins[0].current, true);
    assert.equal(signins[0].device.name, "Test Mac");
    sql
      .prepare("DELETE FROM sync_device_sessions WHERE session_id = ?")
      .run(own.session);
    assert.equal(
      (await request(`/devices/${own.device}/name`, own.token, { name: "No" }))
        .status,
      403,
    );
    assert.equal(
      (await (await request("/devices", own.token)).json()).devices[0].current,
      false,
    );
    assert.equal(
      (await request(`/devices/${own.device}/name`, "invalid", { name: "No" }))
        .status,
      401,
    );
    sql
      .prepare("UPDATE sync_devices SET revoked_at = ? WHERE id = ?")
      .run(Date.now(), own.device);
    assert.equal(
      (
        await request(`/devices/${own.device}/name`, other.token, {
          name: "No",
        })
      ).status,
      404,
    );
  } finally {
    sql.close();
  }
});

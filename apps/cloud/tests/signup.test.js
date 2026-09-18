import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { test } from "node:test";

import worker from "../src/index.ts";
import { sha256 } from "../src/jobs.ts";
import { database } from "./database.js";

test("signup route reports invitation failures, concurrent admission is bounded, and the kill switch preserves login", async () => {
  const { db, sql } = database();
  const originalFetch = globalThis.fetch;
  const tasks = [];
  const origin = "https://account.example.com";
  const env = {
    DB: db,
    AUTH_SECRET: randomBytes(32).toString("hex"),
    EMAIL_JOB_KEY: randomBytes(32).toString("hex"),
    ACCOUNT_ORIGIN: origin,
    SIGNUP_OPEN: "true",
    TURNSTILE_SECRET: "test",
    TURNSTILE_SITE_KEY: "test",
    EMAIL: { send: async () => {} },
  };
  globalThis.fetch = async (url) => {
    assert.equal(
      url,
      "https://challenges.cloudflare.com/turnstile/v0/siteverify",
    );
    return Response.json({
      success: true,
      hostname: "account.example.com",
      action: "account",
    });
  };
  const request = (path, email, token, extra = {}) =>
    worker.fetch(
      new Request(`${origin}/api/auth/${path}`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          origin,
          "cf-connecting-ip": "192.0.2.1",
          "x-turnstile-token": "test",
          ...(token ? { "x-loofah-invitation": token } : {}),
        },
        body: JSON.stringify({
          email,
          name: "Test",
          password: "a sufficiently long test password",
          ...extra,
        }),
      }),
      env,
      { waitUntil: (task) => tasks.push(task) },
    );
  const issue = async (email, expiry = Date.now() + 60_000) => {
    const token = randomBytes(32).toString("hex");
    sql
      .prepare(
        "INSERT INTO sync_invitations (id,email,expires_at) VALUES (?,?,?)",
      )
      .run(await sha256(token), email, expiry);
    return token;
  };
  try {
    assert.equal(
      (await (await request("sign-up/email", "a@example.com")).json()).error,
      "invitation_invalid",
    );
    const expired = await issue("a@example.com", 1);
    assert.equal(
      (await (await request("sign-up/email", "a@example.com", expired)).json())
        .error,
      "invitation_expired",
    );
    const token = await issue("a@example.com");
    assert.equal(
      (await (await request("sign-up/email", "b@example.com", token)).json())
        .error,
      "invitation_email",
    );
    const second = await issue("b@example.com");
    sql.exec("UPDATE sync_beta_settings SET account_limit = 1");
    await Promise.all([
      request("sign-up/email", "a@example.com", token),
      request("sign-up/email", "a@example.com", token),
      request("sign-up/email", "b@example.com", second),
    ]);
    const users = sql.prepare("SELECT * FROM user").all();
    assert.equal(users.length, 1);
    assert.equal(
      sql
        .prepare(
          "SELECT count(*) AS n FROM sync_invitations WHERE consumed_user IS NOT NULL",
        )
        .get().n,
      1,
    );
    const user = users[0];
    const used = user.email === "a@example.com" ? token : second;
    assert.equal(
      (await (await request("sign-up/email", user.email, used)).json()).error,
      "invitation_consumed",
    );
    const unused = user.email === "a@example.com" ? second : token;
    const other =
      user.email === "a@example.com" ? "b@example.com" : "a@example.com";
    assert.equal(
      (await (await request("sign-up/email", other, unused)).json()).error,
      "capacity_reached",
    );
    env.SIGNUP_OPEN = "false";
    assert.equal(
      (await (await request("sign-up/email", other, unused)).json()).error,
      "signup_closed",
    );
    sql.prepare("UPDATE user SET emailVerified = 1 WHERE id = ?").run(user.id);
    const login = await request("sign-in/email", user.email);
    assert.equal(login.status, 200, await login.clone().text());
    const tokenValue = (await login.json()).token;
    const account = await worker.fetch(
      new Request(`${origin}/api/account`, {
        headers: { Authorization: `Bearer ${tokenValue}` },
      }),
      env,
      { waitUntil: (task) => tasks.push(task) },
    );
    assert.equal(account.status, 200);
    const known = await request(
      "request-password-reset",
      user.email,
      undefined,
      { redirectTo: `${origin}/reset-password` },
    );
    const unknown = await request(
      "request-password-reset",
      "unknown@example.com",
      undefined,
      { redirectTo: `${origin}/reset-password` },
    );
    assert.equal(known.status, unknown.status);
    assert.deepEqual(await known.json(), await unknown.json());
    const verified = await request("send-verification-email", user.email);
    const missing = await request(
      "send-verification-email",
      "unknown@example.com",
    );
    assert.equal(verified.status, missing.status);
    assert.deepEqual(await verified.json(), await missing.json());
  } finally {
    await Promise.allSettled(tasks);
    globalThis.fetch = originalFetch;
    sql.close();
  }
});

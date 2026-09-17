import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { test } from "node:test";

import { activateAccount, createAuth } from "../src/auth.ts";
import worker from "../src/index.ts";
import { openJob, sha256 } from "../src/jobs.ts";
import { approveDevice } from "../web/api.ts";
import { database } from "./database.js";

test("native D1 auth requires an email-bound invitation, activates once, approves browser device login, and reset revokes sessions", async (t) => {
  const { sql, db } = database();
  try {
    const origin = "https://account.example.com";
    const env = {
      DB: db,
      AUTH_SECRET: randomBytes(32).toString("hex"),
      EMAIL_JOB_KEY: randomBytes(32).toString("hex"),
      ACCOUNT_ORIGIN: origin,
      SIGNUP_OPEN: "true",
    };
    const auth = createAuth(env);
    const token = randomBytes(32).toString("hex");
    sql
      .prepare(
        "INSERT INTO sync_invitations (id, email, expires_at) VALUES (?, ?, ?)",
      )
      .run(await sha256(token), "invitee@example.com", Date.now() + 60_000);
    const request = (path, body, headers = {}) =>
      auth.handler(
        new Request(`${origin}/api/auth${path}`, {
          method: "POST",
          headers: {
            "content-type": "application/json",
            origin,
            "cf-connecting-ip": "192.0.2.1",
            ...headers,
          },
          body: JSON.stringify(body),
        }),
      );
    const details = {
      email: "invitee@example.com",
      name: "Invitee",
      password: "a sufficiently long password",
    };
    await request("/sign-up/email", details);
    await request(
      "/sign-up/email",
      { ...details, email: "wrong@example.com" },
      { "x-loofah-invitation": token },
    );
    // Better Auth deliberately masks rejected signup responses to prevent account enumeration.
    assert.equal(sql.prepare("SELECT count(*) AS n FROM user").get().n, 0);
    assert.equal(sql.prepare("SELECT count(*) AS n FROM sync_jobs").get().n, 0);
    const signup = await request("/sign-up/email", details, {
      "x-loofah-invitation": token,
    });
    assert.equal(signup.status, 200, await signup.clone().text());
    assert.equal(
      sql.prepare("SELECT count(*) AS n FROM sync_accounts").get().n,
      0,
    );
    const user = sql.prepare("SELECT * FROM user").get();
    assert.equal(
      sql.prepare("SELECT consumed_user FROM sync_invitations").get()
        .consumed_user,
      user.id,
    );
    const job = sql.prepare("SELECT payload FROM sync_jobs").get();
    const verification = await openJob(env.EMAIL_JOB_KEY, job.payload);
    assert.equal(verification.to, details.email);
    assert.equal(verification.kind, "verify");
    const verificationRequest = new URL(verification.url);
    verificationRequest.searchParams.set(
      "callbackURL",
      `${origin}/verify?next=device&user_code=ABCD1234`,
    );
    const verified = await worker.fetch(new Request(verificationRequest), env, {
      waitUntil() {},
    });
    assert.equal(verified.status, 302);
    assert.equal(
      verified.headers.get("location"),
      `${origin}/verify?next=device&user_code=ABCD1234`,
    );
    const receipt = verified.headers
      .getSetCookie()
      .filter((value) => !value.includes("Max-Age=0"))
      .map((value) => value.split(";")[0])
      .join("; ");
    const receiptResult = (cookie) =>
      worker.fetch(
        new Request(`${origin}/api/verification-result`, {
          headers: { cookie },
        }),
        env,
        { waitUntil() {} },
      );
    assert.deepEqual(await (await receiptResult(receipt)).json(), {
      verified: true,
    });
    assert.deepEqual(
      await (await receiptResult("loofah.verification=verified")).json(),
      { verified: false },
    );
    assert.deepEqual(await (await receiptResult("")).json(), {
      verified: false,
    });
    verificationRequest.searchParams.set("token", "invalid");
    const invalid = await worker.fetch(new Request(verificationRequest), env, {
      waitUntil() {},
    });
    assert.match(invalid.headers.get("location"), /error=verification_failed/);
    assert.ok(
      invalid.headers
        .getSetCookie()
        .every((value) => value.includes("Max-Age=0")),
    );
    await activateAccount(db, user.id);
    await activateAccount(db, user.id);
    const vault = sql.prepare("SELECT * FROM sync_accounts").get();
    assert.equal(vault.user_id, user.id);
    assert.equal(vault.quota_bytes, 25_000_000_000);
    assert.equal(
      sql.prepare("SELECT count(*) AS n FROM sync_accounts").get().n,
      1,
    );
    const login = await request("/sign-in/email", details);
    assert.equal(login.status, 200, await login.clone().text());
    assert.equal(sql.prepare("SELECT count(*) AS n FROM session").get().n, 1);
    const cookie = login.headers
      .getSetCookie()
      .map((value) => value.split(";")[0])
      .join("; ");
    assert.ok(cookie.includes("session_token="));
    const device = await request("/device/code", { client_id: "loofah-macos" });
    assert.equal(device.status, 200, await device.clone().text());
    const code = await device.json();
    const unclaimed = await request(
      "/device/approve",
      { userCode: code.user_code },
      { cookie },
    );
    assert.equal(unclaimed.status, 400);
    assert.match((await unclaimed.json()).error_description, /claim/i);
    const browserFetch = t.mock.method(globalThis, "fetch", (path, options) => {
      assert.equal(options.credentials, "same-origin");
      return auth.handler(
        new Request(`${origin}${path}`, {
          ...options,
          headers: { ...options.headers, cookie, origin },
        }),
      );
    });
    try {
      assert.deepEqual(await approveDevice(code.user_code), { success: true });
      const granted = await request("/device/token", {
        grant_type: "urn:ietf:params:oauth:grant-type:device_code",
        client_id: "loofah-macos",
        device_code: code.device_code,
      });
      assert.equal(granted.status, 200, await granted.clone().text());
      const access = await granted.json();
      assert.ok(access.access_token);
      const connected = await auth.api.getSession({
        headers: new Headers({
          authorization: `Bearer ${access.access_token}`,
        }),
      });
      assert.equal(connected.user.id, user.id);
      await assert.rejects(approveDevice(code.user_code));
      await assert.rejects(approveDevice("INVALID-CODE"), /invalid/i);
      const expiring = await request("/device/code", {
        client_id: "loofah-macos",
      });
      const expired = await expiring.json();
      sql
        .prepare("UPDATE deviceCode SET expiresAt = 0 WHERE userCode = ?")
        .run(expired.user_code);
      await assert.rejects(
        approveDevice(expired.user_code),
        /expired.*connect again/,
      );
      sql
        .prepare(
          "UPDATE deviceCode SET expiresAt = ?, userId = ? WHERE userCode = ?",
        )
        .run(Date.now() + 60_000, "another-account", expired.user_code);
      await assert.rejects(approveDevice(expired.user_code), /another account/);
    } finally {
      browserFetch.mock.restore();
    }
    const reset = await request("/request-password-reset", {
      email: details.email,
      redirectTo: `${origin}/reset-password`,
    });
    assert.equal(reset.status, 200, await reset.clone().text());
    const messages = await Promise.all(
      sql
        .prepare("SELECT payload FROM sync_jobs")
        .all()
        .map((row) => openJob(env.EMAIL_JOB_KEY, row.payload)),
    );
    const resetMessage = messages.find((message) => message.kind === "reset");
    const resetToken = new URL(resetMessage.url).pathname.split("/").at(-1);
    const changed = await request("/reset-password", {
      token: resetToken,
      newPassword: "another sufficiently long password",
    });
    assert.equal(changed.status, 200, await changed.clone().text());
    assert.equal(sql.prepare("SELECT count(*) AS n FROM session").get().n, 0);
    assert.deepEqual(sql.prepare("SELECT * FROM sync_accounts").get(), vault);
  } finally {
    sql.close();
  }
});

test("database invitation consumption and beta admission are atomic even without application hooks", async () => {
  const { sql } = database();
  try {
    const insert =
      sql.prepare(`INSERT INTO user (id, name, email, emailVerified, createdAt, updatedAt, invitationHash)
      VALUES (?, 'Name', ?, 0, 0, 0, ?)`);
    const invitation = sql.prepare(
      "INSERT INTO sync_invitations (id, email, expires_at) VALUES (?, ?, ?)",
    );
    invitation.run("a".repeat(64), "a@example.com", Date.now() + 60_000);
    invitation.run("b".repeat(64), "b@example.com", Date.now() + 60_000);
    assert.throws(
      () => insert.run("a", "b@example.com", "a".repeat(64)),
      /invitation unavailable/,
    );
    sql.exec("UPDATE sync_beta_settings SET account_limit = 1");
    insert.run("a", "a@example.com", "a".repeat(64));
    assert.throws(
      () => insert.run("retry", "a@example.com", "a".repeat(64)),
      /invitation unavailable/,
    );
    assert.throws(
      () => insert.run("b", "b@example.com", "b".repeat(64)),
      /beta capacity/,
    );
    assert.equal(
      sql
        .prepare(
          "SELECT consumed_user FROM sync_invitations WHERE email = 'b@example.com'",
        )
        .get().consumed_user,
      null,
    );
  } finally {
    sql.close();
  }
});

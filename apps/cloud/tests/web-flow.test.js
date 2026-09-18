import assert from "node:assert/strict";
import { test } from "node:test";

import { api, ApiError } from "../web/api.ts";
import {
  clearFlow,
  continuationUrl,
  deviceCode,
  readFlow,
  signInLabel,
} from "../web/flow.ts";

test("auth continuation only carries a bounded user code and temporary invitation state survives refresh", () => {
  assert.equal(
    continuationUrl("/verify", "ABCD1234"),
    "/verify?next=device&user_code=ABCD1234",
  );
  for (const code of ["https://evil.test", "a".repeat(100), "../../", "", null])
    assert.equal(deviceCode(code), "");
  const values = new Map();
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  };
  assert.equal(
    readFlow(storage, "invite", "private-fragment-token"),
    "private-fragment-token",
  );
  assert.equal(readFlow(storage, "invite"), "private-fragment-token");
  clearFlow(storage, "invite");
  assert.equal(readFlow(storage, "invite"), "");
  storage.setItem("invite", JSON.stringify({ value: "expired", expires: 1 }));
  assert.equal(readFlow(storage, "invite"), "");
  storage.setItem("invite", "broken");
  assert.equal(readFlow(storage, "invite"), "");
});

test("browser labels do not confuse Mac browsers with connected Loofah devices", () => {
  assert.equal(
    signInLabel("Mozilla Mac Chrome/100 Safari/500", null),
    "Chrome on Mac",
  );
  assert.equal(
    signInLabel(undefined, { name: "Work Mac" }),
    "Loofah · Work Mac",
  );
  assert.equal(signInLabel(undefined, null), "App or unrecognized browser");
});

test("network, non-JSON service errors, and authentication have distinct safe errors", async (t) => {
  const mock = t.mock.method(globalThis, "fetch", async () => {
    throw new TypeError("Failed to fetch");
  });
  await assert.rejects(
    api("/account"),
    (error) =>
      error instanceof ApiError &&
      error.status === 0 &&
      /connection/.test(error.message),
  );
  mock.mock.mockImplementation(
    async () => new Response("worker error", { status: 503 }),
  );
  await assert.rejects(
    api("/account"),
    (error) => error instanceof ApiError && error.status === 503,
  );
  mock.mock.mockImplementation(async () =>
    Response.json({ error: "unauthorized" }, { status: 401 }),
  );
  await assert.rejects(
    api("/account"),
    (error) =>
      error instanceof ApiError &&
      error.status === 401 &&
      error.message === "Sign in to continue.",
  );
});

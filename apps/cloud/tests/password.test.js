import { hashPassword, verifyPassword } from "better-auth/crypto";
import assert from "node:assert/strict";
import { test } from "node:test";

import { password } from "../src/password.ts";

test("constant-time verifier preserves the pinned Better Auth scrypt format and normalization", async () => {
  const hash = await hashPassword("café test password");
  assert.equal(
    await password.verify({ hash, password: "cafe\u0301 test password" }),
    true,
  );
  assert.equal(
    await password.verify({ hash, password: "wrong password" }),
    false,
  );
  const ownHash = await password.hash("another test password");
  assert.equal(
    await verifyPassword({ hash: ownHash, password: "another test password" }),
    true,
  );
  for (const hash of [
    "",
    "broken",
    "0:1",
    `${"a".repeat(32)}:${"b".repeat(126)}`,
    `${"a".repeat(32)}:${"b".repeat(128)}:extra`,
  ]) {
    assert.equal(
      await password.verify({ hash, password: "test password" }),
      false,
    );
  }
});

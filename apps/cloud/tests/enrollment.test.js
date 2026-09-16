import assert from "node:assert/strict";
import { randomUUID, createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import {
  boundDevice,
  createChallenge,
  enrollmentPayload,
  finishEnrollment,
} from "../src/enrollment.ts";
import { database } from "./database.js";

const hex = (bytes) => Buffer.from(bytes).toString("hex");
test("canonical enrollment payload matches the Rust signing vector", async () => {
  const vector = JSON.parse(
    readFileSync(new URL("./enrollment-vector.json", import.meta.url)),
  );
  const payload = enrollmentPayload(vector.challenge);
  assert.equal(
    createHash("sha256").update(payload).digest("hex"),
    vector.payload_sha256,
  );
  const key = await crypto.subtle.importKey(
    "raw",
    Buffer.from(vector.challenge.public_key, "hex"),
    "Ed25519",
    false,
    ["verify"],
  );
  assert.equal(
    await crypto.subtle.verify(
      "Ed25519",
      key,
      Buffer.from(vector.signature, "hex"),
      payload,
    ),
    true,
  );
});

async function key() {
  const pair = await crypto.subtle.generateKey("Ed25519", true, [
    "sign",
    "verify",
  ]);
  return {
    public: hex(await crypto.subtle.exportKey("raw", pair.publicKey)),
    sign: async (bytes) =>
      hex(await crypto.subtle.sign("Ed25519", pair.privateKey, bytes)),
  };
}
function account(sql) {
  const user = randomUUID();
  const vault = randomUUID();
  const session = randomUUID();
  const invite = user.replaceAll("-", "").repeat(2);
  sql
    .prepare(
      "INSERT INTO sync_invitations (id,email,expires_at) VALUES (?,?,?)",
    )
    .run(invite, `${user}@example.com`, Date.now() + 60_000);
  sql
    .prepare(
      "INSERT INTO user (id,name,email,emailVerified,createdAt,updatedAt,invitationHash) VALUES (?,'Name',?,1,0,0,?)",
    )
    .run(user, `${user}@example.com`, invite);
  sql
    .prepare(
      "INSERT INTO session (id,token,userId,expiresAt,createdAt,updatedAt) VALUES (?,?,?,?,0,0)",
    )
    .run(session, randomUUID(), user, Date.now() + 3_600_000);
  sql
    .prepare(
      "INSERT INTO sync_accounts (vault_id,user_id,recovery_generation) VALUES (?,?,?)",
    )
    .run(vault, user, randomUUID());
  return { user, vault, session };
}

test("enrollment requires both exact identity and authority proof, binds the session, and rejects revocation and cross-account access", async () => {
  const { sql, db } = database();
  try {
    const owner = account(sql);
    const other = account(sql);
    const authority = await key();
    const device = await key();
    const input = {
      kind: "enroll",
      device: randomUUID(),
      publicKey: device.public,
      authority: authority.public,
    };
    const challenge = await createChallenge(
      db,
      owner.user,
      owner.session,
      input,
    );
    const payload = enrollmentPayload(challenge);
    const proof = {
      challenge: challenge.id,
      deviceSignature: await device.sign(payload),
      authoritySignature: await authority.sign(payload),
    };
    await assert.rejects(
      finishEnrollment(db, other.user, other.session, proof),
    );
    await assert.rejects(
      finishEnrollment(db, owner.user, owner.session, {
        ...proof,
        authoritySignature: proof.deviceSignature,
      }),
    );
    await assert.rejects(
      finishEnrollment(db, owner.user, owner.session, {
        ...proof,
        deviceSignature: await device.sign(
          enrollmentPayload({ ...challenge, device_id: randomUUID() }),
        ),
      }),
    );
    assert.equal(await boundDevice(db, owner.user, owner.session), null);
    const enrolled = await finishEnrollment(
      db,
      owner.user,
      owner.session,
      proof,
    );
    assert.equal(enrolled.device, input.device);
    assert.deepEqual(
      await finishEnrollment(db, owner.user, owner.session, proof),
      enrolled,
    );
    assert.equal(await boundDevice(db, other.user, owner.session), null);
    const attacker = await key();
    await assert.rejects(
      createChallenge(db, owner.user, owner.session, {
        ...input,
        device: randomUUID(),
        authority: attacker.public,
      }),
    );
    sql
      .prepare("UPDATE sync_devices SET revoked_at = ? WHERE id = ?")
      .run(Date.now(), input.device);
    assert.equal(await boundDevice(db, owner.user, owner.session), null);
    await assert.rejects(
      finishEnrollment(db, owner.user, owner.session, proof),
    );
    await assert.rejects(
      createChallenge(db, owner.user, owner.session, {
        ...input,
        kind: "bind",
      }),
    );
    assert.throws(() =>
      sql
        .prepare(
          "UPDATE sync_accounts SET enrollment_authority = ? WHERE vault_id = ?",
        )
        .run(attacker.public, owner.vault),
    );
  } finally {
    sql.close();
  }
});

test("binding a new session needs an enrolled device key and a challenge fails across recovery generations", async () => {
  const { sql, db } = database();
  try {
    const owner = account(sql);
    const authority = await key();
    const device = await key();
    const input = {
      kind: "enroll",
      device: randomUUID(),
      publicKey: device.public,
      authority: authority.public,
    };
    const challenge = await createChallenge(
      db,
      owner.user,
      owner.session,
      input,
    );
    const payload = enrollmentPayload(challenge);
    await finishEnrollment(db, owner.user, owner.session, {
      challenge: challenge.id,
      deviceSignature: await device.sign(payload),
      authoritySignature: await authority.sign(payload),
    });
    const second = randomUUID();
    sql
      .prepare(
        "INSERT INTO session (id,token,userId,expiresAt,createdAt,updatedAt) VALUES (?,?,?,?,0,0)",
      )
      .run(second, randomUUID(), owner.user, Date.now() + 3_600_000);
    assert.equal(await boundDevice(db, owner.user, second), null);
    const binding = await createChallenge(db, owner.user, second, {
      ...input,
      kind: "bind",
    });
    await finishEnrollment(db, owner.user, second, {
      challenge: binding.id,
      deviceSignature: await device.sign(enrollmentPayload(binding)),
    });
    assert.equal(
      (await boundDevice(db, owner.user, second)).device,
      input.device,
    );
    const obsolete = await createChallenge(db, owner.user, second, {
      ...input,
      kind: "bind",
    });
    sql
      .prepare(
        "UPDATE sync_accounts SET recovery_generation = ? WHERE vault_id = ?",
      )
      .run(randomUUID(), owner.vault);
    await assert.rejects(
      finishEnrollment(db, owner.user, second, {
        challenge: obsolete.id,
        deviceSignature: await device.sign(enrollmentPayload(obsolete)),
      }),
    );
    assert.equal(
      sql
        .prepare(
          "SELECT consumed_at FROM sync_enrollment_challenges WHERE id = ?",
        )
        .get(obsolete.id).consumed_at,
      null,
    );
  } finally {
    sql.close();
  }
});

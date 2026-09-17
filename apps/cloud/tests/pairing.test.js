import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { test } from "node:test";

import { PairingMailbox } from "../src/pairing.ts";
import { database } from "./database.js";

test("mailboxes require distinct verified sessions, active approvers, immutable phases, and bounded expiry", async () => {
  const { sql, db } = database();
  const data = new Map();
  const sockets = [];
  let alarm;
  const storage = {
    list: async ({ prefix }) =>
      new Map([...data].filter(([key]) => key.startsWith(prefix))),
    get: async (key) => data.get(key),
    put: async (key, value) => data.set(key, structuredClone(value)),
    delete: async (key) => data.delete(key),
    setAlarm: async (time) => {
      alarm = time;
    },
  };
  const mailbox = new PairingMailbox(
    {
      storage,
      getWebSockets: () => sockets,
      blockConcurrencyWhile: (work) => work(),
    },
    { DB: db },
  );
  try {
    const user = randomUUID(),
      vault = randomUUID(),
      creator = randomUUID(),
      approver = randomUUID(),
      device = randomUUID();
    const invitation = "a".repeat(64);
    sql
      .prepare(
        "INSERT INTO sync_invitations (id,email,expires_at) VALUES (?,?,?)",
      )
      .run(invitation, "pairing@example.com", Date.now() + 60_000);
    sql
      .prepare(
        "INSERT INTO user (id,name,email,emailVerified,createdAt,updatedAt,invitationHash) VALUES (?,'Pairing','pairing@example.com',1,0,0,?)",
      )
      .run(user, invitation);
    sql
      .prepare(
        "INSERT INTO sync_accounts (vault_id,user_id,recovery_generation) VALUES (?,?,?)",
      )
      .run(vault, user, randomUUID());
    for (const session of [creator, approver])
      sql
        .prepare(
          "INSERT INTO session (id,token,userId,expiresAt,createdAt,updatedAt) VALUES (?,?,?,?,0,0)",
        )
        .run(session, randomUUID(), user, Date.now() + 600_000);
    sql
      .prepare(
        "INSERT INTO sync_devices (vault_id,id,public_key,enrolled_at) VALUES (?,?,?,?)",
      )
      .run(vault, device, "b".repeat(64), Date.now());
    sql
      .prepare(
        "INSERT INTO sync_device_sessions (session_id,vault_id,device_id) VALUES (?,?,?)",
      )
      .run(approver, vault, device);
    const socket = (session, side) => {
      let attachment = { user, session, side };
      const values = [];
      const value = {
        deserializeAttachment: () => structuredClone(attachment),
        serializeAttachment: (value) => {
          attachment = structuredClone(value);
        },
        send: (value) => values.push(JSON.parse(value)),
        close: () => {
          value.closed = true;
        },
        values,
      };
      sockets.push(value);
      return value;
    };
    const send = (socket, value) =>
      mailbox.webSocketMessage(socket, JSON.stringify({ id: "id", ...value }));
    const first = socket(creator, "aaaaaa0000"),
      second = socket(approver, "bbbbbb0000");
    await send(first, { type: "allocate" });
    const name = first.values.find(
      (value) => value.type === "allocated",
    ).nameplate;
    await send(first, { type: "claim", nameplate: name });
    await send(second, { type: "claim", nameplate: name });
    await send(first, { type: "add", phase: "pake", body: "abcd" });
    assert.equal(second.values.at(-1).body, "abcd");
    await send(first, { type: "add", phase: "pake", body: "aabb" });
    assert.equal(first.closed, true);
    sql
      .prepare("UPDATE sync_devices SET revoked_at = ? WHERE id = ?")
      .run(Date.now(), device);
    await send(second, { type: "add", phase: "version", body: "abcd" });
    assert.equal(second.closed, true);
    for (const [key, box] of data)
      if (key.startsWith("box:")) box.expires = Date.now() - 1;
    await mailbox.alarm();
    assert.equal(
      [...data.keys()].filter((key) => key.startsWith("box:")).length,
      0,
    );
    data.set("attempts", { start: Date.now(), count: 10 });
    const limited = socket(creator, "cccccc0000");
    await send(limited, { type: "allocate" });
    assert.equal(limited.closed, true);
    assert.ok(alarm <= Date.now() + 600_000);
  } finally {
    sql.close();
  }
});

import type { VaultPrincipal } from "./storage.ts";

export type EnrollmentChallenge = {
  id: string;
  vault_id: string;
  session_id: string;
  user_id: string;
  kind: "enroll" | "bind";
  device_id: string;
  public_key: string;
  authority: string;
  generation: string;
  nonce: string;
  expires_at: number;
  consumed_at: number | null;
};

export class EnrollmentError extends Error {}

export function enrollmentPayload(challenge: EnrollmentChallenge) {
  return new TextEncoder().encode(
    JSON.stringify([
      "loofah-enrollment-v1",
      challenge.kind,
      challenge.vault_id,
      challenge.user_id,
      challenge.session_id,
      challenge.id,
      challenge.nonce,
      challenge.device_id,
      challenge.public_key,
      challenge.authority,
      challenge.generation,
      String(challenge.expires_at),
    ]),
  );
}

export async function createChallenge(
  db: D1Database,
  user: string,
  session: string,
  input: {
    kind: "enroll" | "bind";
    device: string;
    publicKey: string;
    authority: string;
  },
  now = Date.now(),
) {
  if (
    !["enroll", "bind"].includes(input.kind) ||
    !/^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(input.device) ||
    !/^[a-f0-9]{64}$/.test(input.publicKey) ||
    !/^[a-f0-9]{64}$/.test(input.authority)
  )
    throw new EnrollmentError("invalid enrollment");
  const account = await db
    .prepare(`SELECT a.vault_id, a.enrollment_authority, a.recovery_generation FROM sync_accounts a
    JOIN session s ON s.userId = a.user_id JOIN user u ON u.id = a.user_id
    WHERE a.user_id = ? AND s.id = ? AND u.emailVerified = 1 AND a.active = 1`)
    .bind(user, session)
    .first<{
      vault_id: string;
      enrollment_authority: string | null;
      recovery_generation: string;
    }>();
  if (
    !account ||
    (account.enrollment_authority &&
      account.enrollment_authority !== input.authority)
  )
    throw new EnrollmentError("authority mismatch");
  if (input.kind === "bind") {
    const device = await db
      .prepare(
        `SELECT id FROM sync_devices WHERE vault_id = ? AND id = ? AND public_key = ? AND revoked_at IS NULL`,
      )
      .bind(account.vault_id, input.device, input.publicKey)
      .first();
    if (!device || !account.enrollment_authority)
      throw new EnrollmentError("device unavailable");
  }
  const nonce = Array.from(crypto.getRandomValues(new Uint8Array(32)), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  const challenge: EnrollmentChallenge = {
    id: crypto.randomUUID(),
    vault_id: account.vault_id,
    session_id: session,
    user_id: user,
    kind: input.kind,
    device_id: input.device,
    public_key: input.publicKey,
    authority: input.authority,
    generation: account.recovery_generation,
    nonce,
    expires_at: now + 300_000,
    consumed_at: null,
  };
  await db
    .prepare(`INSERT INTO sync_enrollment_challenges
    (id, vault_id, session_id, user_id, kind, device_id, public_key, authority, generation, nonce, expires_at)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`)
    .bind(
      challenge.id,
      challenge.vault_id,
      session,
      user,
      challenge.kind,
      challenge.device_id,
      challenge.public_key,
      challenge.authority,
      challenge.generation,
      nonce,
      challenge.expires_at,
    )
    .run();
  return challenge;
}

export async function finishEnrollment(
  db: D1Database,
  user: string,
  session: string,
  input: {
    challenge: string;
    deviceSignature: string;
    authoritySignature?: string;
  },
  now = Date.now(),
) {
  const challenge = await db
    .prepare(
      `SELECT * FROM sync_enrollment_challenges WHERE id = ? AND user_id = ? AND session_id = ?`,
    )
    .bind(input.challenge, user, session)
    .first<EnrollmentChallenge>();
  if (!challenge || challenge.expires_at <= now)
    throw new EnrollmentError("challenge unavailable");
  const payload = enrollmentPayload(challenge);
  if (
    !(await verify(challenge.public_key, input.deviceSignature, payload)) ||
    (challenge.kind === "enroll" &&
      !(await verify(
        challenge.authority,
        input.authoritySignature ?? "",
        payload,
      )))
  ) {
    throw new EnrollmentError("invalid proof");
  }
  if (challenge.consumed_at !== null) {
    const principal = await boundDevice(db, user, session);
    if (
      principal?.device === challenge.device_id &&
      principal.generation === challenge.generation
    )
      return principal;
    throw new EnrollmentError("challenge consumed");
  }
  try {
    await db
      .prepare(
        "INSERT INTO sync_enrollment_receipts (challenge, created_at) VALUES (?, ?)",
      )
      .bind(challenge.id, now)
      .run();
  } catch {
    throw new EnrollmentError("enrollment unavailable");
  }
  return boundDevice(db, user, session);
}

export async function boundDevice(
  db: D1Database,
  user: string,
  session: string,
): Promise<VaultPrincipal | null> {
  return db
    .prepare(`SELECT a.vault_id AS vault, d.id AS device, a.recovery_generation AS generation
    FROM sync_device_sessions b JOIN sync_devices d ON d.vault_id = b.vault_id AND d.id = b.device_id
    JOIN sync_accounts a ON a.vault_id = b.vault_id JOIN session s ON s.id = b.session_id JOIN user u ON u.id = s.userId
    WHERE b.session_id = ? AND a.user_id = ? AND s.userId = a.user_id AND u.emailVerified = 1
      AND a.active = 1 AND d.revoked_at IS NULL`)
    .bind(session, user)
    .first<VaultPrincipal>();
}

async function verify(
  publicKey: string,
  signature: string,
  payload: Uint8Array,
) {
  if (!/^[a-f0-9]{128}$/.test(signature)) return false;
  const bytes = (hex: string) =>
    new Uint8Array(
      hex.match(/../g)!.map((value) => Number.parseInt(value, 16)),
    );
  try {
    const key = await crypto.subtle.importKey(
      "raw",
      bytes(publicKey),
      "Ed25519",
      false,
      ["verify"],
    );
    return await crypto.subtle.verify(
      "Ed25519",
      key,
      bytes(signature),
      new Uint8Array(payload),
    );
  } catch {
    return false;
  }
}

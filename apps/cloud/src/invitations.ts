import { sha256 } from "./jobs.ts";

export async function invitationStatus(
  db: D1Database,
  token: string | null | undefined,
  email: unknown,
  now = Date.now(),
) {
  if (!token || !/^[a-f0-9]{64}$/.test(token)) return "invitation_invalid";
  const invitation = await db
    .prepare(
      "SELECT email, expires_at, consumed_user, revoked_at FROM sync_invitations WHERE id = ?",
    )
    .bind(await sha256(token))
    .first<{
      email: string;
      expires_at: number;
      consumed_user: string | null;
      revoked_at: number | null;
    }>();
  if (!invitation || invitation.revoked_at !== null)
    return "invitation_invalid";
  if (
    typeof email !== "string" ||
    invitation.email !== email.trim().toLowerCase()
  )
    return "invitation_email";
  if (invitation.consumed_user !== null) return "invitation_consumed";
  if (invitation.expires_at <= now) return "invitation_expired";
  const capacity = await db
    .prepare(
      "SELECT account_limit, (SELECT count(*) FROM user) AS accounts FROM sync_beta_settings WHERE id = 1",
    )
    .first<{ account_limit: number; accounts: number }>();
  if (!capacity || capacity.accounts >= capacity.account_limit)
    return "capacity_reached";
  return null;
}

import { betterAuth, type BetterAuthOptions } from "better-auth";
import { APIError } from "better-auth/api";
import { bearer, deviceAuthorization } from "better-auth/plugins";

import { invitationStatus } from "./invitations.ts";
import { DurableJobs, sealJob, sha256 } from "./jobs.ts";
import { password } from "./password.ts";

export type AuthEnvironment = {
  DB: D1Database;
  AUTH_SECRET: string;
  EMAIL_JOB_KEY: string;
  ACCOUNT_ORIGIN: string;
  SIGNUP_OPEN: string;
};

export function authOptions(env: AuthEnvironment) {
  const origin = new URL(env.ACCOUNT_ORIGIN);
  if (
    origin.protocol !== "https:" ||
    origin.origin !== env.ACCOUNT_ORIGIN ||
    env.AUTH_SECRET.length < 32
  ) {
    throw new Error("invalid auth configuration");
  }
  const jobs = new DurableJobs(env.DB);
  const email = async (
    kind: "verify" | "reset",
    user: { email: string },
    url: string,
  ) => {
    if (new URL(url).origin !== origin.origin)
      throw new Error("invalid email origin");
    await jobs.enqueue(
      await sha256(`${kind}\n${url}`),
      "account_email",
      await sealJob(env.EMAIL_JOB_KEY, { kind, to: user.email, url }),
    );
  };
  return {
    appName: "Loofah",
    database: env.DB,
    secret: env.AUTH_SECRET,
    baseURL: env.ACCOUNT_ORIGIN,
    trustedOrigins: [env.ACCOUNT_ORIGIN],
    logger: { disabled: true },
    advanced: {
      useSecureCookies: true,
      cookiePrefix: "loofah",
      ipAddress: { ipAddressHeaders: ["cf-connecting-ip"] },
      defaultCookieAttributes: {
        sameSite: "lax",
        httpOnly: true,
        secure: true,
      },
    },
    session: { cookieCache: { enabled: false } },
    rateLimit: { enabled: true, storage: "database", window: 60, max: 60 },
    emailAndPassword: {
      enabled: true,
      disableSignUp: env.SIGNUP_OPEN !== "true",
      requireEmailVerification: true,
      minPasswordLength: 12,
      password,
      revokeSessionsOnPasswordReset: true,
      sendResetPassword: async ({ user, url }) => email("reset", user, url),
    },
    emailVerification: {
      sendOnSignUp: true,
      sendOnSignIn: false,
      autoSignInAfterVerification: false,
      sendVerificationEmail: async ({ user, url }) =>
        email("verify", user, url),
      afterEmailVerification: async (user) => {
        await activateAccount(env.DB, user.id);
      },
    },
    user: {
      additionalFields: {
        invitationHash: {
          type: "string",
          required: false,
          input: false,
          returned: false,
        },
      },
    },
    databaseHooks: {
      user: {
        create: {
          before: async (user, context) => {
            const token = context?.headers?.get("x-loofah-invitation");
            if (
              !token ||
              !/^[a-f0-9]{64}$/.test(token) ||
              env.SIGNUP_OPEN !== "true"
            ) {
              throw new APIError("FORBIDDEN", {
                message: "A valid invitation is required.",
              });
            }
            const hash = await sha256(token);
            const status = await invitationStatus(env.DB, token, user.email);
            if (status)
              throw new APIError("FORBIDDEN", {
                message: "A valid invitation is required.",
              });
            return { data: { ...user, invitationHash: hash } };
          },
        },
      },
    },
    plugins: [
      bearer(),
      deviceAuthorization({
        validateClient: (client) =>
          client === "loofah-macos" || client === "loofah-cli",
        verificationUri: `${env.ACCOUNT_ORIGIN}/device`,
        expiresIn: "10m",
        interval: "5s",
      }),
    ],
  } satisfies BetterAuthOptions;
}

export function createAuth(env: AuthEnvironment) {
  return betterAuth(authOptions(env));
}

export async function activateAccount(db: D1Database, user: string) {
  await db
    .prepare(`INSERT INTO sync_accounts (vault_id, user_id, recovery_generation)
    SELECT ?, id, ? FROM user WHERE id = ? AND emailVerified = 1
    ON CONFLICT (user_id) DO NOTHING`)
    .bind(crypto.randomUUID(), crypto.randomUUID(), user)
    .run();
}

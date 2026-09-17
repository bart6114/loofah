import { Hono } from "hono";
import { bodyLimit } from "hono/body-limit";
import { deleteCookie, getSignedCookie, setSignedCookie } from "hono/cookie";
import { secureHeaders } from "hono/secure-headers";

import { activateAccount, createAuth, type AuthEnvironment } from "./auth.ts";
import {
  boundDevice,
  createChallenge,
  approvePairing,
  EnrollmentError,
  finishEnrollment,
} from "./enrollment.ts";
import { invitationStatus } from "./invitations.ts";
import { DurableJobs, openJob, sha256 } from "./jobs.ts";
import { ObjectStorage } from "./objects.ts";
import {
  StorageError,
  expireHistory,
  VaultSnapshots,
  VaultStorage,
  type VaultPrincipal,
} from "./storage.ts";

export { PairingMailbox } from "./pairing.ts";

export type Environment = AuthEnvironment & {
  PAIRING: DurableObjectNamespace;
  TURNSTILE_SECRET: string;
  EMAIL: SendEmail;
  VAULT: R2Bucket;
  RECOVERY: R2Bucket;
  TURNSTILE_SITE_KEY: string;
  ASSETS: Fetcher;
};

const app = new Hono<{
  Bindings: Environment;
  Variables: { principal: VaultPrincipal };
}>();
// WebSocket upgrade responses have immutable headers in workerd.
const isPairingUpgrade = (path: string, upgrade: string | undefined) =>
  path === "/api/pairing/mailbox" && upgrade?.toLowerCase() === "websocket";
app.use("*", (context, next) =>
  isPairingUpgrade(context.req.path, context.req.header("upgrade"))
    ? next()
    : secureHeaders({ referrerPolicy: "no-referrer" })(context, next),
);
app.use("/api/*", async (context, next) =>
  bodyLimit({
    maxSize: context.req.path.startsWith("/api/sync/")
      ? 8 * 1024 * 1024
      : 64 * 1024,
  })(context, next),
);
app.use("/api/*", async (context, next) => {
  if (!isPairingUpgrade(context.req.path, context.req.header("upgrade")))
    context.header("Cache-Control", "no-store");
  if (context.req.path !== "/api/health") {
    const settings = await context.env.DB.prepare(
      "SELECT writes_enabled FROM sync_beta_settings WHERE id = 1",
    ).first<{ writes_enabled: number }>();
    if (settings?.writes_enabled !== 1)
      return context.json({ error: "maintenance" }, 503);
  }
  await next();
});
app.get("/api/health", (context) =>
  context.json({ service: "loofah-sync", protocol: 1 }),
);
app.get("/api/config", (context) =>
  context.json({
    sitekey: context.env.TURNSTILE_SITE_KEY,
    signupOpen: context.env.SIGNUP_OPEN === "true",
  }),
);

const protectedFlows = new Set([
  "/sign-up/email",
  "/sign-in/email",
  "/request-password-reset",
  "/send-verification-email",
]);
app.on(["GET", "POST"], "/api/auth/*", async (context) => {
  const path = context.req.path.slice("/api/auth".length);
  if (context.req.method === "POST" && protectedFlows.has(path)) {
    const ip = context.req.header("cf-connecting-ip");
    if (!ip || context.req.header("origin") !== context.env.ACCOUNT_ORIGIN)
      return context.json({ error: "forbidden" }, 403);
    const now = Date.now();
    const key = await sha256(
      `${context.env.AUTH_SECRET}\n${ip}\n${Math.floor(now / 60_000)}`,
    );
    const rate =
      await context.env.DB.prepare(`INSERT INTO sync_request_limits (key, count, expires_at) VALUES (?, 1, ?)
      ON CONFLICT (key) DO UPDATE SET count = count + 1 RETURNING count`)
        .bind(key, now + 120_000)
        .first<{ count: number }>();
    if (!rate || rate.count > 20)
      return context.json({ error: "rate_limited" }, 429);
    const token = context.req.header("x-turnstile-token");
    if (!token || token.length > 2048)
      return context.json({ error: "challenge_required" }, 403);
    const result = await fetch(
      "https://challenges.cloudflare.com/turnstile/v0/siteverify",
      {
        method: "POST",
        body: new URLSearchParams({
          secret: context.env.TURNSTILE_SECRET,
          response: token,
          remoteip: ip,
        }),
        signal: AbortSignal.timeout(10_000),
      },
    );
    const verification = (await result.json()) as {
      success?: boolean;
      hostname?: string;
      action?: string;
    };
    if (
      !result.ok ||
      !verification.success ||
      verification.hostname !== new URL(context.env.ACCOUNT_ORIGIN).hostname ||
      verification.action !== "account"
    )
      return context.json({ error: "challenge_failed" }, 403);
  }
  const signupInput: unknown =
    context.req.method === "POST" && path === "/sign-up/email"
      ? await context.req.raw
          .clone()
          .json()
          .catch(() => null)
      : null;
  const signupEmail =
    signupInput && typeof signupInput === "object" && "email" in signupInput
      ? signupInput.email
      : undefined;
  if (context.req.method === "POST" && path === "/sign-up/email") {
    if (context.env.SIGNUP_OPEN !== "true")
      return context.json({ error: "signup_closed" }, 403);
    const status = await invitationStatus(
      context.env.DB,
      context.req.header("x-loofah-invitation"),
      signupEmail,
    );
    if (status) return context.json({ error: status }, 403);
  }
  if (context.req.method === "GET" && path === "/verify-email") {
    deleteCookie(context, "loofah.verification", {
      path: "/api/verification-result",
    });
    const url = new URL(context.req.url);
    const callback = new URL(
      url.searchParams.get("callbackURL") || "/verify",
      context.env.ACCOUNT_ORIGIN,
    );
    if (
      callback.origin !== context.env.ACCOUNT_ORIGIN ||
      callback.pathname !== "/verify"
    )
      return context.json({ error: "forbidden" }, 403);
    url.searchParams.delete("callbackURL");
    const result = await createAuth(context.env).handler(
      new Request(url, context.req.raw),
    );
    const value = (await result.json().catch(() => null)) as {
      status?: boolean;
    } | null;
    if (result.ok && value?.status === true) {
      await setSignedCookie(
        context,
        "loofah.verification",
        String(Date.now()),
        context.env.AUTH_SECRET,
        {
          httpOnly: true,
          secure: true,
          sameSite: "Lax",
          path: "/api/verification-result",
          maxAge: 600,
        },
      );
    } else callback.searchParams.set("error", "verification_failed");
    return context.redirect(callback.toString());
  }
  const response = await createAuth(context.env).handler(context.req.raw);
  context.executionCtx.waitUntil(processJobs(context.env));
  if (context.req.method === "POST" && path === "/sign-up/email") {
    const status = await invitationStatus(
      context.env.DB,
      context.req.header("x-loofah-invitation"),
      signupEmail,
    );
    if (status === "capacity_reached")
      return context.json({ error: status }, 403);
  }
  return response;
});

app.get("/api/verification-result", async (context) => {
  const receipt = await getSignedCookie(
    context,
    context.env.AUTH_SECRET,
    "loofah.verification",
  );
  const issued = typeof receipt === "string" ? Number(receipt) : NaN;
  return context.json({
    verified:
      Number.isFinite(issued) &&
      issued <= Date.now() &&
      issued > Date.now() - 600_000,
  });
});

app.get("/api/account", async (context) => {
  const auth = createAuth(context.env);
  const session = await auth.api.getSession({
    headers: context.req.raw.headers,
  });
  if (!session?.user.emailVerified)
    return context.json({ error: "unauthorized" }, 401);
  await activateAccount(context.env.DB, session.user.id);
  const account =
    await context.env.DB.prepare(`SELECT vault_id, quota_bytes, used_bytes, recovery_generation, enrollment_authority,
      (SELECT COUNT(*) FROM sync_heads h JOIN sync_revisions r
        ON r.vault_id = h.vault_id AND r.id = h.revision
        WHERE h.vault_id = a.vault_id AND r.operation != 'delete') AS synced_items,
      (SELECT COUNT(*) FROM sync_devices d
        WHERE d.vault_id = a.vault_id AND d.revoked_at IS NULL) AS active_devices,
      (SELECT created_at FROM sync_changes c WHERE c.vault_id = a.vault_id
        ORDER BY sequence DESC LIMIT 1) AS last_change_at
    FROM sync_accounts a WHERE user_id = ? AND active = 1`)
      .bind(session.user.id)
      .first();
  if (!account) return context.json({ error: "unavailable" }, 503);
  return context.json({
    account,
    identity: {
      user: session.user.id,
      session: session.session.id,
      email: session.user.email,
    },
  });
});

app.get("/api/account/sign-ins", async (context) => {
  const auth = createAuth(context.env);
  const session = await auth.api.getSession({
    headers: context.req.raw.headers,
  });
  if (!session?.user.emailVerified)
    return context.json({ error: "unauthorized" }, 401);
  const sessions = await auth.api.listSessions({
    headers: context.req.raw.headers,
  });
  const bindings =
    await context.env.DB.prepare(`SELECT ds.session_id, d.id, d.name
    FROM sync_device_sessions ds JOIN sync_devices d ON ds.vault_id = d.vault_id AND ds.device_id = d.id
    JOIN sync_accounts a ON a.vault_id = d.vault_id WHERE a.user_id = ?`)
      .bind(session.user.id)
      .all();
  return context.json(
    sessions.map((item) => {
      const device = bindings.results.find(
        (binding) => binding.session_id === item.id,
      );
      return {
        ...item,
        current: item.id === session.session.id,
        device: device ? { id: device.id, name: device.name } : null,
      };
    }),
  );
});

app.post("/api/enrollment/:action", async (context) => {
  if (!context.req.header("authorization")?.startsWith("Bearer "))
    return context.json({ error: "unauthorized" }, 401);
  const session = await createAuth(context.env).api.getSession({
    headers: context.req.raw.headers,
  });
  if (!session?.user.emailVerified)
    return context.json({ error: "unauthorized" }, 401);
  await activateAccount(context.env.DB, session.user.id);
  const input = await context.req.json();
  if (context.req.param("action") === "challenge") {
    return context.json(
      await createChallenge(
        context.env.DB,
        session.user.id,
        session.session.id,
        input,
      ),
    );
  }
  if (context.req.param("action") === "approve-pairing") {
    return context.json(
      await approvePairing(
        context.env.DB,
        session.user.id,
        session.session.id,
        input,
      ),
    );
  }
  if (context.req.param("action") === "finish") {
    return context.json(
      await finishEnrollment(
        context.env.DB,
        session.user.id,
        session.session.id,
        input,
      ),
    );
  }
  return context.json({ error: "not_found" }, 404);
});

app.get("/api/pairing/mailbox", async (context) => {
  if (!context.req.header("authorization")?.startsWith("Bearer "))
    return context.json({ error: "unauthorized" }, 401);
  const session = await createAuth(context.env).api.getSession({
    headers: context.req.raw.headers,
  });
  if (!session?.user.emailVerified)
    return context.json({ error: "unauthorized" }, 401);
  try {
    const namespace = context.env.PAIRING.jurisdiction("eu");
    return await namespace.get(namespace.idFromName(session.user.id)).fetch(
      new Request("https://pairing/mailbox", {
        headers: {
          Upgrade: context.req.header("upgrade") ?? "",
          "x-loofah-user": session.user.id,
          "x-loofah-session": session.session.id,
        },
      }),
    );
  } catch (error) {
    console.error(
      "pairing mailbox unavailable",
      error instanceof Error ? error.message.slice(0, 300) : "unknown",
    );
    return context.json({ error: "unavailable" }, 503);
  }
});

app.get("/api/devices", async (context) => {
  const session = await createAuth(context.env).api.getSession({
    headers: context.req.raw.headers,
  });
  if (!session?.user.emailVerified)
    return context.json({ error: "unauthorized" }, 401);
  const devices =
    await context.env.DB.prepare(`SELECT d.id, d.name, d.public_key, d.enrolled_at, d.revoked_at,
      EXISTS (SELECT 1 FROM sync_device_sessions ds WHERE ds.session_id = ? AND ds.vault_id = d.vault_id AND ds.device_id = d.id) AS current
    FROM sync_devices d JOIN sync_accounts a ON a.vault_id = d.vault_id
    WHERE a.user_id = ? AND a.active = 1 ORDER BY d.enrolled_at`)
      .bind(session.session.id, session.user.id)
      .all();
  return context.json({
    devices: devices.results.map((device) => ({
      ...device,
      current: Boolean(device.current),
    })),
  });
});

app.post("/api/devices/:id/name", async (context) => {
  if (!context.req.header("authorization")?.startsWith("Bearer "))
    return context.json({ error: "unauthorized" }, 401);
  const session = await createAuth(context.env).api.getSession({
    headers: context.req.raw.headers,
  });
  if (!session?.user.emailVerified)
    return context.json({ error: "unauthorized" }, 401);
  const principal = await boundDevice(
    context.env.DB,
    session.user.id,
    session.session.id,
  );
  if (!principal) return context.json({ error: "device_required" }, 403);
  const input = await context.req.json().catch(() => null);
  const name = typeof input?.name === "string" ? input.name.trim() : "";
  if (!name || name.length > 80 || /[\u0000-\u001f\u007f]/.test(name))
    return context.json({ error: "invalid_device_name" }, 400);
  const device = await context.env.DB.prepare(
    "UPDATE sync_devices SET name = ? WHERE vault_id = ? AND id = ? AND revoked_at IS NULL RETURNING id",
  )
    .bind(name, principal.vault, context.req.param("id"))
    .first();
  if (!device) return context.json({ error: "not_found" }, 404);
  return context.json({ renamed: true });
});

app.post("/api/devices/:id/revoke", async (context) => {
  if (!context.req.header("authorization")?.startsWith("Bearer "))
    return context.json({ error: "unauthorized" }, 401);
  const session = await createAuth(context.env).api.getSession({
    headers: context.req.raw.headers,
  });
  if (!session?.user.emailVerified)
    return context.json({ error: "unauthorized" }, 401);
  const principal = await boundDevice(
    context.env.DB,
    session.user.id,
    session.session.id,
  );
  if (!principal) return context.json({ error: "device_required" }, 403);
  await context.env.DB.prepare(
    "UPDATE sync_devices SET revoked_at = ? WHERE vault_id = ? AND id = ? AND revoked_at IS NULL",
  )
    .bind(Date.now(), principal.vault, context.req.param("id"))
    .run();
  return context.json({ revoked: true });
});

app.use("/api/sync/*", async (context, next) => {
  if (!context.req.header("authorization")?.startsWith("Bearer "))
    return context.json({ error: "unauthorized" }, 401);
  const session = await createAuth(context.env).api.getSession({
    headers: context.req.raw.headers,
  });
  if (!session?.user.emailVerified)
    return context.json({ error: "unauthorized" }, 401);
  const principal = await boundDevice(
    context.env.DB,
    session.user.id,
    session.session.id,
  );
  if (!principal) return context.json({ error: "device_required" }, 403);
  if (
    context.req.header("x-loofah-recovery-generation") !== principal.generation
  )
    return context.json({ error: "recovery_required" }, 409);
  context.set("principal", principal);
  await next();
});
app.post("/api/sync/reservations", async (context) => {
  await new VaultStorage(context.env.DB, context.get("principal")).reserve(
    await context.req.json(),
  );
  return context.json({ reserved: true });
});
app.post("/api/sync/snapshots", async (context) =>
  context.json(
    await new VaultSnapshots(context.env.DB, context.get("principal")).begin(),
  ),
);
app.get("/api/sync/snapshots/:id", async (context) =>
  context.json(
    await new VaultSnapshots(context.env.DB, context.get("principal")).page(
      context.req.param("id"),
      context.req.query("after"),
    ),
  ),
);
app.delete("/api/sync/snapshots/:id", async (context) => {
  await new VaultSnapshots(context.env.DB, context.get("principal")).release(
    context.req.param("id"),
  );
  return context.json({ released: true });
});
app.get("/api/sync/heads/:entity", async (context) => {
  const entity = context.req.param("entity");
  if (!/^[a-f0-9]{64}$/.test(entity)) throw new StorageError("invalid");
  const head = await context.env.DB.prepare(
    "SELECT revision FROM sync_heads WHERE vault_id = ? AND entity = ?",
  )
    .bind(context.get("principal").vault, entity)
    .first<{ revision: string }>();
  return context.json({ revision: head?.revision ?? null });
});
app.get("/api/sync/changes", async (context) => {
  const after = Number(context.req.query("after") ?? 0);
  if (!Number.isSafeInteger(after) || after < 0)
    throw new StorageError("invalid");
  const changes =
    await context.env.DB.prepare(`SELECT sequence, entity, revision, operation, created_at FROM sync_changes
    WHERE vault_id = ? AND sequence > ? ORDER BY sequence LIMIT 100`)
      .bind(context.get("principal").vault, after)
      .all();
  return context.json({ changes: changes.results });
});
app.post("/api/sync/revisions", async (context) => {
  const revision = await new VaultStorage(
    context.env.DB,
    context.get("principal"),
  ).commit(await context.req.json());
  return context.json({ revision });
});
app.get("/api/sync/history/:entity", async (context) => {
  const cursor = context.req.query("before")
    ? {
        at: Number(context.req.query("before")),
        id: context.req.query("id") ?? "",
      }
    : undefined;
  return context.json(
    await new VaultStorage(context.env.DB, context.get("principal")).history(
      context.req.param("entity"),
      cursor,
    ),
  );
});
app.post("/api/sync/history/purge", async (context) => {
  await new VaultStorage(context.env.DB, context.get("principal")).purge(
    await context.req.json(),
  );
  return context.json({ purged: true });
});
app.get("/api/sync/revisions/:id", async (context) => {
  const principal = context.get("principal");
  const revision = await context.env.DB.prepare(
    "SELECT * FROM sync_revisions WHERE vault_id = ? AND id = ?",
  )
    .bind(principal.vault, context.req.param("id"))
    .first();
  if (!revision) return context.json({ error: "not_found" }, 404);
  const objects =
    await context.env.DB.prepare(`SELECT o.id, o.bytes, o.digest, o.kind FROM sync_membership m
    JOIN sync_objects o ON o.vault_id = m.vault_id AND o.id = m.object WHERE m.vault_id = ? AND m.revision = ? ORDER BY o.id`)
      .bind(principal.vault, context.req.param("id"))
      .all();
  return context.json({ revision, objects: objects.results });
});
app.put("/api/sync/objects/:id", async (context) => {
  await new ObjectStorage(
    context.env.DB,
    context.env.VAULT,
    context.get("principal"),
  ).put(context.req.param("id"), await context.req.arrayBuffer());
  return context.json({ uploaded: true });
});
app.get("/api/sync/objects/:id", (context) =>
  new ObjectStorage(
    context.env.DB,
    context.env.VAULT,
    context.get("principal"),
  ).get(context.req.param("id")),
);
app.post("/api/sync/objects/:id/multipart", async (context) =>
  context.json(
    await new ObjectStorage(
      context.env.DB,
      context.env.VAULT,
      context.get("principal"),
    ).startMultipart(context.req.param("id")),
  ),
);
app.put("/api/sync/objects/:id/parts/:number", async (context) => {
  await new ObjectStorage(
    context.env.DB,
    context.env.VAULT,
    context.get("principal"),
  ).putPart(
    context.req.param("id"),
    Number(context.req.param("number")),
    context.req.header("x-loofah-sha256") ?? "",
    await context.req.arrayBuffer(),
  );
  return context.json({ uploaded: true });
});
app.post("/api/sync/objects/:id/complete", async (context) => {
  await new ObjectStorage(
    context.env.DB,
    context.env.VAULT,
    context.get("principal"),
  ).completeMultipart(context.req.param("id"));
  return context.json({ uploaded: true });
});

app.onError((error, context) => {
  if (error instanceof EnrollmentError)
    return context.json({ error: "enrollment_failed" }, 403);
  if (error instanceof StorageError)
    return context.json(
      { error: error.code },
      error.code === "invalid"
        ? 400
        : error.code === "quota"
          ? 507
          : error.code === "unavailable"
            ? 503
            : 409,
    );
  return context.json({ error: "unavailable" }, 503);
});
app.notFound((context) =>
  context.req.path.startsWith("/api/")
    ? context.json({ error: "not_found" }, 404)
    : context.env.ASSETS.fetch(context.req.raw),
);

async function processJobs(env: Environment) {
  await new DurableJobs(env.DB).run({
    account_email: async (payload) => {
      const value = await openJob(env.EMAIL_JOB_KEY, payload);
      if (
        !value ||
        typeof value !== "object" ||
        !("kind" in value) ||
        !("to" in value) ||
        !("url" in value) ||
        !["verify", "reset"].includes(String(value.kind)) ||
        typeof value.to !== "string" ||
        typeof value.url !== "string" ||
        new URL(value.url).origin !== env.ACCOUNT_ORIGIN
      )
        throw new Error("invalid account email");
      await env.EMAIL.send({
        from: "accounts@notify.loofah.io",
        to: value.to,
        subject:
          value.kind === "verify"
            ? "Verify your Loofah email"
            : "Reset your Loofah password",
        text: `${value.kind === "verify" ? "Verify your email to finish setting up your Loofah account" : "Reset your Loofah account password"}:\n\n${value.url}\n\nIf you did not request this, you can ignore this email.`,
      });
    },
  });
}

export default {
  fetch: app.fetch,
  async scheduled(_controller: ScheduledController, env: Environment) {
    const settings = await env.DB.prepare(
      "SELECT writes_enabled FROM sync_beta_settings WHERE id = 1",
    ).first<{ writes_enabled: number }>();
    if (settings?.writes_enabled !== 1) return;
    await processJobs(env);
    await expireHistory(env.DB);
    await env.DB.prepare("DELETE FROM sync_snapshots WHERE expires_at < ?")
      .bind(Date.now())
      .run();
    await env.DB.prepare(
      "DELETE FROM sync_enrollment_challenges WHERE expires_at < ?",
    )
      .bind(Date.now())
      .run();
    await env.DB.prepare(`DELETE FROM sync_request_limits WHERE key IN (
      SELECT key FROM sync_request_limits WHERE expires_at < ? ORDER BY expires_at LIMIT 1000)`)
      .bind(Date.now())
      .run();
  },
} satisfies ExportedHandler<Environment>;

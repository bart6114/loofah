import "./style.css";

import { useForm } from "@tanstack/react-form";
import {
  QueryClient,
  QueryClientProvider,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";

import { api, ApiError, approveDevice } from "./api";
import {
  clearFlow,
  continuationUrl,
  deviceCode,
  readFlow,
  signInLabel,
} from "./flow";

const client = new QueryClient({
  defaultOptions: { queries: { retry: false, staleTime: 30_000 } },
});
const location = new URL(window.location.href);
const resetToken = location.searchParams.get("token") ?? "";
const flowStorage = (() => {
  try {
    return window.sessionStorage;
  } catch {
    return undefined;
  }
})();
const invitation = readFlow(
  flowStorage,
  "loofah.invitation",
  new URLSearchParams(location.hash.slice(1)).get("invitation") ?? "",
);
const pendingDevice = deviceCode(
  readFlow(
    flowStorage,
    "loofah.device",
    deviceCode(location.searchParams.get("user_code")),
  ),
);
const nextUrl = (path: string) => continuationUrl(path, pendingDevice);
if (resetToken || location.hash)
  window.history.replaceState({}, "", nextUrl(location.pathname));

function Retry({ error, retry }: { error: Error; retry: () => void }) {
  return (
    <div role="alert">
      <p>{error.message}</p>
      {error instanceof ApiError && error.status === 401 ? (
        <a href={nextUrl("/login")}>Sign in</a>
      ) : (
        <button className="secondary" onClick={retry}>
          Try again
        </button>
      )}
    </div>
  );
}

declare global {
  interface Window {
    turnstile?: {
      render: (
        element: HTMLElement,
        options: {
          sitekey: string;
          action: string;
          callback: (token: string) => void;
          "expired-callback": () => void;
        },
      ) => string;
      remove: (id: string) => void;
      reset: (id: string) => void;
    };
  }
}

function Challenge({
  sitekey,
  onToken,
  resetRef,
}: {
  sitekey: string;
  onToken: (token: string) => void;
  resetRef: { current: (() => void) | null };
}) {
  const container = useRef<HTMLDivElement>(null);
  const callback = useRef(onToken);
  callback.current = onToken;
  useEffect(() => {
    let id: string | undefined;
    let cancelled = false;
    const render = () => {
      if (cancelled || !container.current || !window.turnstile || id) return;
      id = window.turnstile.render(container.current, {
        sitekey,
        action: "account",
        callback: (token) => callback.current(token),
        "expired-callback": () => callback.current(""),
      });
      resetRef.current = () => {
        if (id) window.turnstile?.reset(id);
        callback.current("");
      };
    };
    let script = document.querySelector<HTMLScriptElement>(
      "script[data-turnstile]",
    );
    if (!script) {
      script = document.createElement("script");
      script.src =
        "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
      script.dataset.turnstile = "true";
      script.async = true;
      document.head.append(script);
    }
    script.addEventListener("load", render);
    render();
    return () => {
      cancelled = true;
      script?.removeEventListener("load", render);
      if (id) window.turnstile?.remove(id);
      resetRef.current = null;
    };
  }, [sitekey, resetRef]);
  return <div ref={container} className="challenge" />;
}

function AccountForm({
  kind,
  sitekey,
}: {
  kind:
    | "login"
    | "signup"
    | "forgot"
    | "resend"
    | "reset"
    | "password"
    | "device";
  sitekey: string;
}) {
  const [showPassword, setShowPassword] = useState(false);
  const resetRef = useRef<(() => void) | null>(null);
  const queryClient = useQueryClient();
  const requiresChallenge = ["login", "signup", "forgot", "resend"].includes(
    kind,
  );
  const mutation = useMutation({
    mutationFn: async (value: {
      email: string;
      password: string;
      name: string;
      invitation: string;
      challenge: string;
      newPassword: string;
      userCode: string;
    }) => {
      const headers = {
        "x-turnstile-token": value.challenge,
        "x-loofah-invitation": value.invitation,
      };
      switch (kind) {
        case "login":
          return api(
            "/auth/sign-in/email",
            { email: value.email, password: value.password },
            headers,
          );
        case "signup":
          return api(
            "/auth/sign-up/email",
            {
              email: value.email,
              password: value.password,
              name: value.name,
              callbackURL: `${window.location.origin}${nextUrl("/verify")}`,
            },
            headers,
          );
        case "forgot":
          return api(
            "/auth/request-password-reset",
            {
              email: value.email,
              redirectTo: `${window.location.origin}${nextUrl("/reset-password")}`,
            },
            headers,
          );
        case "resend":
          return api(
            "/auth/send-verification-email",
            {
              email: value.email,
              callbackURL: `${window.location.origin}${nextUrl("/verify")}`,
            },
            headers,
          );
        case "reset":
          return api("/auth/reset-password", {
            token: resetToken,
            newPassword: value.newPassword,
          });
        case "password":
          return api("/auth/change-password", {
            currentPassword: value.password,
            newPassword: value.newPassword,
            revokeOtherSessions: true,
          });
        case "device":
          return approveDevice(value.userCode);
      }
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries();
      if (kind === "signup") clearFlow(flowStorage, "loofah.invitation");
      if (kind === "device") clearFlow(flowStorage, "loofah.device");
      if (kind === "login")
        window.location.assign(pendingDevice ? nextUrl("/device") : "/account");
    },
    onSettled: () => resetRef.current?.(),
  });
  const form = useForm({
    defaultValues: {
      email: "",
      password: "",
      name: "",
      invitation,
      challenge: "",
      newPassword: "",
      userCode: pendingDevice,
    },
    onSubmit: async ({ value }) => {
      await mutation.mutateAsync(value).catch(() => {});
    },
  });
  const fields: {
    name:
      | "email"
      | "password"
      | "name"
      | "invitation"
      | "newPassword"
      | "userCode";
    label: string;
    type?: string;
    autocomplete?: string;
  }[] = [];
  if (["login", "signup", "forgot", "resend"].includes(kind))
    fields.push({
      name: "email",
      label: "Email",
      type: "email",
      autocomplete: "email",
    });
  if (kind === "signup")
    fields.push({ name: "name", label: "Name", autocomplete: "name" });
  if (["login", "signup", "password"].includes(kind))
    fields.push({
      name: "password",
      label: kind === "password" ? "Current password" : "Password",
      type: "password",
      autocomplete: kind === "signup" ? "new-password" : "current-password",
    });
  if (["reset", "password"].includes(kind))
    fields.push({
      name: "newPassword",
      label: "New password",
      type: "password",
      autocomplete: "new-password",
    });
  if (kind === "device")
    fields.push({
      name: "userCode",
      label: "Code shown in Loofah on your Mac",
      autocomplete: "off",
    });
  const labels = {
    login: "Sign in",
    signup: "Create account",
    forgot: "Send reset link",
    resend: "Send verification link",
    reset: "Set new password",
    password: "Change password",
    device: "Approve this Mac’s login",
  };
  if (mutation.isSuccess && kind !== "login")
    return (
      <div className="notice" role="status">
        {kind === "device" ? (
          <>
            <strong>Signed in. Finish setting up sync on your Mac.</strong>
            <p>
              Return to Loofah Staging → Settings → Sync. On your first Mac,
              save your recovery kit and choose the saved file to verify it. For
              another Mac, use that kit or connect through a Mac you already set
              up.
            </p>
            <p>
              Then choose Start syncing. Signing in alone does not upload your
              notes.
            </p>
          </>
        ) : kind === "password" ? (
          "Password changed. Other sessions have been signed out."
        ) : kind === "reset" ? (
          <>
            Password changed. <a href={nextUrl("/login")}>Sign in</a> to
            continue.
          </>
        ) : (
          <>
            <p>
              Check your inbox. If this request is eligible, a link will arrive
              shortly.
            </p>
            <p>Delivery can take a few minutes. Check your spam folder too.</p>
            <a
              href={nextUrl(
                kind === "forgot" ? "/forgot-password" : "/resend-verification",
              )}
            >
              Request another email
            </a>
            {kind !== "forgot" && (
              <p>
                <a href={nextUrl("/login")}>Already verified? Sign in</a>
              </p>
            )}
          </>
        )}
      </div>
    );
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit();
      }}
    >
      {fields.map(({ name, label, type = "text", autocomplete }) => (
        <form.Field key={name} name={name}>
          {(field) => (
            <label>
              {label}
              <input
                name={name}
                type={type === "password" && showPassword ? "text" : type}
                autoComplete={autocomplete}
                required
                minLength={
                  name === "newPassword" ||
                  (name === "password" && kind === "signup")
                    ? 12
                    : undefined
                }
                value={field.state.value}
                onBlur={field.handleBlur}
                onChange={(event) => field.handleChange(event.target.value)}
              />
            </label>
          )}
        </form.Field>
      ))}
      {fields.some((field) => field.type === "password") && (
        <label className="password-toggle">
          <input
            type="checkbox"
            checked={showPassword}
            onChange={(event) => setShowPassword(event.target.checked)}
          />
          Show password{kind === "password" ? "s" : ""}
        </label>
      )}
      {["signup", "reset", "password"].includes(kind) && (
        <p className="muted">Use at least 12 characters.</p>
      )}
      {kind === "password" && (
        <p>
          Changing your password signs out your other browsers and Macs. You
          will need to sign in again there. Your local notes and recovery kit
          stay unchanged.
        </p>
      )}
      {requiresChallenge && (
        <Challenge
          sitekey={sitekey}
          resetRef={resetRef}
          onToken={(token) => form.setFieldValue("challenge", token)}
        />
      )}
      {mutation.error && (
        <p role="alert" className="error">
          {mutation.error.message}
        </p>
      )}
      <form.Subscribe
        selector={(state) =>
          [state.isSubmitting, state.values.challenge] as const
        }
      >
        {([submitting, challenge]) => (
          <button
            type="submit"
            disabled={submitting || (requiresChallenge && !challenge)}
          >
            {submitting ? "Please wait…" : labels[kind]}
          </button>
        )}
      </form.Subscribe>
    </form>
  );
}

function Account() {
  const queryClient = useQueryClient();
  const account = useQuery({
    queryKey: ["account"],
    queryFn: () =>
      api<{
        account: {
          used_bytes: number;
          quota_bytes: number;
          synced_items: number;
          active_devices: number;
          last_change_at: number | null;
        };
        identity: { email: string; session: string };
      }>("/account"),
    refetchInterval: 30_000,
  });
  const sessions = useQuery({
    queryKey: ["sessions"],
    queryFn: () =>
      api<
        {
          id: string;
          token: string;
          userAgent?: string;
          createdAt: string;
          current: boolean;
          device: { name?: string | null } | null;
        }[]
      >("/account/sign-ins"),
    enabled: account.isSuccess,
  });
  const revoke = useMutation({
    mutationFn: (token: string) => api("/auth/revoke-session", { token }),
    onSuccess: (_, token) => {
      if (
        sessions.data?.some(
          (session) => session.current && session.token === token,
        )
      )
        window.location.assign(nextUrl("/login"));
      else void queryClient.invalidateQueries({ queryKey: ["sessions"] });
    },
  });
  const logout = useMutation({
    mutationFn: () => api("/auth/sign-out", {}),
    onSuccess: () => {
      queryClient.clear();
      window.location.assign(nextUrl("/login"));
    },
  });
  if (account.isPending) return <p role="status">Loading your account…</p>;
  if (account.error)
    return <Retry error={account.error} retry={() => void account.refetch()} />;
  return (
    <>
      <h1>Your account</h1>
      <p>
        Signed in as <strong>{account.data.identity.email}</strong>
      </p>
      {account.data.account.active_devices === 0 && (
        <div className="notice">
          <strong>Your account is ready. Set up sync on your Mac next.</strong>
          <p>
            Open Loofah Staging → Settings → Sync. Sign in, save and verify your
            recovery kit, then choose Start syncing. For a vault you already set
            up, use your kit or another connected Mac.
          </p>
        </div>
      )}
      <section>
        <h2>Sync storage</h2>
        <p className="usage">
          {(account.data.account.used_bytes / 1e9).toFixed(2)}{" "}
          <small>
            / {(account.data.account.quota_bytes / 1e9).toLocaleString()} GB
          </small>
        </p>
        <progress
          aria-label="Sync storage used"
          max={account.data.account.quota_bytes}
          value={account.data.account.used_bytes}
        />
        <dl className="sync-stats">
          <div>
            <dt>Synced items</dt>
            <dd>{account.data.account.synced_items.toLocaleString()}</dd>
          </div>
          <div>
            <dt>Macs with access</dt>
            <dd>{account.data.account.active_devices.toLocaleString()}</dd>
          </div>
        </dl>
        <p className="muted">
          Items include sessions and your vault-wide people, tags and task
          lists. Deleted items are excluded. Sync connects your own Macs; it
          does not share content with other people.
        </p>
        <p>
          <strong>Latest cloud update</strong>
          <br />
          {account.data.account.last_change_at === null ? (
            "No changes uploaded yet"
          ) : (
            <time
              dateTime={new Date(
                account.data.account.last_change_at,
              ).toISOString()}
            >
              {new Date(account.data.account.last_change_at).toLocaleString()}
            </time>
          )}
        </p>
        <p>
          Current content, retained history and unresolved conflicts share this
          allowance. Content retained in history continues using storage after
          deletion.
        </p>
        <p>
          Manage sync and your recovery kit in Loofah Staging → Settings → Sync.
          This page updates every 30 seconds while open.
        </p>
      </section>
      <section>
        <h2>Account sign-ins</h2>
        <p>
          Signing out stops access to your account from that browser or app.
          Local notes stay on the Mac.
        </p>
        {sessions.isPending && <p role="status">Loading sign-ins…</p>}
        {sessions.error && (
          <Retry error={sessions.error} retry={() => void sessions.refetch()} />
        )}
        <ul className="sessions">
          {sessions.data?.map((session) => (
            <li key={session.id}>
              <div>
                <strong>
                  {signInLabel(session.userAgent, session.device)}
                  {session.current ? " · This browser" : ""}
                </strong>
                <small>
                  Signed in {new Date(session.createdAt).toLocaleString()}
                </small>
              </div>
              <button
                className="secondary"
                disabled={revoke.isPending}
                onClick={() => revoke.mutate(session.token)}
              >
                Sign out
              </button>
            </li>
          ))}
        </ul>
        {revoke.error && (
          <p role="alert" className="error">
            {revoke.error.message}
          </p>
        )}
      </section>
      <section>
        <h2>Account security</h2>
        <p>
          <a href={nextUrl("/change-password")}>Change password</a>
        </p>
        <p>
          Your account password cannot decrypt your vault. Keep your recovery
          kit somewhere safe.
        </p>
        <button
          className="secondary"
          disabled={logout.isPending}
          onClick={() => logout.mutate()}
        >
          Sign out of this browser
        </button>
        {logout.error && <p role="alert">{logout.error.message}</p>}
      </section>
    </>
  );
}

function SwitchAccount() {
  const change = useMutation({
    mutationFn: () => api("/auth/sign-out", {}),
    onSuccess: () => window.location.assign(nextUrl("/login")),
  });
  return (
    <>
      <button
        className="link-button"
        disabled={change.isPending}
        onClick={() => change.mutate()}
      >
        Use another account
      </button>
      {change.error && <span role="alert">{change.error.message}</span>}
    </>
  );
}

function Verification() {
  const result = useQuery({
    queryKey: ["verification-result"],
    queryFn: () => api<{ verified: boolean }>("/verification-result"),
    retry: false,
  });
  if (result.isPending)
    return <p role="status">Checking email verification…</p>;
  if (result.error)
    return <Retry error={result.error} retry={() => void result.refetch()} />;
  const verified = result.data.verified && !location.searchParams.has("error");
  return (
    <>
      <h1>{verified ? "Email verified" : "Email verification"}</h1>
      <p>
        {verified
          ? "Your account is ready. Sign in to continue setting up sync."
          : "We could not confirm verification from this page. Open the link in your verification email, or request a new one."}
      </p>
      <p>
        <a href={nextUrl("/login")}>Sign in to continue</a>
        {!verified && (
          <>
            {" "}
            ·{" "}
            <a href={nextUrl("/resend-verification")}>
              Request a new verification link
            </a>
          </>
        )}
      </p>
    </>
  );
}

function App() {
  const config = useQuery({
    queryKey: ["config"],
    queryFn: () => api<{ signupOpen: boolean; sitekey: string }>("/config"),
  });
  const current = location.pathname;
  const deviceSession = useQuery({
    queryKey: ["account"],
    queryFn: () => api<{ identity: { email: string } }>("/account"),
    enabled: current === "/device",
  });
  let content;
  if (config.isPending) content = <p role="status">Loading…</p>;
  else if (config.error)
    content = (
      <Retry error={config.error} retry={() => void config.refetch()} />
    );
  else if (current === "/account" || current === "/") content = <Account />;
  else if (current === "/verify") content = <Verification />;
  else if (
    current === "/reset-password" &&
    (!resetToken || location.searchParams.has("error"))
  )
    content = (
      <>
        <h1>Request a new password link</h1>
        <p>This password reset link is missing, expired or invalid.</p>
        <a href={nextUrl("/forgot-password")}>Send a new reset link</a>
      </>
    );
  else if (current === "/signup" && !config.data.signupOpen)
    content = (
      <>
        <h1>Sync beta</h1>
        <p>Signup is currently closed while we finish testing.</p>
        <p>Loofah on your Mac works offline without an account.</p>
        <a href={nextUrl("/login")}>Already have an account? Sign in</a>
      </>
    );
  else if (current === "/signup" && !invitation)
    content = (
      <>
        <h1>You need an invitation</h1>
        <p>
          Sync is currently invitation-only. Open the invitation link you
          received to create your account. If you refreshed or returned later,
          reopen that link.
        </p>
        <a href={nextUrl("/login")}>Already have an account? Sign in</a>
      </>
    );
  else if (current === "/device" && deviceSession.isPending)
    content = <p role="status">Checking your sign-in…</p>;
  else if (current === "/device" && deviceSession.error)
    content = (
      <>
        <h1>Connect your Mac</h1>
        <Retry
          error={deviceSession.error}
          retry={() => void deviceSession.refetch()}
        />
      </>
    );
  else {
    const pages = {
      "/login": ["Sign in", "login"],
      "/signup": ["Create your invited account", "signup"],
      "/forgot-password": ["Reset your password", "forgot"],
      "/resend-verification": ["Verify your email", "resend"],
      "/reset-password": ["Choose a new password", "reset"],
      "/change-password": ["Change your password", "password"],
      "/device": ["Approve a Mac login", "device"],
    } as const;
    const page = pages[current as keyof typeof pages];
    content = page ? (
      <>
        <h1>{page[0]}</h1>
        {page[1] === "device" && (
          <>
            <p>
              Signed in as <strong>{deviceSession.data?.identity.email}</strong>
              . <SwitchAccount />
            </p>
            <p>
              Approve only the code shown in Loofah Staging on the Mac you are
              connecting. Next, the app will guide you through securing this Mac
              before syncing starts.
            </p>
          </>
        )}
        <AccountForm kind={page[1]} sitekey={config.data.sitekey} />
        {page[1] === "login" && (
          <nav className="form-links">
            <a href={nextUrl("/forgot-password")}>Forgot password?</a>
            <a href={nextUrl("/resend-verification")}>Resend verification</a>
            <a href={nextUrl("/signup")}>Have an invitation?</a>
          </nav>
        )}
        {page[1] === "signup" && (
          <>
            <p className="muted">
              Sync is optional. You can continue using Loofah offline without an
              account.
            </p>
            <nav className="form-links">
              <a href={nextUrl("/login")}>Already registered? Sign in</a>
              <a href={nextUrl("/resend-verification")}>
                Waiting for verification? Send another email
              </a>
            </nav>
          </>
        )}
      </>
    ) : (
      <>
        <h1>Page not found</h1>
        <a href="/account">Go to your account</a>
      </>
    );
  }
  return (
    <>
      <header>
        <a href="https://loofah.io" className="brand">
          Loofah<span>private by nature</span>
        </a>
        <a href="/account">Account</a>
      </header>
      <main>{content}</main>
      <footer>
        <a href="https://loofah.io">Back to Loofah</a>
        <span>Optional sync. Your local files stay yours.</span>
      </footer>
    </>
  );
}

createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={client}>
    <App />
  </QueryClientProvider>,
);

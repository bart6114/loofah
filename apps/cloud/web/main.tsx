import "./style.css";

import { useForm } from "@tanstack/react-form";
import {
  QueryClient,
  QueryClientProvider,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { createRoot } from "react-dom/client";

const client = new QueryClient({
  defaultOptions: { queries: { retry: false, staleTime: 30_000 } },
});
const location = new URL(window.location.href);
const resetToken = location.searchParams.get("token") ?? "";
const invitation =
  new URLSearchParams(location.hash.slice(1)).get("invitation") ?? "";
if (resetToken || invitation)
  window.history.replaceState({}, "", location.pathname);

async function api<T = unknown>(
  path: string,
  body?: unknown,
  headers?: Record<string, string>,
): Promise<T> {
  const response = await fetch(`/api${path}`, {
    method: body === undefined ? "GET" : "POST",
    credentials: "same-origin",
    headers: {
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      ...headers,
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const value = await response.json();
  if (!response.ok)
    throw new Error(
      value.message ??
        (
          {
            signup_closed:
              "Signup is paused. Existing accounts can still sign in.",
            invitation_invalid:
              "This invitation is invalid or has been revoked. Ask for a new invitation.",
            invitation_expired:
              "This invitation has expired. Ask for a new invitation.",
            invitation_consumed:
              "This invitation has already been used. Sign in or request another verification email below.",
            invitation_email:
              "Use the email address your invitation was issued to.",
            capacity_reached:
              "The private beta is full. Your invitation has not been consumed. Please try again later.",
            challenge_required: "Complete the security check.",
            challenge_failed: "The security check expired. Try again.",
            rate_limited: "Too many attempts. Wait a minute and try again.",
            unauthorized: "Sign in to continue.",
            maintenance: "Account service is temporarily unavailable.",
          } as Record<string, string>
        )[value.error] ??
        "This request could not be completed. Please try again.",
    );
  return value as T;
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
              callbackURL: `${window.location.origin}/verify`,
            },
            headers,
          );
        case "forgot":
          return api(
            "/auth/request-password-reset",
            {
              email: value.email,
              redirectTo: `${window.location.origin}/reset-password`,
            },
            headers,
          );
        case "resend":
          return api(
            "/auth/send-verification-email",
            {
              email: value.email,
              callbackURL: `${window.location.origin}/verify`,
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
          return api("/auth/device/approve", { userCode: value.userCode });
      }
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries();
      if (kind === "login")
        window.location.assign(
          location.searchParams.get("next") === "device"
            ? `/device?user_code=${encodeURIComponent(location.searchParams.get("user_code") ?? "")}`
            : "/account",
        );
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
      userCode: location.searchParams.get("user_code") ?? "",
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
    fields.push(
      { name: "name", label: "Name", autocomplete: "name" },
      { name: "invitation", label: "Invitation code", autocomplete: "off" },
    );
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
          "Login approved. Return to Loofah on your Mac to finish device enrollment."
        ) : kind === "password" ? (
          "Password changed. Other sessions have been signed out."
        ) : kind === "reset" ? (
          <>
            Password changed. <a href="/login">Sign in</a> to continue.
          </>
        ) : (
          <>
            <p>
              Check your inbox. If this request is eligible, a link will arrive
              shortly.
            </p>
            <p>Delivery can take a few minutes. Check your spam folder too.</p>
            <a
              href={
                kind === "forgot" ? "/forgot-password" : "/resend-verification"
              }
            >
              Request another email
            </a>
            {kind !== "forgot" && (
              <p>
                <a href="/login">Already verified? Sign in</a>
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
                type={type}
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
      api<{ account: { used_bytes: number; quota_bytes: number } }>("/account"),
  });
  const sessions = useQuery({
    queryKey: ["sessions"],
    queryFn: () =>
      api<
        { id: string; token: string; userAgent?: string; createdAt: string }[]
      >("/auth/list-sessions"),
    enabled: account.isSuccess,
  });
  const revoke = useMutation({
    mutationFn: (token: string) => api("/auth/revoke-session", { token }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["sessions"] }),
  });
  const logout = useMutation({
    mutationFn: () => api("/auth/sign-out", {}),
    onSuccess: () => {
      queryClient.clear();
      window.location.assign("/login");
    },
  });
  if (account.isPending) return <p role="status">Loading your account…</p>;
  if (account.error)
    return (
      <p>
        {account.error.message} <a href="/login">Sign in</a>
      </p>
    );
  return (
    <>
      <h1>Your account</h1>
      <section>
        <h2>Sync storage</h2>
        <p className="usage">
          {(account.data.account.used_bytes / 1e9).toFixed(2)}{" "}
          <small>/ 25 GB</small>
        </p>
        <p>
          Current content, retained history and unresolved conflicts share this
          allowance. Content retained in history continues using storage after
          deletion.
        </p>
        <p>
          Mac sync is being prepared for the private beta. Account creation does
          not upload content from your Mac.
        </p>
      </section>
      <section>
        <h2>Signed-in sessions</h2>
        {sessions.error && <p role="alert">Could not load sessions.</p>}
        <ul className="sessions">
          {sessions.data?.map(
            (session: {
              id: string;
              token: string;
              userAgent?: string;
              createdAt: string;
            }) => (
              <li key={session.id}>
                <div>
                  <strong>
                    {session.userAgent?.includes("Mac")
                      ? "Mac"
                      : "Browser or app session"}
                  </strong>
                  <small>
                    Signed in {new Date(session.createdAt).toLocaleDateString()}
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
            ),
          )}
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
          <a href="/change-password">Change password</a>
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

function App() {
  const config = useQuery({
    queryKey: ["config"],
    queryFn: () => api<{ signupOpen: boolean; sitekey: string }>("/config"),
  });
  const current = location.pathname;
  const deviceSession = useQuery({
    queryKey: ["account"],
    queryFn: () => api("/account"),
    enabled: current === "/device",
  });
  let content;
  if (config.isPending) content = <p role="status">Loading…</p>;
  else if (config.error)
    content = (
      <p role="alert">
        Account service is temporarily unavailable. Please try again.
      </p>
    );
  else if (current === "/account" || current === "/") content = <Account />;
  else if (current === "/verify")
    content = (
      <>
        <h1>Email verification</h1>
        <p>
          {location.searchParams.has("error")
            ? "This verification link could not be used. Request a new link below."
            : "Sign in to check your account’s verification status."}
        </p>
        <p>
          <a href="/login">Sign in</a> ·{" "}
          <a href="/resend-verification">Request a new link</a>
        </p>
      </>
    );
  else if (current === "/signup" && !config.data.signupOpen)
    content = (
      <>
        <h1>Sync beta</h1>
        <p>Signup is currently closed while we finish testing.</p>
        <p>Loofah on your Mac works offline without an account.</p>
        <a href="/login">Already have an account? Sign in</a>
      </>
    );
  else if (current === "/device" && !deviceSession.isSuccess)
    content = (
      <>
        <h1>Connect your Mac</h1>
        <p>Sign in to approve the code displayed in Loofah.</p>
        <a
          href={`/login?next=device&user_code=${encodeURIComponent(location.searchParams.get("user_code") ?? "")}`}
        >
          Sign in
        </a>
      </>
    );
  else {
    const pages = {
      "/login": ["Sign in", "login"],
      "/signup": ["Create your account", "signup"],
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
          <p>
            Only approve a code shown in Loofah on the Mac you are connecting.
            Your vault keys are transferred separately from a trusted Mac or
            your recovery kit.
          </p>
        )}
        <AccountForm kind={page[1]} sitekey={config.data.sitekey} />
        {page[1] === "login" && (
          <nav className="form-links">
            <a href="/forgot-password">Forgot password?</a>
            <a href="/resend-verification">Resend verification</a>
            <a href="/signup">Create account</a>
          </nav>
        )}
        {page[1] === "signup" && (
          <>
            <p className="muted">
              Sync is optional. You can continue using Loofah offline without an
              account.
            </p>
            <nav className="form-links">
              <a href="/login">Already registered? Sign in</a>
              <a href="/resend-verification">
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

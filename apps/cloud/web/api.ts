export class ApiError extends Error {
  constructor(
    message: string,
    public status: number,
  ) {
    super(message);
  }
}

export async function api<T = unknown>(
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
  }).catch(() => {
    throw new ApiError(
      "Could not reach Loofah. Check your connection and try again.",
      0,
    );
  });
  const parsed = await response.json().catch(() => null);
  const value = parsed && typeof parsed === "object" ? parsed : {};
  if (!response.ok)
    throw new ApiError(
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
            expired_token:
              "This device login code has expired. Cancel the connection in Loofah and connect again for a new code.",
            access_denied:
              "This code belongs to another account. Cancel the connection in Loofah and connect again with the correct account.",
            unauthorized: "Sign in to continue.",
            maintenance: "Account service is temporarily unavailable.",
          } as Record<string, string>
        )[value.error] ??
        value.error_description ??
        "This request could not be completed. Please try again.",
      response.status,
    );
  return value as T;
}

export async function approveDevice(userCode: string) {
  // Better Auth must bind the code to this browser session before approval.
  await api(`/auth/device?user_code=${encodeURIComponent(userCode)}`);
  return api("/auth/device/approve", { userCode });
}

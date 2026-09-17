export function deviceCode(value: string | null) {
  return value && /^[A-Z0-9-]{6,32}$/.test(value) ? value : "";
}

export function continuationUrl(path: string, code: string) {
  const valid = deviceCode(code);
  return valid
    ? `${path}?next=device&user_code=${encodeURIComponent(valid)}`
    : path;
}

export function readFlow(
  storage: Storage | undefined,
  key: string,
  incoming = "",
) {
  try {
    if (incoming) {
      storage?.setItem(
        key,
        JSON.stringify({ value: incoming, expires: Date.now() + 600_000 }),
      );
      return incoming;
    }
    const saved = JSON.parse(storage?.getItem(key) || "null");
    if (typeof saved?.value === "string" && saved.expires > Date.now())
      return saved.value as string;
    storage?.removeItem(key);
  } catch {}
  return incoming;
}

export function clearFlow(storage: Storage | undefined, key: string) {
  try {
    storage?.removeItem(key);
  } catch {}
}

export function signInLabel(
  userAgent: string | undefined,
  device: { name?: string | null } | null,
) {
  if (device) return device.name ? `Loofah · ${device.name}` : "Loofah on Mac";
  const browser = userAgent?.includes("Edg/")
    ? "Edge"
    : userAgent?.includes("Firefox/")
      ? "Firefox"
      : userAgent?.includes("Chrome/")
        ? "Chrome"
        : userAgent?.includes("Safari/")
          ? "Safari"
          : null;
  if (browser)
    return `${browser}${userAgent?.includes("Mac") ? " on Mac" : ""}`;
  return "App or unrecognized browser";
}

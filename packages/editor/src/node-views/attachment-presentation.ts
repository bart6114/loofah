import { commands as openerCommands } from "@hypr/plugin-opener2";

import type { AttachmentResolution } from "./attachment-resolver";

export type AttachmentAction = () => void | Promise<unknown>;
export type AttachmentPresentation = (input: {
  src?: unknown;
  path?: unknown;
  resolved?: AttachmentResolution | null;
}) => { src?: string; open?: AttachmentAction; reveal?: AttachmentAction };

export const legacyAttachmentPresentation: AttachmentPresentation = ({
  src,
  path,
  resolved,
}) => {
  const location =
    resolved?.path ?? (typeof path === "string" ? path : undefined);
  return {
    src: resolved?.src ?? (typeof src === "string" ? src : undefined),
    open:
      resolved?.open ??
      (location
        ? async () =>
            location.startsWith("https://")
              ? await openerCommands.openUrl(location, null)
              : await openerCommands.openPath(location, null)
        : undefined),
    reveal:
      resolved?.reveal ??
      (location && !location.startsWith("https://")
        ? async () => await openerCommands.revealItemInDir(location)
        : undefined),
  };
};

// Resolved values come from a client-owned resource handle, never a server path.
export const scopedAttachmentPresentation: AttachmentPresentation = ({
  src,
  resolved,
}) => ({
  src: resolved?.src ?? externalImageSource(src),
  open: resolved?.open,
  reveal: resolved?.reveal,
});

function externalImageSource(value: unknown): string | undefined {
  if (typeof value !== "string") return;
  if (/^data:image\/[a-z0-9.+-]+[;,]/i.test(value.slice(0, 256))) return value;
  if (value.length > 32 * 1024) return;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return;
  }
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username ||
    url.password
  )
    return;
  const host = url.hostname
    .toLowerCase()
    .replace(/\.$/, "")
    .replace(/^\[|\]$/g, "");
  if (host === "localhost" || host.endsWith(".localhost")) return;
  if (host.includes(":")) {
    if (
      host === "::" ||
      host === "::1" ||
      /^(?:::ffff:|f[cd]|fe[89ab])/i.test(host)
    )
      return;
  } else if (/^\d+\.\d+\.\d+\.\d+$/.test(host)) {
    const [a, b] = host.split(".").map(Number);
    if (
      a === 0 ||
      a === 10 ||
      a === 127 ||
      a >= 224 ||
      (a === 169 && b === 254) ||
      (a === 172 && b >= 16 && b <= 31) ||
      (a === 192 && b === 168) ||
      (a === 100 && b >= 64 && b <= 127)
    )
      return;
  }
  return value;
}

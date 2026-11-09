import { beforeEach, describe, expect, it, vi } from "vitest";

const commands = vi.hoisted(() => ({
  openUrl: vi.fn(),
  openPath: vi.fn(),
  revealItemInDir: vi.fn(),
}));
vi.mock("@hypr/plugin-opener2", () => ({ commands }));

import {
  legacyAttachmentPresentation,
  scopedAttachmentPresentation,
} from "./attachment-presentation";

beforeEach(() => vi.clearAllMocks());

describe("client attachment presentation", () => {
  it("keeps legacy device paths and web file actions in the local adapter", async () => {
    for (const path of [
      "/Users/me/Vault/attachments/paper.pdf",
      "C:\\Users\\Me\\Vault\\paper.pdf",
    ]) {
      const input = Object.freeze({ src: "asset:/paper.pdf", path });
      const view = legacyAttachmentPresentation(input);
      expect(view.src).toBe(input.src);
      await view.open?.();
      await view.reveal?.();
      expect(commands.openPath).toHaveBeenLastCalledWith(path, null);
      expect(commands.revealItemInDir).toHaveBeenLastCalledWith(path);
    }
    const web = legacyAttachmentPresentation({
      src: "https://example.com/paper.pdf",
      path: "https://example.com/paper.pdf",
    });
    await web.open?.();
    expect(commands.openUrl).toHaveBeenLastCalledWith(
      "https://example.com/paper.pdf",
      null,
    );
    expect(web.reveal).toBeUndefined();
  });

  it("ignores server and node paths and invokes only explicit client actions", async () => {
    const open = vi.fn();
    const reveal = vi.fn();
    const input = Object.freeze({
      src: "asset:/private.png",
      path: "/private/secret",
      resolved: Object.freeze({
        src: "loofah-media://localhost/scoped-opaque-handle",
        path: "/a/server/path",
        open,
        reveal,
      }),
    });
    const view = scopedAttachmentPresentation(input);
    expect(view.src).toBe(input.resolved.src);
    expect(open).not.toHaveBeenCalled();
    await view.open?.();
    await view.reveal?.();
    expect(open).toHaveBeenCalledOnce();
    expect(reveal).toHaveBeenCalledOnce();
    expect(commands.openPath).not.toHaveBeenCalled();
    expect(commands.openUrl).not.toHaveBeenCalled();
    expect(commands.revealItemInDir).not.toHaveBeenCalled();
    const noActions = scopedAttachmentPresentation({
      ...input,
      resolved: { src: input.resolved.src, path: "/server/path" },
    });
    expect(noActions.open).toBeUndefined();
    expect(noActions.reveal).toBeUndefined();
  });

  it.each([
    "asset://localhost/Users/me/private.png",
    "ASSET:/C:/private.png",
    "file:///C:/private.png",
    "file://attacker/share/private.png",
    "\\\\attacker\\share\\private.png",
    "/Users/me/private.png",
    "C:\\private.png",
    "tauri://localhost/private.png",
    "ipc://localhost/private.png",
    "loofah-media://localhost/forged-handle",
    "blob:https://tauri.localhost/another-window",
    "http://asset.localhost/private.png",
    "https://asset.localhost/private.png",
    "http://localhost./private.png",
    "http://127.1/private.png",
    "http://2130706433/private.png",
    "http://0.0.0.0/private.png",
    "http://10.1.2.3/private.png",
    "http://172.16.1.1/private.png",
    "http://192.168.1.1/private.png",
    "http://169.254.169.254/private.png",
    "http://[::1]/private.png",
    "http://[::ffff:127.0.0.1]/private.png",
    "http://[fd00::1]/private.png",
    "http://[fe80::1]/private.png",
    "javascript:alert(1)",
    "data:text/html,<script>bad</script>",
    "https://user:secret@example.com/private.png",
  ])("does not present an unresolved device/native source: %s", (src) => {
    const view = scopedAttachmentPresentation({
      src,
      path: src,
      resolved: null,
    });
    expect(view).toEqual({
      src: undefined,
      open: undefined,
      reveal: undefined,
    });
    expect(commands.openPath).not.toHaveBeenCalled();
  });

  it("does not substitute a raw source for a scoped attachment that is unavailable", () => {
    expect(
      scopedAttachmentPresentation({
        src: "https://example.com/unrelated.png",
        attachmentId: "owned-image.png",
        resolved: null,
      }).src,
    ).toBeUndefined();
    expect(
      scopedAttachmentPresentation({
        src: "https://example.com/unrelated.png",
        resolved: { open: vi.fn() },
      }).src,
    ).toBeUndefined();
  });

  it("keeps supported public and raster image sources without modifying their text", () => {
    for (const src of [
      "https://example.com/image.png?signature=a%2Bb#part",
      "http://example.com/image.png",
      "https://8.8.8.8/image.png",
      "https://[2606:4700:4700::1111]/image.png",
      "data:image/png;base64,aGVsbG8=",
      "data:image/svg+xml,%3Csvg%20xmlns=%22http://www.w3.org/2000/svg%22/%3E",
    ]) {
      const input = Object.freeze({ src });
      expect(scopedAttachmentPresentation(input).src).toBe(src);
    }
    expect(scopedAttachmentPresentation({ src: 42 }).src).toBeUndefined();
    expect(
      scopedAttachmentPresentation({
        src: "https://example.com/" + "x".repeat(32768),
      }).src,
    ).toBeUndefined();
  });
});

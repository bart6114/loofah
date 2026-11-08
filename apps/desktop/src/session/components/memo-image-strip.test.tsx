import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import {
  scopedAttachmentPresentation,
  legacyAttachmentPresentation,
} from "@hypr/editor/attachment-presentation";

import { MemoImageStrip } from "./memo-image-strip";

import { createSessionSourceHost } from "~/session/session-source.test-support";
import type { VaultClient } from "~/shared/application/vault-client";

const source = vi.hoisted(() => ({ raw: "" }));
vi.mock("~/session/queries", () => ({ useSessionRawMd: () => source.raw }));
const hosts: ReturnType<typeof createSessionSourceHost>[] = [];
function fixture(mode: "always" | "online" = "always", scope?: string) {
  const close = vi.fn().mockResolvedValue(undefined);
  const open = vi.fn<VaultClient["openAttachment"]>(
    async (_session, filename) => ({
      id: crypto.randomUUID(),
      info: { filename, sizeBytes: 7, contentType: "image/png" },
      src: "loofah-media://current-image",
      labels: { open: { id: "Open" }, reveal: { id: "Show" } },
      open: vi.fn(),
      reveal: vi.fn(),
      close,
    }),
  );
  const list = vi.fn<VaultClient["attachments"]>();
  const host = createSessionSourceHost(mode, {
    ...(scope ? { scope } : {}),
    features: ["attachment_info"],
    attachmentPresentation:
      mode === "online"
        ? scopedAttachmentPresentation
        : legacyAttachmentPresentation,
    openAttachment: open,
    attachments: list,
  });
  hosts.push(host);
  return { ...host, open, list, close };
}
beforeEach(() => {
  source.raw = JSON.stringify({
    type: "doc",
    content: [
      {
        type: "image",
        attrs: {
          attachmentId: "image.png",
          src: "asset:/old-private-source.png",
          alt: "Canonical image",
        },
      },
    ],
  });
});
afterEach(() => {
  cleanup();
  hosts.splice(0).forEach((host) => host.dispose());
});
it.each(["always", "online"] as const)(
  "resolves referenced images in %s mode by ID without a full attachment list or stored runtime URL",
  async (mode) => {
    const host = fixture(mode);
    const navigate = vi.fn();
    render(
      <host.Host>
        <MemoImageStrip sessionId="session-1" onImageClick={navigate} />
      </host.Host>,
    );
    const image = await screen.findByRole("img", { name: "Canonical image" });
    expect(image.getAttribute("src")).toBe("loofah-media://current-image");
    expect(host.open).toHaveBeenCalledWith(
      "session-1",
      "image.png",
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
    expect(host.list).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Show image in note" }));
    expect(navigate).toHaveBeenCalledWith(
      "loofah-media://current-image",
      "image.png",
    );
    expect(source.raw).toContain("asset:/old-private-source.png");
  },
);
it("never falls back to a persisted old resource URL while canonical resolution is unavailable", async () => {
  const host = fixture("online");
  let finish!: () => void;
  host.open.mockImplementationOnce(
    () =>
      new Promise((done) => {
        finish = () => done(null);
      }),
  );
  render(
    <host.Host>
      <MemoImageStrip sessionId="session-1" />
    </host.Host>,
  );
  await waitFor(() => expect(host.open).toHaveBeenCalledOnce());
  expect(screen.queryByRole("img")).toBeNull();
  await act(async () => finish());
  expect(screen.queryByRole("img")).toBeNull();
});
it.each([50, 150, 500])(
  "closes an old %ims thumbnail reply across same-ID/same-string Scope replacement",
  async (delay) => {
    const old = fixture("online", "same"),
      next = fixture("online", "same");
    old.open.mockImplementationOnce(async () => {
      await new Promise((done) => setTimeout(done, delay));
      return {
        id: "old",
        info: { filename: "image.png", contentType: "image/png", sizeBytes: 7 },
        src: "old-private-url",
        labels: { open: { id: "Open" }, reveal: { id: "Show" } },
        open: vi.fn(),
        reveal: vi.fn(),
        close: old.close,
      };
    });
    const view = render(
      <old.Host>
        <MemoImageStrip sessionId="session-1" />
      </old.Host>,
    );
    await waitFor(() => expect(old.open).toHaveBeenCalledOnce());
    old.scope.dispose();
    view.rerender(
      <next.Host>
        <MemoImageStrip sessionId="session-1" />
      </next.Host>,
    );
    await screen.findByRole("img");
    await act(async () => {
      await new Promise((done) => setTimeout(done, delay + 10));
    });
    expect(screen.getByRole("img").getAttribute("src")).toBe(
      "loofah-media://current-image",
    );
    expect(old.close).toHaveBeenCalledOnce();
  },
);
it("removes previews after terminal access loss and does not revive a retired host", async () => {
  const host = fixture("online");
  render(
    <host.Host>
      <MemoImageStrip sessionId="session-1" />
    </host.Host>,
  );
  await screen.findByRole("img");
  act(() =>
    host.emit({ kind: "connection", state: "login_required", error: null }),
  );
  await waitFor(() => expect(screen.queryByRole("img")).toBeNull());
  act(() => host.emit({ kind: "connection", state: "connected", error: null }));
  expect(screen.queryByRole("img")).toBeNull();
  expect(host.close).toHaveBeenCalledOnce();
});

import { beforeEach, describe, expect, it, vi } from "vitest";

const host = vi.hoisted(() => ({
  info: vi.fn(),
  directory: vi.fn(),
  open: vi.fn(),
  reveal: vi.fn(),
  uri: vi.fn(),
  join: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: { applicationAttachmentInfo: host.info },
}));
vi.mock("@hypr/plugin-fs-sync", () => ({
  commands: { attachmentDir: host.directory },
}));
vi.mock("@hypr/plugin-opener2", () => ({
  commands: { openPath: host.open, revealItemInDir: host.reveal },
}));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: host.uri }));
vi.mock("@tauri-apps/api/path", () => ({ join: host.join }));

import {
  openLocalAttachmentDirectory,
  openLocalAttachment,
  openRemoteAttachment,
} from "./attachments";
import type { NativeRemoteConnection } from "./remote";

const info = {
  filename: "chart ü.png",
  contentType: "image/png",
  sizeBytes: 42,
};
const id = "b7b8db23-ac32-49f7-96c2-f35363c668c7";
function remote() {
  const api = {
    request: vi.fn().mockResolvedValue(info),
    openAttachment: vi
      .fn()
      .mockResolvedValue({ id, preview_limit_bytes: 32 * 1024 * 1024 }),
    copyAttachment: vi.fn().mockResolvedValue({ copied_filename: "copy.png" }),
    closeAttachment: vi.fn().mockResolvedValue(undefined),
  };
  return { api, connection: api as unknown as NativeRemoteConnection };
}
beforeEach(() => {
  vi.resetAllMocks();
  host.info.mockResolvedValue({ status: "ok", data: info });
  host.directory.mockResolvedValue({
    status: "ok",
    data: "/client/vault/sessions/note/attachments",
  });
  host.join.mockImplementation(async (...parts: string[]) => parts.join("/"));
  host.uri.mockImplementation(
    (path: string, protocol = "asset") =>
      `${protocol}://localhost/${encodeURIComponent(path)}`,
  );
  host.open.mockResolvedValue({ status: "ok", data: null });
  host.reveal.mockResolvedValue({ status: "ok", data: null });
});

describe("client attachment resources", () => {
  it("keeps local assets on device without hashing, downloading or requiring a server", async () => {
    const resource = (await openLocalAttachment("note", info.filename))!;
    expect(host.info).toHaveBeenCalledWith("note", info.filename);
    expect(host.directory).toHaveBeenCalledWith("note");
    expect(resource.path).toBe(
      "/client/vault/sessions/note/attachments/chart ü.png",
    );
    expect(resource.src).toBe(
      "asset://localhost/%2Fclient%2Fvault%2Fsessions%2Fnote%2Fattachments%2Fchart%20%C3%BC.png",
    );
    expect(host.open).not.toHaveBeenCalled();
    await resource.open();
    await resource.reveal();
    expect(host.open).toHaveBeenCalledWith(resource.path, null);
    expect(host.reveal).toHaveBeenCalledWith(resource.path);
    expect(resource.save).toBeUndefined();
    await resource.close();
    await expect(resource.open()).rejects.toMatchObject({
      detail: { code: "cancelled" },
    });
    expect(host.open).toHaveBeenCalledTimes(1);
  });
  it("does not resolve device paths for missing or cancelled local metadata", async () => {
    host.info.mockResolvedValueOnce({ status: "ok", data: null });
    expect(await openLocalAttachment("note", "missing.png")).toBeNull();
    const cancel = new AbortController();
    host.info.mockImplementationOnce(async () => {
      cancel.abort();
      return { status: "ok", data: info };
    });
    await expect(
      openLocalAttachment("note", info.filename, { signal: cancel.signal }),
    ).rejects.toMatchObject({ name: "AbortError" });
    expect(host.directory).not.toHaveBeenCalled();
    expect(host.open).not.toHaveBeenCalled();
  });
  it("uses cheap remote resources and opaque native URIs, and delegates device copies explicitly", async () => {
    const { api, connection } = remote();
    const resource = (await openRemoteAttachment(
      connection,
      "vault",
      "note",
      info.filename,
    ))!;
    expect(api.request).toHaveBeenCalledWith(
      {
        kind: "attachment_info",
        vault_id: "vault",
        session_id: "note",
        attachment_id: info.filename,
      },
      "attachment_info",
      undefined,
    );
    expect(api.openAttachment).toHaveBeenCalledWith(
      "vault",
      "note",
      info.filename,
      undefined,
    );
    expect(resource.src).toBe(`loofah-media://localhost/attachments%2F${id}`);
    expect(resource.path).toBeUndefined();
    expect(api.copyAttachment).not.toHaveBeenCalled();
    const options = { signal: new AbortController().signal };
    await resource.open(options);
    await resource.reveal();
    await resource.save!();
    expect(api.copyAttachment.mock.calls).toEqual([
      [id, "open", options],
      [id, "reveal", undefined],
      [id, "save", undefined],
    ]);
    expect(host.directory).not.toHaveBeenCalled();
    expect(host.open).not.toHaveBeenCalled();
    await resource.close();
    await resource.close();
    expect(api.closeAttachment).toHaveBeenCalledTimes(1);
    await expect(resource.save!()).rejects.toMatchObject({
      detail: { code: "cancelled" },
    });
    expect(api.copyAttachment).toHaveBeenCalledTimes(3);
  });
  it("keeps original-copy actions for oversized images and nonimage files without fetching preview bytes", async () => {
    for (const value of [
      { ...info, sizeBytes: 33 * 1024 * 1024 },
      { ...info, contentType: "application/pdf" },
    ]) {
      const { api, connection } = remote();
      api.request.mockResolvedValue(value);
      const resource = (await openRemoteAttachment(
        connection,
        "vault",
        "note",
        info.filename,
      ))!;
      expect(resource.src).toBeUndefined();
      expect(api.copyAttachment).not.toHaveBeenCalled();
      await resource.save!();
      expect(api.copyAttachment).toHaveBeenCalledWith(id, "save", undefined);
      await resource.close();
    }
    const { api, connection } = remote();
    api.request.mockResolvedValue(null);
    expect(
      await openRemoteAttachment(connection, "vault", "note", "missing"),
    ).toBeNull();
    expect(api.openAttachment).not.toHaveBeenCalled();
  });
  it("closes a late native handle when cancellation or URI construction prevents delivery", async () => {
    const cancel = new AbortController();
    const { api, connection } = remote();
    api.openAttachment.mockImplementationOnce(async () => {
      cancel.abort();
      return { id, preview_limit_bytes: 32 * 1024 * 1024 };
    });
    await expect(
      openRemoteAttachment(connection, "vault", "note", info.filename, {
        signal: cancel.signal,
      }),
    ).rejects.toMatchObject({ name: "AbortError" });
    expect(api.closeAttachment).toHaveBeenCalledWith(id);
    const second = remote();
    host.uri.mockImplementationOnce(() => {
      throw new Error("Invalid native URI");
    });
    await expect(
      openRemoteAttachment(second.connection, "vault", "note", info.filename),
    ).rejects.toThrow("Invalid native URI");
    expect(second.api.closeAttachment).toHaveBeenCalledTimes(1);
    expect(second.api.copyAttachment).not.toHaveBeenCalled();
  });
});

it.each([
  "/client/vault/sessions/note/attachments",
  "C:\\\\Users\\\\me\\\\vault\\\\sessions\\\\note\\\\attachments",
])(
  "opens the captured local directory on either desktop platform: %s",
  async (path) => {
    host.directory.mockResolvedValueOnce({ status: "ok", data: path });
    await openLocalAttachmentDirectory("note");
    expect(host.open).toHaveBeenCalledWith(path, null);
  },
);
it("does not open a directory after cancellation, and reports native opener failures", async () => {
  const controller = new AbortController();
  host.directory.mockImplementationOnce(async () => {
    controller.abort();
    return { status: "ok", data: "/private" };
  });
  await expect(
    openLocalAttachmentDirectory("note", { signal: controller.signal }),
  ).rejects.toThrow();
  expect(host.open).not.toHaveBeenCalled();
  host.open.mockResolvedValueOnce({
    status: "error",
    error: "No opener available",
  });
  await expect(openLocalAttachmentDirectory("note")).rejects.toThrow(
    "No opener available",
  );
  const resource = await openLocalAttachment("note", info.filename);
  host.open.mockResolvedValueOnce({ status: "error", error: "Open failed" });
  await expect(resource!.open()).rejects.toThrow("Open failed");
  host.reveal.mockResolvedValueOnce({
    status: "error",
    error: "Reveal failed",
  });
  await expect(resource!.reveal()).rejects.toThrow("Reveal failed");
});

import {
  QueryClient,
  QueryClientProvider,
  onlineManager,
} from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";

import type { FileUploadCandidate } from "@hypr/editor/note";

import { ApplicationRequestError } from "~/shared/application/errors";
import type { VaultClient } from "~/shared/application/vault-client";
import { VaultScope } from "~/shared/application/vault-scope";

const host = vi.hoisted(() => ({ scope: null as VaultScope | null }));
vi.mock("~/shared/application/vault-provider", () => ({
  useVaultHost: () => host,
}));
vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `asset:${path}`,
}));

import { useFileUpload, useFileUploadStates } from "./useFileUpload";

const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  host.scope = null;
  onlineManager.setOnline(true);
});
const candidate: FileUploadCandidate = {
  kind: "path",
  path: "/device/diagram.png",
  name: "diagram.png",
};
async function fixture(name = "local-one") {
  const close = vi.fn().mockResolvedValue(undefined);
  const closeResource = vi.fn().mockResolvedValue(undefined);
  const prepare = vi.fn(async (source: FileUploadCandidate) => ({
    input: {
      filename: source.name,
      size_bytes: source.kind === "file" ? source.file.size : 4_000_000_000,
      sha256: "a".repeat(64),
    },
    source:
      source.kind === "path"
        ? { kind: "path" as const, path: source.path }
        : { kind: "staged" as const, id: crypto.randomUUID() },
    close,
  }));
  const upload = vi.fn(async (_session, source, _options) => ({
    filename: source.input.filename,
    sizeBytes: source.input.size_bytes,
    sha256: source.input.sha256,
    contentType: "image/png",
  }));
  const open = vi.fn(async (session, filename) => ({
    path: `/vault/${name}/sessions/${session}/attachments/${filename}`,
    close: closeResource,
  }));
  const scope = new VaultScope({
    scope: name,
    canEdit: true,
    networkMode: "always",
    features: ["attachment_upload"],
    prepareAttachmentSource: prepare,
    uploadAttachmentSource: upload,
    openAttachment: open,
    subscribe: async (listener: Parameters<VaultClient["subscribe"]>[0]) => {
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient);
  scopes.push(scope);
  await scope.start();
  host.scope = scope;
  const queryClient = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
  const hook = renderHook(
    () => ({
      upload: useFileUpload("session-1"),
      ...useFileUploadStates("session-1"),
    }),
    { wrapper },
  );
  return {
    ...hook,
    scope,
    queryClient: scope.queryClient,
    rootClient: queryClient,
    prepare,
    upload,
    open,
    close,
    closeResource,
  };
}

it("routes a browser File through shared source preparation without whole-file buffering, retaining local URLs and attachment cache behavior", async () => {
  const f = await fixture();
  const arrayBuffer = vi
    .fn()
    .mockRejectedValue(new Error("Whole-file reads are forbidden"));
  const file = {
    size: 7,
    name: "clipboard.png",
    arrayBuffer,
  } as unknown as File;
  let result;
  await act(async () => {
    result = await f.result.current.upload({
      kind: "file",
      file,
      name: file.name,
    });
  });
  expect(arrayBuffer).not.toHaveBeenCalled();
  expect(f.prepare).toHaveBeenCalledTimes(1);
  expect(f.upload).toHaveBeenCalledTimes(1);
  expect(result).toMatchObject({
    attachmentId: "clipboard.png",
    url: "",
    path: "",
    attachment: { sizeBytes: 7, filename: "clipboard.png" },
  });
  expect(f.rootClient.getQueryCache().getAll()).toEqual([]);
  expect(
    f.scope.queryClient.getQueryData([
      "vault",
      f.scope.client.scope,
      "attachment_list",
      "session-1",
    ]),
  ).toBeUndefined();
  expect(f.close).toHaveBeenCalledTimes(1);
  expect(f.open).not.toHaveBeenCalled();
  expect(f.closeResource).not.toHaveBeenCalled();
  expect(f.queryClient.getMutationCache().getAll()).toHaveLength(0);
});

it("keeps native paths on the device and uploads locally even when browser networking is offline", async () => {
  const f = await fixture();
  onlineManager.setOnline(false);
  await act(async () => {
    const result = await f.result.current.upload(candidate);
    expect(result.attachment.sizeBytes).toBe(4_000_000_000);
  });
  expect(f.prepare.mock.calls[0][0]).toEqual(candidate);
  expect(f.upload.mock.calls[0][1].source).toEqual({
    kind: "path",
    path: candidate.path,
  });
});

it("serializes imports in submission order", async () => {
  const f = await fixture();
  let finish!: (value: any) => void;
  f.upload.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  let first!: Promise<unknown>, second!: Promise<unknown>;
  act(() => {
    first = f.result.current.upload(candidate);
    second = f.result.current.upload({ ...candidate, name: "second.png" });
  });
  await waitFor(() => expect(f.upload).toHaveBeenCalledTimes(1));
  expect(f.prepare).toHaveBeenCalledTimes(1);
  await act(async () => {
    finish({
      filename: "diagram.png",
      sizeBytes: 4_000_000_000,
      sha256: "a".repeat(64),
      contentType: "image/png",
    });
    await Promise.all([first, second]);
  });
  expect(f.prepare.mock.calls.map(([source]) => source.name)).toEqual([
    "diagram.png",
    "second.png",
  ]);
});

it("retains the original declaration, source and UUID on a lost acknowledgement instead of preparing a different retry", async () => {
  const f = await fixture();
  const id = crypto.randomUUID();
  f.upload.mockRejectedValueOnce(
    new ApplicationRequestError({
      code: "unavailable",
      message: "Reply lost",
      request_id: id,
      retryable: true,
      current_revision: null,
      mutation_outcome: "unknown",
    }),
  );
  await act(async () => {
    await expect(f.result.current.upload(candidate, id)).rejects.toThrow(
      "Reply lost",
    );
  });
  await waitFor(() => expect(f.result.current.states).toHaveLength(1));
  expect(f.close).not.toHaveBeenCalled();
  await act(async () => {
    await f.result.current.upload(
      { ...candidate, path: "/changed.bin", name: "changed.bin" },
      id,
    );
  });
  expect(f.prepare).toHaveBeenCalledTimes(1);
  expect(f.upload).toHaveBeenCalledTimes(2);
  expect(f.upload.mock.calls[0][1].input).toEqual(
    f.upload.mock.calls[1][1].input,
  );
  expect(f.upload.mock.calls[0][1].source).toEqual(
    f.upload.mock.calls[1][1].source,
  );
  expect(f.upload.mock.calls.map((call) => call[2].requestId)).toEqual([
    id,
    id,
  ]);
  await waitFor(() => expect(f.result.current.states).toHaveLength(0));
  expect(f.close).toHaveBeenCalledTimes(1);
});

it("refuses work before a host is ready and never falls back to a different vault", async () => {
  const f = await fixture();
  host.scope = null;
  expect(() => f.rerender()).toThrow("immutable vault host");
  expect(f.prepare).not.toHaveBeenCalled();
  expect(f.upload).not.toHaveBeenCalled();
});

it("clears retained host upload attempts when their scope ends, including after the UI unmounts, without clearing unrelated mutations", async () => {
  const f = await fixture();
  f.upload.mockRejectedValue(new Error("Reply lost"));
  await act(async () => {
    await expect(f.result.current.upload(candidate)).rejects.toThrow();
  });
  const other = f.rootClient.getMutationCache().build(f.rootClient, {
    mutationKey: ["unrelated"],
    mutationFn: async () => {},
  });
  f.unmount();
  f.scope.dispose();
  expect(f.close).toHaveBeenCalledTimes(1);
  expect(f.queryClient.getMutationCache().getAll()).toEqual([]);
  expect(f.rootClient.getMutationCache().getAll()).toEqual([other]);
});

it("returns the server-owned canonical filename without requiring any local resource path", async () => {
  const f = await fixture();
  f.upload.mockResolvedValueOnce({
    filename: "diagram 1.png",
    contentType: "image/png",
    sizeBytes: 7,
    sha256: "a".repeat(64),
  });
  await act(async () => {
    expect(await f.result.current.upload(candidate)).toMatchObject({
      attachmentId: "diagram 1.png",
      url: "",
      path: "",
    });
  });
  expect(f.open).not.toHaveBeenCalled();
});

it("never inserts a completed old upload into a replacement host with identical session IDs", async () => {
  const old = await fixture("same-scope");
  let finish!: () => void;
  old.upload.mockImplementationOnce(
    () =>
      new Promise((done) => {
        finish = () =>
          done({
            filename: "old.png",
            contentType: "image/png",
            sizeBytes: 7,
            sha256: "a".repeat(64),
          });
      }),
  );
  let pending!: Promise<unknown>;
  act(() => {
    pending = old.result.current.upload(candidate);
  });
  const refused = expect(pending).rejects.toMatchObject({
    detail: { mutation_outcome: "unknown" },
  });
  await waitFor(() => expect(old.upload).toHaveBeenCalledOnce());
  const next = await fixture("same-scope");
  old.rerender();
  await act(async () => {
    finish();
    await refused;
  });
  expect(
    next.scope.queryClient.getQueryData([
      "vault",
      next.scope.client.scope,
      "attachment_list",
      "session-1",
    ]),
  ).toBeUndefined();
  expect(next.prepare).not.toHaveBeenCalled();
  expect(next.upload).not.toHaveBeenCalled();
});

import { onlineManager } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import type { FileUploadCandidate } from "@hypr/editor/note";

import type { PreparedAttachmentUpload } from "./device-attachment-source";
import { ApplicationRequestError } from "./errors";
import type { FileMetadata } from "./protocol.gen";
import {
  useVaultFileUpload,
  useVaultFileUploadStates,
} from "./use-vault-file-upload";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultScope } from "./vault-scope";

const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  onlineManager.setOnline(true);
});
const candidate: FileUploadCandidate = {
  kind: "file",
  name: "clipboard.png",
  file: new File(["payload"], "clipboard.png"),
};
const metadata: FileMetadata = {
  filename: "clipboard 1.png",
  sizeBytes: 7,
  contentType: "image/png",
  sha256: "1".repeat(64),
};
function failure() {
  return new ApplicationRequestError({
    code: "unavailable",
    message: "Reply lost",
    request_id: null,
    retryable: true,
    current_revision: null,
    mutation_outcome: "unknown",
  });
}
async function fixture(name = "first", mode: "always" | "online" = "always") {
  let event!: (value: VaultClientEvent) => void;
  const close = vi.fn().mockResolvedValue(undefined);
  const prepared: PreparedAttachmentUpload = {
    input: { filename: candidate.name, size_bytes: 7, sha256: metadata.sha256 },
    source: { kind: "staged", id: crypto.randomUUID() },
    close,
  };
  const prepareAttachmentSource = vi.fn().mockResolvedValue(prepared);
  const uploadAttachmentSource = vi.fn().mockResolvedValue(metadata);
  const client = {
    scope: name,
    networkMode: mode,
    canEdit: true,
    features: ["attachment_upload"],
    prepareAttachmentSource,
    uploadAttachmentSource,
    subscribe: async (listener: (value: VaultClientEvent) => void) => {
      event = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  scopes.push(scope);
  await scope.start();
  const hook = renderHook(() => ({
    upload: useVaultFileUpload(scope, "note"),
    ...useVaultFileUploadStates(scope, "note"),
  }));
  return {
    ...hook,
    scope,
    prepared,
    prepareAttachmentSource,
    uploadAttachmentSource,
    close,
    emit: (value: VaultClientEvent) => act(() => event(value)),
  };
}

it("retains the exact source/declaration/UUID after a lost reply and releases it only after an explicit successful retry", async () => {
  const f = await fixture();
  const id = crypto.randomUUID();
  f.uploadAttachmentSource.mockRejectedValueOnce(failure());
  await act(async () => {
    await expect(f.result.current.upload(candidate, id)).rejects.toMatchObject({
      message: "Reply lost",
    });
  });
  await waitFor(() => expect(f.result.current.states).toHaveLength(1));
  expect(f.result.current.states[0].clientId).toBe(id);
  expect(f.close).not.toHaveBeenCalled();
  expect(f.uploadAttachmentSource).toHaveBeenCalledTimes(1);
  await act(async () => {
    expect(
      await f.result.current.upload(
        { kind: "path", name: "different.bin", path: "/different.bin" },
        id,
      ),
    ).toEqual(metadata);
  });
  expect(f.prepareAttachmentSource).toHaveBeenCalledTimes(1);
  for (const [session, source, options] of f.uploadAttachmentSource.mock
    .calls) {
    expect(session).toBe("note");
    expect(source.input).toEqual(f.prepared.input);
    expect(source.source).toEqual(f.prepared.source);
    expect(options.requestId).toBe(id);
  }
  expect(f.close).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(f.result.current.states).toHaveLength(0));
  expect(f.scope.queryClient.getMutationCache().getAll()).toHaveLength(0);
});

it("never queues new work while disconnected and still permits connected local work when browser networking is offline", async () => {
  const f = await fixture();
  f.scope.queryClient.setDefaultOptions({
    mutations: { retry: 3, networkMode: "online" },
  });
  f.emit({ kind: "connection", state: "reconnecting", error: null });
  await act(async () => {
    await expect(f.result.current.upload(candidate)).rejects.toThrow();
  });
  expect(f.prepareAttachmentSource).not.toHaveBeenCalled();
  f.emit({ kind: "connection", state: "connected", error: null });
  onlineManager.setOnline(false);
  await act(async () => {
    expect(await f.result.current.upload(candidate)).toEqual(metadata);
  });
  expect(f.uploadAttachmentSource).toHaveBeenCalledTimes(1);
});

it("closes late preparation after scope disposal without dispatching a vault mutation", async () => {
  const f = await fixture();
  let finish!: (source: PreparedAttachmentUpload) => void;
  f.prepareAttachmentSource.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  let pending!: Promise<unknown>;
  act(() => {
    pending = f.result.current.upload(candidate).catch((cause) => cause);
  });
  await waitFor(() =>
    expect(f.prepareAttachmentSource).toHaveBeenCalledTimes(1),
  );
  act(() => f.scope.dispose());
  await act(async () => {
    finish(f.prepared);
    expect(await pending).toBeInstanceOf(ApplicationRequestError);
  });
  expect(f.close).toHaveBeenCalledTimes(1);
  expect(f.uploadAttachmentSource).not.toHaveBeenCalled();
});

it("cancels and closes an active source on access loss and refuses a late success without touching another account", async () => {
  const f = await fixture();
  const other = await fixture("other-account");
  let finish!: (value: FileMetadata) => void;
  f.uploadAttachmentSource.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  let pending!: Promise<unknown>;
  act(() => {
    pending = f.result.current.upload(candidate).catch((cause) => cause);
  });
  await waitFor(() =>
    expect(f.uploadAttachmentSource).toHaveBeenCalledTimes(1),
  );
  const signal = f.uploadAttachmentSource.mock.calls[0][2].signal;
  f.emit({ kind: "connection", state: "access_denied", error: null });
  expect(signal.aborted).toBe(true);
  expect(f.close).toHaveBeenCalledTimes(1);
  await act(async () => {
    finish(metadata);
    expect(await pending).toMatchObject({
      detail: { code: "cancelled", mutation_outcome: "unknown" },
    });
  });
  expect(other.result.current.states).toHaveLength(0);
  expect(other.uploadAttachmentSource).not.toHaveBeenCalled();
});

it("keeps failed attempts visible until explicit removal and closes their private source once", async () => {
  const f = await fixture();
  const id = crypto.randomUUID();
  f.uploadAttachmentSource.mockRejectedValue(failure());
  await act(async () => {
    await expect(f.result.current.upload(candidate, id)).rejects.toThrow();
  });
  await waitFor(() => expect(f.result.current.states).toHaveLength(1));
  act(() => f.result.current.remove(id));
  await waitFor(() => expect(f.result.current.states).toHaveLength(0));
  expect(f.close).toHaveBeenCalledTimes(1);
  f.scope.dispose();
  expect(f.close).toHaveBeenCalledTimes(1);
});

it("refuses remote offline writes before preparation and again after asynchronous source staging", async () => {
  const f = await fixture("remote", "online");
  onlineManager.setOnline(false);
  await expect(f.result.current.upload(candidate)).rejects.toThrow();
  expect(f.prepareAttachmentSource).not.toHaveBeenCalled();
  onlineManager.setOnline(true);
  let finish!: () => void;
  f.prepareAttachmentSource.mockImplementationOnce(
    () =>
      new Promise((done) => {
        finish = () => done(f.prepared);
      }),
  );
  let attempt!: Promise<unknown>;
  act(() => {
    attempt = f.result.current.upload(candidate);
  });
  const failed = expect(attempt).rejects.toThrow();
  await waitFor(() => expect(f.prepareAttachmentSource).toHaveBeenCalledOnce());
  onlineManager.setOnline(false);
  await act(async () => {
    finish();
    await failed;
  });
  expect(f.uploadAttachmentSource).not.toHaveBeenCalled();
  expect(f.close).not.toHaveBeenCalled();
  f.scope.dispose();
  expect(f.close).toHaveBeenCalledOnce();
});

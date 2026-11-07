import { beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  devicePrepareAttachmentUpload: vi.fn(),
  deviceAttachmentSourceBegin: vi.fn(),
  deviceAttachmentSourceAppend: vi.fn(),
  deviceAttachmentSourceFinish: vi.fn(),
  deviceAttachmentSourceClose: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({ commands: native }));

import { prepareDeviceAttachmentSource } from "./device-attachment-source";

const input = {
  filename: "clipboard.png",
  size_bytes: 2 * 64 * 1024 + 17,
  sha256: "1".repeat(64),
};
beforeEach(() => {
  vi.resetAllMocks();
  native.deviceAttachmentSourceBegin.mockResolvedValue({
    status: "ok",
    data: null,
  });
  native.deviceAttachmentSourceAppend.mockImplementation(
    async (_id, offset, data) => ({ status: "ok", data: offset + data.length }),
  );
  native.deviceAttachmentSourceFinish.mockResolvedValue({
    status: "ok",
    data: input,
  });
  native.deviceAttachmentSourceClose.mockResolvedValue(undefined);
});
function file(bytes: Uint8Array) {
  return {
    name: input.filename,
    size: bytes.length,
    arrayBuffer: vi.fn(() => {
      throw new Error("Whole-file reads are forbidden");
    }),
    slice: vi.fn((start: number, end: number) => ({
      arrayBuffer: async () => bytes.slice(start, end).buffer,
    })),
  } as unknown as File;
}

it("stages browser/clipboard files in bounded sequential chunks and returns an opaque closable source", async () => {
  const bytes = new Uint8Array(input.size_bytes).fill(7);
  const source = file(bytes);
  const prepared = await prepareDeviceAttachmentSource({
    kind: "file",
    name: input.filename,
    file: source,
  });
  expect(prepared.input).toEqual(input);
  expect(prepared.source).toEqual({ kind: "staged", id: expect.any(String) });
  const id = (prepared.source as { id: string }).id;
  expect(native.deviceAttachmentSourceBegin).toHaveBeenCalledWith(
    id,
    input.filename,
    bytes.length,
  );
  expect(
    native.deviceAttachmentSourceAppend.mock.calls.map(([, offset, data]) => [
      offset,
      data.length,
    ]),
  ).toEqual([
    [0, 65536],
    [65536, 65536],
    [131072, 17],
  ]);
  expect(
    native.deviceAttachmentSourceAppend.mock.calls.flatMap(
      ([, , data]) => data,
    ),
  ).toEqual(Array.from(bytes));
  expect(source.arrayBuffer).not.toHaveBeenCalled();
  expect(native.deviceAttachmentSourceClose).not.toHaveBeenCalled();
  await prepared.close();
  await prepared.close();
  expect(native.deviceAttachmentSourceClose).toHaveBeenCalledTimes(1);
  expect(native.deviceAttachmentSourceClose).toHaveBeenCalledWith(id);
});

it("reviews native paths without creating or staging a browser-file source", async () => {
  native.devicePrepareAttachmentUpload.mockResolvedValue({
    status: "ok",
    data: input,
  });
  const prepared = await prepareDeviceAttachmentSource({
    kind: "path",
    path: "/device/clipboard.png",
    name: input.filename,
  });
  expect(prepared.source).toEqual({
    kind: "path",
    path: "/device/clipboard.png",
  });
  expect(native.devicePrepareAttachmentUpload).toHaveBeenCalledWith(
    "/device/clipboard.png",
  );
  await prepared.close();
  expect(native.deviceAttachmentSourceBegin).not.toHaveBeenCalled();
  expect(native.deviceAttachmentSourceClose).not.toHaveBeenCalled();
});

it("finishes empty files without emitting any chunks", async () => {
  native.deviceAttachmentSourceFinish.mockResolvedValue({
    status: "ok",
    data: { ...input, size_bytes: 0 },
  });
  const prepared = await prepareDeviceAttachmentSource({
    kind: "file",
    name: input.filename,
    file: file(new Uint8Array()),
  });
  expect(prepared.input.size_bytes).toBe(0);
  expect(native.deviceAttachmentSourceAppend).not.toHaveBeenCalled();
  await prepared.close();
});

it.each([
  "begin",
  "append",
  "finish",
  "offset",
  "filename",
  "size",
  "hash",
  "slice",
])("cleans private source ownership after a %s failure", async (mode) => {
  const fail = {
    status: "error",
    error: {
      code: "unavailable",
      message: "Injected source failure",
      retryable: false,
      request_id: null,
      current_revision: null,
      mutation_outcome: null,
    },
  };
  const source = file(new Uint8Array(input.size_bytes));
  if (mode === "begin")
    native.deviceAttachmentSourceBegin.mockResolvedValue(fail);
  if (mode === "append")
    native.deviceAttachmentSourceAppend.mockResolvedValue(fail);
  if (mode === "finish")
    native.deviceAttachmentSourceFinish.mockResolvedValue(fail);
  if (mode === "offset")
    native.deviceAttachmentSourceAppend.mockResolvedValue({
      status: "ok",
      data: 1,
    });
  if (mode === "filename")
    native.deviceAttachmentSourceFinish.mockResolvedValue({
      status: "ok",
      data: { ...input, filename: "wrong.png" },
    });
  if (mode === "size")
    native.deviceAttachmentSourceFinish.mockResolvedValue({
      status: "ok",
      data: { ...input, size_bytes: 1 },
    });
  if (mode === "hash")
    native.deviceAttachmentSourceFinish.mockResolvedValue({
      status: "ok",
      data: { ...input, sha256: "A".repeat(64) },
    });
  if (mode === "slice")
    vi.mocked(source.slice).mockReturnValue({
      arrayBuffer: async () => new ArrayBuffer(1),
    } as Blob);
  await expect(
    prepareDeviceAttachmentSource({
      kind: "file",
      name: input.filename,
      file: source,
    }),
  ).rejects.toThrow();
  expect(native.deviceAttachmentSourceClose).toHaveBeenCalledTimes(1);
});

it("refuses already-cancelled work and cleans a cancellation between chunk acknowledgements", async () => {
  const source = file(new Uint8Array(input.size_bytes));
  await expect(
    prepareDeviceAttachmentSource(
      { kind: "file", name: input.filename, file: source },
      { signal: AbortSignal.abort() },
    ),
  ).rejects.toThrow();
  expect(native.deviceAttachmentSourceBegin).not.toHaveBeenCalled();
  const controller = new AbortController();
  native.deviceAttachmentSourceAppend.mockImplementation(
    async (_id, offset, data) => {
      controller.abort();
      return { status: "ok", data: offset + data.length };
    },
  );
  await expect(
    prepareDeviceAttachmentSource(
      { kind: "file", name: input.filename, file: source },
      { signal: controller.signal },
    ),
  ).rejects.toThrow();
  expect(native.deviceAttachmentSourceAppend).toHaveBeenCalledTimes(1);
  expect(native.deviceAttachmentSourceFinish).not.toHaveBeenCalled();
  expect(native.deviceAttachmentSourceClose).toHaveBeenCalledTimes(1);
});

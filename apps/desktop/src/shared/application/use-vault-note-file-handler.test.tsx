import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  upload: vi.fn(),
  remove: vi.fn(),
  native: vi.fn(),
  drop: vi.fn(),
  drag: vi.fn(),
}));
vi.mock("./use-vault-file-upload", () => ({
  useVaultFileUpload: () => mocks.upload,
  useVaultFileUploadStates: () => ({ remove: mocks.remove }),
}));
vi.mock("~/shared/hooks/useNativeFileDrop", () => ({
  useNativeFileDrop: mocks.native,
}));
vi.mock("@hypr/editor/note", () => ({
  handleNativeFileDrop: mocks.drop,
  handleNativeEditorDrag: mocks.drag,
}));

import { useVaultNoteFileHandler } from "./use-vault-note-file-handler";
import type { VaultScope } from "./vault-scope";

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

it("returns canonical attachment IDs for editor nodes without persisting runtime URLs or device paths", async () => {
  mocks.upload.mockResolvedValue({ filename: "diagram 1.png" });
  const { result } = renderHook(() =>
    useVaultNoteFileHandler({} as VaultScope, "note", true),
  );
  const candidate = {
    kind: "file" as const,
    name: "diagram.png",
    file: new File(["image"], "diagram.png"),
  };
  const id = crypto.randomUUID();
  expect(
    await result.current.fileHandlerConfig?.onFileUpload?.(candidate, id),
  ).toEqual({ attachmentId: "diagram 1.png", url: "", path: "" });
  expect(mocks.upload).toHaveBeenCalledWith(candidate, id);
  result.current.fileHandlerConfig?.onFileUploadRemove?.(id);
  expect(mocks.remove).toHaveBeenCalledWith(id);
});

it("routes native paths and CSS coordinates through the existing editor drop adapter and preserves host view callbacks", () => {
  const onViewReady = vi.fn(),
    onViewDisposed = vi.fn();
  const { result } = renderHook(() =>
    useVaultNoteFileHandler({} as VaultScope, "note", true, {
      onViewReady,
      onViewDisposed,
    }),
  );
  const view = {
    editable: true,
    posAtCoords: vi.fn().mockReturnValue({ pos: 42 }),
    state: { doc: { content: { size: 100 } } },
  };
  act(() => result.current.onViewReady?.(view as never));
  expect(onViewReady).toHaveBeenCalledWith(view);
  const callbacks = mocks.native.mock.calls.slice(-1)[0]![1];
  callbacks.onDrop(["C:\\Users\\Test\\diagram.png", "/device/spec.pdf"], {
    x: 10,
    y: 20,
  });
  expect(view.posAtCoords).toHaveBeenCalledWith({ left: 10, top: 20 });
  expect(mocks.drop).toHaveBeenCalledWith(
    view,
    result.current.fileHandlerConfig,
    [
      {
        kind: "path",
        path: "C:\\Users\\Test\\diagram.png",
        name: "diagram.png",
      },
      { kind: "path", path: "/device/spec.pdf", name: "spec.pdf" },
    ],
    42,
  );
  callbacks.onInternalDrag({ type: "over", point: { x: 10, y: 20 } });
  expect(mocks.drag).toHaveBeenCalledWith(view, {
    type: "over",
    point: { x: 10, y: 20 },
  });
  act(() => result.current.onViewDisposed?.(view as never));
  expect(onViewDisposed).toHaveBeenCalledWith(view);
  callbacks.onDrop(["/late.pdf"], { x: 10, y: 20 });
  expect(mocks.drop).toHaveBeenCalledTimes(1);
});

it("disables the native listener and editor file handler when the host provides its existing handler", () => {
  const { result } = renderHook(() =>
    useVaultNoteFileHandler({} as VaultScope, "note", false),
  );
  expect(result.current.fileHandlerConfig).toBeUndefined();
  expect(mocks.native.mock.calls.slice(-1)[0]![1].enabled).toBe(false);
});

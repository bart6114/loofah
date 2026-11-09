import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ open: vi.fn(), drop: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open }));
vi.mock("~/shared/hooks/useNativeFileDrop", () => ({
  useNativeFileDrop: mocks.drop,
}));

import { ApplicationRequestError } from "./errors";
import { VaultAttachmentsPane } from "./vault-attachments-pane";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultScope } from "./vault-scope";

const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  vi.resetAllMocks();
});
const metadata = {
  filename: "diagram 1.png",
  contentType: "image/png",
  sizeBytes: 7,
  sha256: "1".repeat(64),
};
async function fixture(canEdit = true) {
  let event!: (value: VaultClientEvent) => void;
  const files: Array<typeof metadata> = [];
  const close = vi.fn().mockResolvedValue(undefined);
  const prepared = {
    input: { filename: "diagram.png", size_bytes: 7, sha256: metadata.sha256 },
    source: { kind: "path", path: "/device/diagram.png" },
    close,
  };
  const prepareAttachmentSource = vi.fn().mockResolvedValue(prepared);
  const uploadAttachmentSource = vi.fn().mockImplementation(async () => {
    files.push(metadata);
    return metadata;
  });
  const client = {
    scope: "uploads",
    networkMode: "always",
    canEdit,
    features: ["attachment_upload", "attachment_list"],
    prepareAttachmentSource,
    uploadAttachmentSource,
    attachments: vi.fn(async () => ({ files: [...files], next_cursor: null })),
    openAttachment: vi.fn().mockResolvedValue(null),
    subscribe: async (listener: (value: VaultClientEvent) => void) => {
      event = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  scopes.push(scope);
  await scope.start();
  render(<VaultAttachmentsPane scope={scope} sessionId="note" />);
  await screen.findByText("No attachments yet");
  return {
    scope,
    client,
    files,
    close,
    prepared,
    prepareAttachmentSource,
    uploadAttachmentSource,
    emit: (value: VaultClientEvent) => act(() => event(value)),
  };
}

it("chooses native files and refreshes the scoped attachment list after publication", async () => {
  const f = await fixture();
  mocks.open.mockResolvedValue(["/device/diagram.png"]);
  fireEvent.click(screen.getByRole("button", { name: "Add attachments" }));
  expect(await screen.findByText(metadata.filename)).toBeTruthy();
  expect(mocks.open).toHaveBeenCalledWith({
    multiple: true,
    directory: false,
    title: "Add attachments",
  });
  expect(f.prepareAttachmentSource).toHaveBeenCalledWith(
    { kind: "path", path: "/device/diagram.png", name: "diagram.png" },
    { signal: expect.any(AbortSignal) },
  );
  expect(f.uploadAttachmentSource).toHaveBeenCalledWith(
    "note",
    expect.objectContaining({
      input: f.prepared.input,
      source: f.prepared.source,
    }),
    { requestId: expect.any(String), signal: expect.any(AbortSignal) },
  );
  expect(f.close).toHaveBeenCalledTimes(1);
});

it("keeps an uncertain attempt visible after its file appears and retries with the exact original identity", async () => {
  const f = await fixture();
  mocks.open.mockResolvedValue(["/device/diagram.png"]);
  f.uploadAttachmentSource
    .mockImplementationOnce(async () => {
      f.files.push(metadata);
      f.emit({
        kind: "change",
        change: {
          kind: "index_changed",
          entity: "artifacts",
          ids: ["note"],
          artifacts: [
            {
              session_id: "note",
              target: { kind: "attachment", attachment_id: metadata.filename },
            },
          ],
        },
      });
      throw new ApplicationRequestError({
        code: "unavailable",
        message: "Reply lost",
        request_id: null,
        retryable: true,
        current_revision: null,
        mutation_outcome: "unknown",
      });
    })
    .mockResolvedValueOnce(metadata);
  fireEvent.click(screen.getByRole("button", { name: "Add attachments" }));
  expect(await screen.findByText("Reply lost")).toBeTruthy();
  expect(await screen.findByText(metadata.filename)).toBeTruthy();
  expect(f.close).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Retry diagram.png" }));
  await waitFor(() => expect(screen.queryByText("Reply lost")).toBeNull());
  expect(f.prepareAttachmentSource).toHaveBeenCalledTimes(1);
  const calls = f.uploadAttachmentSource.mock.calls;
  expect(calls).toHaveLength(2);
  expect(calls[1][2].requestId).toBe(calls[0][2].requestId);
  expect(calls[1][1].input).toEqual(calls[0][1].input);
  expect(f.files).toHaveLength(1);
  expect(f.close).toHaveBeenCalledTimes(1);
});

it("uses the same upload flow for native drops and disables selection while disconnected", async () => {
  const f = await fixture();
  const callback = mocks.drop.mock.calls.slice(-1)[0]![1];
  act(() => callback.onDrop(["C:\\device\\diagram.png"], { x: 20, y: 30 }));
  await screen.findByText(metadata.filename);
  expect(f.prepareAttachmentSource.mock.calls[0][0]).toEqual({
    kind: "path",
    path: "C:\\device\\diagram.png",
    name: "diagram.png",
  });
  f.emit({ kind: "connection", state: "reconnecting", error: null });
  expect(
    (
      screen.getByRole("button", {
        name: "Add attachments",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  expect(mocks.drop.mock.calls.slice(-1)[0]![1].enabled).toBe(false);
});

it("does not expose upload controls to readers", async () => {
  const f = await fixture(false);
  expect(screen.queryByRole("button", { name: "Add attachments" })).toBeNull();
  expect(f.prepareAttachmentSource).not.toHaveBeenCalled();
});

it("allows media-only device staging without granting attachment publication", async () => {
  const f = await fixture();
  Object.assign(f.client, { features: ["media_jobs"] });
  const prepared = await f.scope.prepareAttachmentSource({
    kind: "path",
    path: "/device/diagram.png",
    name: "diagram.png",
  });
  expect(f.prepareAttachmentSource).toHaveBeenCalledOnce();
  await expect(
    f.scope.uploadAttachmentSource("note", prepared, "original"),
  ).rejects.toThrow("Attachment upload is unavailable");
  expect(f.uploadAttachmentSource).not.toHaveBeenCalled();
  await prepared.close();
  expect(f.close).toHaveBeenCalledOnce();
});

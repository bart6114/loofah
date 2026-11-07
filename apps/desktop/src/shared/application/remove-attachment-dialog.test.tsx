import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { ApplicationRequestError } from "./errors";
import { RemoveAttachmentDialog } from "./remove-attachment-dialog";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultScope } from "./vault-scope";

const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
});
const metadata = {
  filename: "file.pdf",
  contentType: "application/pdf",
  sizeBytes: 1536,
  sha256: "0".repeat(64),
};

async function fixture(canEdit = true) {
  let emit!: (value: VaultClientEvent) => void;
  const attachmentMetadata = vi.fn().mockResolvedValue(metadata);
  const removeAttachment = vi.fn().mockResolvedValue(true);
  const scope = new VaultScope({
    scope: "test",
    networkMode: "always",
    canEdit,
    features: ["attachment_metadata", "attachment_remove"],
    attachmentMetadata,
    removeAttachment,
    subscribe: async (listener: Parameters<VaultClient["subscribe"]>[0]) => {
      emit = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient);
  scope.queryClient.setDefaultOptions({
    queries: { retry: false, networkMode: "always" },
  });
  scopes.push(scope);
  await scope.start();
  const onClose = vi.fn();
  const onRemoved = vi.fn();
  const mount = () =>
    render(
      <RemoveAttachmentDialog
        scope={scope}
        sessionId="note"
        attachmentId="file.pdf"
        onClose={onClose}
        onRemoved={onRemoved}
      />,
    );
  return {
    scope,
    attachmentMetadata,
    removeAttachment,
    onClose,
    onRemoved,
    mount,
    emit: (event: VaultClientEvent) => emit(event),
  };
}

it("waits for file review and sends its exact revision with one explicit request UUID", async () => {
  const f = await fixture();
  let resolve!: (value: typeof metadata) => void;
  f.attachmentMetadata.mockImplementation(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  f.mount();
  expect(screen.getByText("Checking attachment…")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Move to Trash" })).toBeNull();
  expect(f.removeAttachment).not.toHaveBeenCalled();
  await act(async () => resolve(metadata));
  expect(await screen.findByText("1.5 KB · application/pdf")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Move to Trash" }));
  await waitFor(() => expect(f.onRemoved).toHaveBeenCalledTimes(1));
  expect(f.removeAttachment).toHaveBeenCalledWith(
    "note",
    "file.pdf",
    { expected_sha256: metadata.sha256 },
    { requestId: expect.any(String) },
  );
  expect(f.removeAttachment.mock.calls[0][3].requestId).toMatch(
    /^[0-9a-f-]{36}$/,
  );
});

it("never adopts changed bytes behind an already displayed confirmation", async () => {
  const f = await fixture();
  f.mount();
  await screen.findByRole("button", { name: "Move to Trash" });
  act(() =>
    f.scope.queryClient.setQueryData(
      f.scope.attachmentMetadataOptions("note", "file.pdf").queryKey,
      { ...metadata, sha256: "1".repeat(64) },
    ),
  );
  await screen.findByText(
    "The attachment changed. Close this dialog and review it again.",
  );
  expect(
    (screen.getByRole("button", { name: "Move to Trash" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(f.removeAttachment).not.toHaveBeenCalled();
});

it("refuses confirmation from cached metadata when the current review cannot be refreshed", async () => {
  const f = await fixture();
  f.mount();
  await screen.findByRole("button", { name: "Move to Trash" });
  f.attachmentMetadata.mockRejectedValue(
    new Error("Disconnected during review"),
  );
  await act(async () =>
    f.scope.queryClient.invalidateQueries({
      queryKey: f.scope.attachmentMetadataOptions("note", "file.pdf").queryKey,
    }),
  );
  await screen.findByText(
    "Couldn’t check the attachment. Close this dialog and review it again.",
  );
  expect(
    (screen.getByRole("button", { name: "Move to Trash" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(f.removeAttachment).not.toHaveBeenCalled();
});

it("blocks closing while a confirmed request is pending and never retries an uncertain reply", async () => {
  const f = await fixture();
  let reject!: (error: Error) => void;
  f.removeAttachment.mockImplementation(
    () =>
      new Promise((_done, fail) => {
        reject = fail;
      }),
  );
  f.mount();
  fireEvent.click(await screen.findByRole("button", { name: "Move to Trash" }));
  await screen.findByText("Removing…");
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(f.onClose).not.toHaveBeenCalled();
  await act(async () =>
    reject(
      new ApplicationRequestError({
        code: "unavailable",
        message: "Lost acknowledgement",
        retryable: true,
        request_id: null,
        current_revision: null,
        mutation_outcome: "unknown",
      }),
    ),
  );
  await screen.findByText(
    "Removal could not be confirmed. Check the attachment and vault Trash before trying again.",
  );
  expect(
    (screen.getByRole("button", { name: "Move to Trash" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(f.removeAttachment).toHaveBeenCalledTimes(1);
  expect(f.onRemoved).not.toHaveBeenCalled();
});

it("requires fresh review after a known refusal and hides actions on access loss", async () => {
  const f = await fixture();
  f.removeAttachment.mockRejectedValue(
    new ApplicationRequestError({
      code: "conflict",
      message: "File changed",
      retryable: false,
      request_id: null,
      current_revision: null,
      mutation_outcome: "not_committed",
    }),
  );
  f.mount();
  fireEvent.click(await screen.findByRole("button", { name: "Move to Trash" }));
  await screen.findByText(
    "The attachment was not removed. Close this dialog and review it again.",
  );
  act(() =>
    f.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  await screen.findByText(
    "Attachment access is unavailable. Reconnect or sign in again.",
  );
  expect(screen.queryByRole("button", { name: "Move to Trash" })).toBeNull();
  expect(f.removeAttachment).toHaveBeenCalledTimes(1);
});

it("retains uncertain feedback if a change notification removes the file before its reply arrives", async () => {
  const f = await fixture();
  let reject!: (cause: Error) => void;
  f.removeAttachment.mockImplementation(
    () =>
      new Promise((_done, fail) => {
        reject = fail;
      }),
  );
  f.mount();
  fireEvent.click(await screen.findByRole("button", { name: "Move to Trash" }));
  await screen.findByText("Removing…");
  act(() =>
    f.scope.queryClient.setQueryData(
      f.scope.attachmentMetadataOptions("note", "file.pdf").queryKey,
      null,
    ),
  );
  await screen.findByText("This attachment is no longer available.");
  expect(
    (screen.getByText("Close", { selector: "button" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  await act(async () => reject(new Error("Lost reply after commit")));
  await screen.findByText(
    "Removal could not be confirmed. Check the attachment and vault Trash before trying again.",
  );
  expect(f.removeAttachment).toHaveBeenCalledTimes(1);
  expect(f.onRemoved).not.toHaveBeenCalled();
});

it("does not hash files for a reader and aborts an unfinished metadata read on unmount", async () => {
  const reader = await fixture(false);
  const mounted = reader.mount();
  expect(reader.attachmentMetadata).not.toHaveBeenCalled();
  mounted.unmount();
  const editor = await fixture();
  editor.attachmentMetadata.mockImplementation(() => new Promise(() => {}));
  const pending = editor.mount();
  await waitFor(() =>
    expect(editor.attachmentMetadata).toHaveBeenCalledTimes(1),
  );
  const signal = editor.attachmentMetadata.mock.calls[0][2]
    .signal as AbortSignal;
  pending.unmount();
  expect(signal.aborted).toBe(true);
  expect(editor.removeAttachment).not.toHaveBeenCalled();
});

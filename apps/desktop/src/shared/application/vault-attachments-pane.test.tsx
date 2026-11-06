import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import type { AttachmentResource } from "./attachments";
import { VaultAttachmentsPane } from "./vault-attachments-pane";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultScope } from "./vault-scope";

const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
});
function file(filename: string) {
  return { filename, contentType: "application/pdf", sizeBytes: 1024 };
}
async function fixture(name = "scope") {
  let event!: (value: VaultClientEvent) => void;
  const client = {
    scope: name,
    networkMode: "always",
    attachments: vi.fn(),
    openAttachment: vi.fn(),
    subscribe: vi.fn(async (listener) => {
      event = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    }),
  } as unknown as VaultClient & {
    attachments: ReturnType<typeof vi.fn>;
    openAttachment: ReturnType<typeof vi.fn>;
  };
  const scope = new VaultScope(client);
  scope.queryClient.setDefaultOptions({
    queries: { retry: false, networkMode: "always", staleTime: Infinity },
    mutations: { networkMode: "always" },
  });
  scopes.push(scope);
  await scope.start();
  return { scope, client, emit: (value: VaultClientEvent) => event(value) };
}
function resource(id: string): AttachmentResource {
  return {
    id,
    info: file(id),
    labels: {
      open: { id: "open", message: "Open downloaded copy" },
      reveal: { id: "show", message: "Show downloaded copy" },
      save: { id: "save", message: "Save original copy" },
    },
    open: vi.fn().mockResolvedValue(undefined),
    reveal: vi.fn().mockResolvedValue(undefined),
    save: vi.fn().mockResolvedValue(undefined),
    close: vi.fn().mockResolvedValue(undefined),
  };
}

it("paginates through the shared client and opens a resource only when its actions are expanded", async () => {
  const { scope, client } = await fixture();
  client.attachments
    .mockResolvedValueOnce({ files: [file("a.pdf")], next_cursor: "a.pdf" })
    .mockResolvedValueOnce({ files: [file("z.pdf")], next_cursor: null });
  const attachment = resource("a.pdf");
  client.openAttachment.mockResolvedValue(attachment);
  render(<VaultAttachmentsPane scope={scope} sessionId="note" />);
  await screen.findByRole("button", { name: "Actions for a.pdf" });
  expect(screen.getByText("1.0 KB")).toBeTruthy();
  expect(client.attachments).toHaveBeenCalledWith(
    "note",
    100,
    undefined,
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(client.openAttachment).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Load more" }));
  await screen.findByRole("button", { name: "Actions for z.pdf" });
  expect(client.attachments).toHaveBeenLastCalledWith(
    "note",
    100,
    "a.pdf",
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(client.openAttachment).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Actions for a.pdf" }));
  fireEvent.click(
    await screen.findByRole("button", { name: "Save original copy" }),
  );
  await screen.findByText("Saved original copy.");
  expect(attachment.save).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole("button", { name: "Actions for a.pdf" }));
  await waitFor(() => expect(attachment.close).toHaveBeenCalledTimes(1));
});

it("refreshes listing for attachment changes while ignoring recording notifications", async () => {
  const { scope, client, emit } = await fixture();
  client.attachments
    .mockResolvedValueOnce({ files: [file("old.pdf")], next_cursor: null })
    .mockResolvedValue({ files: [file("new.pdf")], next_cursor: null });
  render(<VaultAttachmentsPane scope={scope} sessionId="note" />);
  await screen.findByText("old.pdf");
  act(() =>
    emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "artifacts",
        ids: ["note"],
        artifacts: [{ session_id: "note", target: { kind: "recording" } }],
      },
    }),
  );
  await act(async () => new Promise((resolve) => setTimeout(resolve, 20)));
  expect(client.attachments).toHaveBeenCalledTimes(1);
  act(() =>
    emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "artifacts",
        ids: ["note"],
        artifacts: [
          {
            session_id: "note",
            target: { kind: "attachment", attachment_id: "new.pdf" },
          },
        ],
      },
    }),
  );
  await screen.findByText("new.pdf");
  expect(screen.queryByText("old.pdf")).toBeNull();
});

it("keeps failed reads visible and supports retry without manufacturing an empty listing", async () => {
  const { scope, client } = await fixture();
  client.attachments
    .mockRejectedValueOnce(new Error("offline"))
    .mockResolvedValueOnce({ files: [], next_cursor: null });
  render(<VaultAttachmentsPane scope={scope} sessionId="note" />);
  await screen.findByRole("alert");
  expect(screen.queryByText("No attachments yet")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Try again" }));
  await screen.findByText("No attachments yet");
});

it("closes expanded resources and hides cached filenames after access loss", async () => {
  const { scope, client, emit } = await fixture();
  const attachment = resource("private.pdf");
  client.attachments.mockResolvedValue({
    files: [file("private.pdf")],
    next_cursor: null,
  });
  client.openAttachment.mockResolvedValue(attachment);
  render(<VaultAttachmentsPane scope={scope} sessionId="note" />);
  fireEvent.click(
    await screen.findByRole("button", { name: "Actions for private.pdf" }),
  );
  await screen.findByRole("button", { name: "Open downloaded copy" });
  act(() => emit({ kind: "connection", state: "access_denied", error: null }));
  await screen.findByRole("alert");
  expect(screen.queryByText("private.pdf")).toBeNull();
  expect(
    screen.queryByRole("button", { name: "Open downloaded copy" }),
  ).toBeNull();
  await waitFor(() => expect(attachment.close).toHaveBeenCalledTimes(1));
  expect(client.attachments).toHaveBeenCalledTimes(1);
});

it("keeps otherwise identical vault and session listings isolated", async () => {
  const first = await fixture("first"),
    second = await fixture("second");
  first.client.attachments.mockResolvedValue({
    files: [file("first.pdf")],
    next_cursor: null,
  });
  second.client.attachments.mockResolvedValue({
    files: [file("second.pdf")],
    next_cursor: null,
  });
  const view = render(
    <VaultAttachmentsPane scope={first.scope} sessionId="same" />,
  );
  await screen.findByText("first.pdf");
  view.rerender(<VaultAttachmentsPane scope={second.scope} sessionId="same" />);
  await screen.findByText("second.pdf");
  expect(screen.queryByText("first.pdf")).toBeNull();
});

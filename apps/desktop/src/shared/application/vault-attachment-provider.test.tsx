import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { Fragment, StrictMode } from "react";
import { createRef } from "react";
import { afterEach, expect, it, vi } from "vitest";

import {
  AttachmentResource,
  AttachmentResolverContext,
  scopedAttachmentPresentation,
} from "@hypr/editor/node-views";
import { NoteEditor, type NoteEditorRef } from "@hypr/editor/note";

import type { AttachmentResource as Resource } from "./attachments";
import { ApplicationRequestError } from "./errors";
import { VaultAttachmentProvider } from "./vault-attachment-provider";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultScope } from "./vault-scope";

const scopes: VaultScope[] = [];
afterEach(async () => {
  cleanup();
  await Promise.all(scopes.splice(0).map((scope) => scope.dispose()));
});

function resource(id = "one", image = true): Resource {
  return {
    id,
    info: {
      filename: image ? "a.png" : "a.pdf",
      contentType: image ? "image/png" : "application/pdf",
      sizeBytes: 42,
    },
    src: image ? `loofah-media://localhost/${id}` : undefined,
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
function fixture() {
  let event!: (event: VaultClientEvent) => void;
  const client = {
    scope: crypto.randomUUID(),
    networkMode: "always",
    openAttachment: vi.fn(),
    attachmentPresentation: scopedAttachmentPresentation,
    subscribe: vi.fn(async (listener) => {
      event = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    }),
  } as unknown as VaultClient & { openAttachment: ReturnType<typeof vi.fn> };
  const scope = new VaultScope(client);
  scope.queryClient.setDefaultOptions({
    queries: { retry: false, networkMode: "always", staleTime: Infinity },
    mutations: { networkMode: "always" },
  });
  scopes.push(scope);
  return { scope, client, emit: (value: VaultClientEvent) => event(value) };
}
function editor(scope: VaultScope, image = true, strict = false) {
  const ref = createRef<NoteEditorRef>();
  const changed = vi.fn();
  const Mode = strict ? StrictMode : Fragment;
  const rendered = render(
    <Mode>
      <VaultAttachmentProvider scope={scope} sessionId="session">
        <NoteEditor
          ref={ref}
          handleChange={changed}
          attachmentPresentation={scopedAttachmentPresentation}
          initialContent={{
            type: "doc",
            content: [
              {
                type: "paragraph",
                content: [{ type: "text", text: "Canonical" }],
              },
              image
                ? {
                    type: "image",
                    attrs: {
                      attachmentId: "a.png",
                      src: "attachments/a.png",
                      alt: "Owned image",
                    },
                  }
                : {
                    type: "fileAttachment",
                    attrs: {
                      attachmentId: "a.pdf",
                      name: "a.pdf",
                      mimeType: "application/pdf",
                    },
                  },
            ],
          }}
        />
      </VaultAttachmentProvider>
    </Mode>,
  );
  return { ...rendered, ref, changed };
}

it("mounts a scoped resource in the actual editor without storing its URL or creating an edit", async () => {
  const { scope, client } = fixture();
  const current = resource();
  client.openAttachment.mockResolvedValue(current);
  const { ref, changed, unmount } = editor(scope);
  await waitFor(() =>
    expect(screen.getByAltText("Owned image").getAttribute("src")).toBe(
      current.src,
    ),
  );
  expect(client.openAttachment).toHaveBeenCalledTimes(1);
  const canonical = JSON.stringify(ref.current!.view!.state.doc.toJSON());
  expect(canonical).toContain('"attachmentId":"a.png"');
  expect(canonical).not.toContain("loofah-media:");
  expect(canonical).not.toContain("downloaded");
  expect(changed).not.toHaveBeenCalled();
  fireEvent.load(screen.getByAltText("Owned image"));
  expect(screen.queryByText("Loading preview…")).toBeNull();
  unmount();
  await waitFor(() => expect(current.close).toHaveBeenCalledTimes(1));
});

it("offers keyboard file actions, bounded pending feedback, recoverable errors and a fresh original save", async () => {
  const { scope, client } = fixture();
  const current = resource("pdf", false);
  client.openAttachment.mockResolvedValue(current);
  let reject!: (error: Error) => void;
  vi.mocked(current.open).mockImplementationOnce(
    () =>
      new Promise((_resolve, fail) => {
        reject = fail;
      }),
  );
  editor(scope, false);
  const button = await screen.findByRole("button", {
    name: "Open downloaded copy",
  });
  fireEvent.click(button);
  fireEvent.click(button);
  await waitFor(() => expect(current.open).toHaveBeenCalledTimes(1));
  expect(screen.getByText("Preparing file…")).toBeTruthy();
  expect(button.hasAttribute("disabled")).toBe(true);
  await act(async () => reject(new Error("offline")));
  await screen.findByRole("alert");
  fireEvent.click(button);
  await screen.findByText("Opened file.");
  expect(current.open).toHaveBeenCalledTimes(2);
  fireEvent.click(screen.getByRole("button", { name: "Save original copy" }));
  await screen.findByText("Saved original copy.");
  expect(current.save).toHaveBeenCalledTimes(1);
});

it("retries a failed preview with a new handle and closes the replaced handle", async () => {
  const { scope, client } = fixture();
  const first = resource("old"),
    next = resource("fresh");
  client.openAttachment
    .mockResolvedValueOnce(first)
    .mockResolvedValueOnce(next);
  const { ref } = editor(scope);
  await waitFor(() =>
    expect(screen.getByAltText("Owned image").getAttribute("src")).toBe(
      first.src,
    ),
  );
  const nodeRoot = () => {
    let position = 0;
    ref.current!.view!.state.doc.descendants((node, pos) => {
      if (node.type.name === "image") position = pos;
    });
    return ref.current!.view!.nodeDOM(position);
  };
  const root = nodeRoot();
  fireEvent.error(screen.getByAltText("Owned image"));
  await screen.findByText(
    "Could not load preview. Retry or save the original copy.",
  );
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() =>
    expect(screen.getByAltText("Owned image").getAttribute("src")).toBe(
      next.src,
    ),
  );
  expect(first.close).toHaveBeenCalledTimes(1);
  expect(root?.isConnected).toBe(true);
  expect(nodeRoot()).toBe(root);
});

it("retains visible file content through failed metadata and missing attachments and allows retry", async () => {
  const { scope, client } = fixture();
  const current = resource("pdf", false);
  client.openAttachment
    .mockRejectedValueOnce(new Error("offline"))
    .mockResolvedValueOnce(null)
    .mockResolvedValueOnce(current);
  editor(scope, false);
  await screen.findByText("Could not load attachment. Retry when connected.");
  expect(screen.getByText("a.pdf")).toBeTruthy();
  expect(
    screen.queryByRole("button", { name: "Open downloaded copy" }),
  ).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await screen.findByText("Attachment not found.");
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await screen.findByRole("button", { name: "Open downloaded copy" });
});

it("shares duplicate editor resources and ignores recording-only changes", async () => {
  const { scope, client, emit } = fixture();
  const first = resource("old"),
    next = resource("new");
  client.openAttachment
    .mockResolvedValueOnce(first)
    .mockResolvedValueOnce(next);
  await scope.start();
  const view = render(
    <VaultAttachmentProvider scope={scope} sessionId="session">
      {[1, 2].map((id) => (
        <AttachmentResource key={id} attachmentId="a.png">
          {(resolved) => <span>{resolved?.src}</span>}
        </AttachmentResource>
      ))}
    </VaultAttachmentProvider>,
  );
  await waitFor(() => expect(screen.getAllByText(first.src!)).toHaveLength(2));
  expect(client.openAttachment).toHaveBeenCalledTimes(1);
  act(() =>
    emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "artifacts",
        ids: ["session"],
        artifacts: [{ session_id: "session", target: { kind: "recording" } }],
      },
    }),
  );
  await act(async () => new Promise((resolve) => setTimeout(resolve, 20)));
  expect(client.openAttachment).toHaveBeenCalledTimes(1);
  act(() =>
    emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "artifacts",
        ids: ["session"],
        artifacts: [
          {
            session_id: "session",
            target: { kind: "attachment", attachment_id: "a.png" },
          },
        ],
      },
    }),
  );
  await waitFor(() => expect(screen.getAllByText(next.src!)).toHaveLength(2));
  expect(first.close).toHaveBeenCalledTimes(1);
  view.unmount();
  await waitFor(() => expect(next.close).toHaveBeenCalledTimes(1));
});

it("aborts an active device action on unmount", async () => {
  const { scope, client } = fixture();
  const current = resource("pdf", false);
  client.openAttachment.mockResolvedValue(current);
  let signal!: AbortSignal;
  vi.mocked(current.save!).mockImplementation(
    ({ signal: active } = {}) =>
      new Promise((_resolve, reject) => {
        signal = active!;
        signal.addEventListener("abort", () => reject(new Error("cancelled")), {
          once: true,
        });
      }),
  );
  const { unmount } = editor(scope, false);
  fireEvent.click(
    await screen.findByRole("button", { name: "Save original copy" }),
  );
  await waitFor(() => expect(signal).toBeTruthy());
  unmount();
  expect(signal.aborted).toBe(true);
  await waitFor(() => expect(current.close).toHaveBeenCalledTimes(1));
});

it("does not expose actions after access is revoked", async () => {
  const { scope, client } = fixture();
  const current = resource("pdf", false);
  client.openAttachment.mockResolvedValue(current);
  vi.mocked(current.open).mockRejectedValue(
    new ApplicationRequestError({
      code: "forbidden",
      message: "denied",
      retryable: false,
      request_id: null,
      current_revision: null,
      mutation_outcome: "not_committed",
    }),
  );
  editor(scope, false);
  fireEvent.click(
    await screen.findByRole("button", { name: "Open downloaded copy" }),
  );
  await waitFor(() => expect(scope.getSnapshot().state).toBe("access_denied"));
  await screen.findByText(
    "Attachment access is unavailable. Reconnect or sign in again.",
  );
  expect(
    screen.queryByRole("button", { name: "Open downloaded copy" }),
  ).toBeNull();
  expect(client.openAttachment).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(current.close).toHaveBeenCalledTimes(1));
});

it("cleans StrictMode's cancelled opening before rendering the next resource", async () => {
  const { scope, client } = fixture();
  const cancelled = resource("cancelled"),
    active = resource("active");
  client.openAttachment
    .mockResolvedValueOnce(cancelled)
    .mockResolvedValue(active);
  const { unmount } = editor(scope, true, true);
  await waitFor(() =>
    expect(screen.getByAltText("Owned image").getAttribute("src")).toBe(
      active.src,
    ),
  );
  expect(client.openAttachment).toHaveBeenCalledTimes(2);
  expect(cancelled.close).toHaveBeenCalledTimes(1);
  expect(active.close).not.toHaveBeenCalled();
  unmount();
  await waitFor(() => expect(active.close).toHaveBeenCalledTimes(1));
});

it("keeps an existing synchronous local resolver without issuing attachment requests", () => {
  const { scope, client } = fixture();
  const rendered = render(
    <VaultAttachmentProvider scope={scope} sessionId="session" enabled={false}>
      <AttachmentResolverContext.Provider
        value={() => ({ src: "asset://localhost/local.png" })}
      >
        <AttachmentResource attachmentId="a.png">
          {(resolved) => <span>{resolved?.src}</span>}
        </AttachmentResource>
      </AttachmentResolverContext.Provider>
    </VaultAttachmentProvider>,
  );
  expect(screen.getByText("asset://localhost/local.png")).toBeTruthy();
  expect(client.openAttachment).not.toHaveBeenCalled();
  rendered.unmount();
});

it("reports an uncertain device action without retrying it automatically", async () => {
  const { scope, client } = fixture();
  const current = resource("pdf", false);
  client.openAttachment.mockResolvedValue(current);
  vi.mocked(current.save!).mockRejectedValue(
    new ApplicationRequestError({
      code: "cancelled",
      message: "reply lost",
      retryable: false,
      request_id: null,
      current_revision: null,
      mutation_outcome: "unknown",
    }),
  );
  editor(scope, false);
  fireEvent.click(
    await screen.findByRole("button", { name: "Save original copy" }),
  );
  await screen.findByText(
    "Could not confirm the file action. A copy may already be in Downloads/Loofah.",
  );
  expect(current.save).toHaveBeenCalledTimes(1);
});

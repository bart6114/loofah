import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { DeleteDocumentModal } from "./delete-document-modal";
import { ApplicationRequestError } from "./errors";
import type {
  DeleteDocumentOutcome,
  DocumentEditorSnapshot,
} from "./protocol.gen";
import type { VaultClient } from "./vault-client";

import { createRenderCacheHost } from "~/session/components/note-input/transcript/render-cache.test-support";

const guard = vi.hoisted(() => ({
  prepare: vi.fn(),
  assertReady: vi.fn(),
  dispose: vi.fn(),
}));
vi.mock("./session-generation-guard", () => ({
  captureSessionGenerationGuard: () => guard,
}));

const target = { kind: "template_output" as const, document_id: "output" };
let snapshot: DocumentEditorSnapshot;
let host: ReturnType<typeof createRenderCacheHost>;
let read: ReturnType<typeof vi.fn<VaultClient["readDocumentEditor"]>>;
let remove: ReturnType<
  typeof vi.fn<NonNullable<VaultClient["deleteDocument"]>>
>;
beforeEach(() => {
  vi.resetAllMocks();
  guard.prepare.mockResolvedValue(undefined);
  snapshot = {
    document: {
      revision: "a".repeat(64),
      document: {
        id: "output",
        session_id: "note",
        kind: "template_output",
        title: "Reviewed output",
        template_id: "template",
        sort_order: 1,
        markdown: "Saved body before confirmation",
      },
    },
    metadata: {
      revision: "m0",
      meta: {
        id: "note",
        title: "Note title",
        created_at: "2026-11-08T10:00:00Z",
        started_at: null,
        ended_at: null,
        tags: [],
      },
    },
    tasks: { revision: "t0", tasks: [] },
  };
  read = vi.fn(async () => structuredClone(snapshot));
  remove = vi.fn(async () => ({
    kind: "deleted" as const,
    revision: "deleted",
    trash_token: "opaque",
  }));
  host = createRenderCacheHost("delete:original", "always", {
    readDocumentEditor: read,
    deleteDocument: remove,
  });
});
afterEach(() => {
  cleanup();
  host.dispose();
});
function setup() {
  const deleted = vi.fn();
  const closed = vi.fn();
  const value = render(
    <host.Host>
      <DeleteDocumentModal
        sessionId="note"
        target={target}
        onClose={closed}
        onDeleted={deleted}
      />
    </host.Host>,
  );
  return { ...value, deleted, closed };
}

it("prepares the saved preview before confirmation and removes only its frozen version", async () => {
  const f = setup();
  expect(remove).not.toHaveBeenCalled();
  await screen.findByText("Saved body before confirmation");
  snapshot.document.revision = "b".repeat(64);
  snapshot.metadata.meta.created_at = "later-generation";
  await act(async () => {
    await host.scope.queryClient.invalidateQueries();
  });
  fireEvent.click(screen.getByRole("button", { name: "Remove output" }));
  await waitFor(() => expect(f.deleted).toHaveBeenCalledOnce());
  expect(read).toHaveBeenCalledOnce();
  expect(remove).toHaveBeenCalledWith(
    "note",
    target,
    {
      expected_revision: "a".repeat(64),
      expected_session_created_at: "2026-11-08T10:00:00Z",
    },
    expect.objectContaining({
      requestId: expect.any(String),
      signal: expect.any(AbortSignal),
    }),
  );
});

it("does not replay an unknown removal automatically and explicit retry retains the exact original identity", async () => {
  remove.mockRejectedValueOnce(
    new ApplicationRequestError({
      code: "unavailable",
      message: "Reply lost",
      retryable: true,
      request_id: null,
      current_revision: null,
      mutation_outcome: "unknown",
    }),
  );
  const f = setup();
  await screen.findByText("Saved body before confirmation");
  fireEvent.click(screen.getByRole("button", { name: "Remove output" }));
  await screen.findByText("Reply lost");
  expect(remove).toHaveBeenCalledOnce();
  expect(f.deleted).not.toHaveBeenCalled();
  const first = remove.mock.calls[0];
  snapshot.document.revision = "changed";
  fireEvent.click(
    screen.getByRole("button", { name: "Retry original removal" }),
  );
  await waitFor(() => expect(f.deleted).toHaveBeenCalledOnce());
  expect(remove.mock.calls[1]).toEqual(first);
  expect(read).toHaveBeenCalledOnce();
});

it("requires a new review after a conflict and never silently rebases the removal", async () => {
  remove.mockResolvedValue({
    kind: "conflict",
    snapshot: {
      ...snapshot,
      document: { ...snapshot.document, revision: "new" },
    },
  });
  const f = setup();
  await screen.findByText("Saved body before confirmation");
  fireEvent.click(screen.getByRole("button", { name: "Remove output" }));
  await screen.findByText(/saved output changed/);
  expect(
    (screen.getByRole("button", { name: "Remove output" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(remove).toHaveBeenCalledOnce();
  expect(f.deleted).not.toHaveBeenCalled();
  expect(read).toHaveBeenCalledOnce();
});

it("cancels on unmount and ignores a late successful reply", async () => {
  let finish!: (outcome: DeleteDocumentOutcome) => void;
  remove.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const f = setup();
  await screen.findByText("Saved body before confirmation");
  fireEvent.click(screen.getByRole("button", { name: "Remove output" }));
  await waitFor(() => expect(remove).toHaveBeenCalledOnce());
  const signal = remove.mock.calls[0][3].signal!;
  f.unmount();
  expect(signal.aborted).toBe(true);
  await act(async () =>
    finish({ kind: "deleted", revision: "gone", trash_token: "opaque" }),
  );
  expect(f.deleted).not.toHaveBeenCalled();
});

it("retires confirmation after access loss even if the connection later returns", async () => {
  setup();
  await screen.findByText("Saved body before confirmation");
  act(() => host.emit("access_denied"));
  act(() => host.emit("connected"));
  expect(
    (screen.getByRole("button", { name: "Remove output" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(remove).not.toHaveBeenCalled();
});

it("refuses a snapshot belonging to a different output", async () => {
  snapshot.document.document!.id = "other";
  setup();
  await screen.findByText("The server returned a different document");
  expect(
    (screen.getByRole("button", { name: "Remove output" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(remove).not.toHaveBeenCalled();
});

it("flushes and protects device drafts before reading a removal preview", async () => {
  guard.prepare.mockRejectedValue(new Error("Uncertain draft"));
  setup();
  await screen.findByText(/Save or recover your edits before removing/);
  expect(read).not.toHaveBeenCalled();
  expect(remove).not.toHaveBeenCalled();
});

it("refuses new local edits after preview without replacing its baseline", async () => {
  setup();
  await screen.findByText("Saved body before confirmation");
  guard.assertReady.mockImplementation(() => {
    throw new Error("New draft");
  });
  fireEvent.click(screen.getByRole("button", { name: "Remove output" }));
  await screen.findByText(/Save or recover your edits before removing/);
  expect(remove).not.toHaveBeenCalled();
  expect(read).toHaveBeenCalledOnce();
});

it("checks device drafts again after a known noncommitted refusal", async () => {
  remove.mockRejectedValueOnce(
    new ApplicationRequestError({
      code: "conflict",
      message: "Recording is busy",
      retryable: true,
      request_id: null,
      current_revision: null,
      mutation_outcome: "not_committed",
    }),
  );
  setup();
  await screen.findByText("Saved body before confirmation");
  fireEvent.click(screen.getByRole("button", { name: "Remove output" }));
  await screen.findByText("Recording is busy");
  guard.assertReady.mockImplementation(() => {
    throw new Error("New draft");
  });
  fireEvent.click(
    screen.getByRole("button", { name: "Retry original removal" }),
  );
  await screen.findByText(/Save or recover your edits before removing/);
  expect(remove).toHaveBeenCalledOnce();
});

it("retires the original confirmation when its session is replaced", async () => {
  const f = setup();
  await screen.findByText("Saved body before confirmation");
  f.rerender(
    <host.Host>
      <DeleteDocumentModal
        sessionId="other"
        target={target}
        onClose={f.closed}
        onDeleted={f.deleted}
      />
    </host.Host>,
  );
  expect(
    (screen.getByRole("button", { name: "Remove output" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(read).toHaveBeenCalledOnce();
  expect(remove).not.toHaveBeenCalled();
});

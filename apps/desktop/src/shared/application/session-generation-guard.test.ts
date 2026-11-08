import { onlineManager } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { documentDraftClient, documentDraftId } from "./document-draft";
import { documentEditorDrafts } from "./document-editor-draft";
import { NativeDraftStore } from "./draft-store";
import { noteDrafts } from "./note-draft";
import { editorText } from "./note-editor-draft";
import type { DocumentTarget } from "./protocol.gen";
import { sessionEditorFlushKey } from "./session-cleanup-guard";
import { captureSessionGenerationGuard } from "./session-generation-guard";
import type { VaultClient } from "./vault-client";
import type { VaultScope } from "./vault-scope";

import {
  captureNoteDraft,
  discardNoteDraft,
} from "~/session/pending-note-drafts";
import {
  drainDatabaseWrites,
  registerWriteFlusher,
} from "~/shared/write-queue";
import { useLiveTitle } from "~/store/zustand/live-title";

const inventory = vi.spyOn(NativeDraftStore.prototype, "inventory");
const stops: (() => void)[] = [];
const pending: string[] = [];
let sequence = 0;
const note = (markdown = "Saved", revision = "n0") => ({
  markdown,
  title: "",
  revision,
});
function deferred<T>(fallback: T) {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  stops.push(() => resolve(fallback));
  return { promise, resolve };
}
function fixture(
  target: DocumentTarget = { kind: "summary" },
  scopeId?: string,
) {
  let state = "connected";
  const watches = new Set<() => void>();
  const client = {
    scope: scopeId ?? `generation:${++sequence}`,
    canEdit: true,
    networkMode: "always",
    saveNote: vi.fn(async (_id, input) => ({
      kind: "saved",
      note: note(input.markdown, "n1"),
    })),
    saveDocument: vi.fn(async (_id, _target, input) => ({
      kind: "saved",
      snapshot: {
        document: {
          id: "output",
          session_id: "same",
          kind: "template_output",
          title: "Kept title",
          template_id: "kept-template",
          sort_order: 7,
          markdown: input.markdown,
        },
        revision: "d1",
      },
    })),
  } as unknown as VaultClient & {
    saveNote: ReturnType<typeof vi.fn>;
    saveDocument: ReturnType<typeof vi.fn>;
  };
  const scope = {
    client,
    getSnapshot: () => ({ state, error: null }),
    subscribe: (changed: () => void) => {
      watches.add(changed);
      return () => watches.delete(changed);
    },
  } as unknown as VaultScope;
  const raw = noteDrafts.open(client, "same", note());
  const guard = captureSessionGenerationGuard(scope, "same", target);
  stops.push(() => guard.dispose());
  return {
    client,
    scope,
    raw,
    guard,
    watches,
    state(next: string) {
      state = next;
      for (const changed of watches) changed();
    },
    flush(flush: () => Promise<void>) {
      stops.push(
        registerWriteFlusher(
          sessionEditorFlushKey(client.scope, "same"),
          flush,
        ),
      );
    },
  };
}
beforeEach(() => {
  inventory.mockReset().mockResolvedValue([]);
  onlineManager.setOnline(true);
});
afterEach(async () => {
  for (const stop of stops.splice(0)) stop();
  for (const key of pending.splice(0)) discardNoteDraft(key);
  onlineManager.setOnline(true);
  vi.useRealTimers();
  await drainDatabaseWrites();
  useLiveTitle.setState({ titles: {} });
});

it.each([false, true])(
  "refuses an unsaved or uncertain scoped title before summary admission (uncertain=%s)",
  async (uncertain) => {
    const context = fixture();
    useLiveTitle.getState().setTitle({
      scope: context.scope,
      sessionId: "same",
      owner: {},
      title: uncertain ? "Saved" : "Unsaved title",
      baselineTitle: "Saved",
      revision: "metadata",
      uncertain,
      legacy: false,
    });
    await expect(context.guard.prepare()).rejects.toThrow("Confirm or recover");
  },
);

it("retires a prepared generation if a title draft appears then disappears", async () => {
  const context = fixture();
  await context.guard.prepare();
  const draft = {
    scope: context.scope,
    sessionId: "same",
    owner: {},
    title: "Unsaved title",
    baselineTitle: "",
    revision: "metadata",
    uncertain: false,
    legacy: false,
  };
  useLiveTitle.getState().setTitle(draft);
  useLiveTitle.getState().clearTitle(draft);
  expect(() => context.guard.assertReady()).toThrow("Confirm or recover");
});

it("ignores another immutable owner's title draft despite matching scope and session strings", async () => {
  const context = fixture(undefined, "same-scope");
  const replacement = fixture(undefined, "same-scope");
  await context.guard.prepare();
  useLiveTitle.getState().setTitle({
    scope: replacement.scope,
    sessionId: "same",
    owner: {},
    title: "Other owner",
    baselineTitle: "",
    revision: "metadata",
    uncertain: true,
    legacy: false,
  });
  expect(() => context.guard.assertReady()).not.toThrow();
});

it("flushes first-500ms raw typing before generation without requiring a closed tab", async () => {
  const context = fixture();
  context.raw.capture(() => ({ markdown: "Just typed", title: "" }));
  context.flush(async () => {
    await context.raw.save();
  });
  await context.guard.prepare();
  expect(context.client.saveNote).toHaveBeenCalledOnce();
  expect(context.raw.getSnapshot().baseline.markdown).toBe("Just typed");
  expect(() => context.guard.assertReady()).not.toThrow();
  expect(inventory.mock.contexts[0]).toMatchObject({
    scope: context.client.scope,
  });
});

it("flushes the captured target document while preserving its metadata", async () => {
  const target: DocumentTarget = {
    kind: "template_output",
    document_id: "output",
  };
  const context = fixture(target);
  const output = noteDrafts.open(
    documentDraftClient(context.client, "same", target),
    documentDraftId("same", target),
    note("Body", "d0"),
  );
  context.guard.dispose();
  const guard = captureSessionGenerationGuard(context.scope, "same", target);
  stops.push(() => guard.dispose());
  output.edit("Edited output", "");
  context.flush(async () => {
    await output.save();
  });
  await guard.prepare();
  expect(context.client.saveDocument).toHaveBeenCalledWith(
    "same",
    target,
    { markdown: "Edited output", expected_revision: "d0" },
    undefined,
  );
  expect(output.getSnapshot().baseline.markdown).toBe("Edited output");
  expect(() => guard.assertReady()).not.toThrow();
});

it("waits for a delayed save and refuses rejected or lost save confirmation", async () => {
  const context = fixture();
  const response = deferred({ kind: "saved", note: note("Pending", "n1") });
  context.client.saveNote.mockReturnValueOnce(response.promise);
  context.raw.edit("Pending", "");
  context.flush(async () => {
    await context.raw.save();
  });
  const prepared = context.guard.prepare();
  await vi.waitFor(() =>
    expect(context.client.saveNote).toHaveBeenCalledOnce(),
  );
  expect(inventory).not.toHaveBeenCalled();
  response.resolve({ kind: "saved", note: note("Pending", "n1") });
  await prepared;
  expect(() => context.guard.assertReady()).not.toThrow();
  const lost = fixture();
  lost.raw.edit("Keep private typing", "");
  lost.client.saveNote.mockRejectedValueOnce(new Error("Lost save reply"));
  lost.flush(async () => {
    await lost.raw.save();
  });
  await expect(lost.guard.prepare()).rejects.toThrow("Lost save reply");
  expect(lost.raw.recovery().uncertain).toBe(true);
  expect(lost.raw.getSnapshot().draft.markdown).toBe("Keep private typing");
});

it("latches permission loss and later user edits but allows ordinary unchanged confirmation", async () => {
  const context = fixture();
  await context.guard.prepare();
  context.raw.receive(note("Saved", "n2"));
  expect(() => context.guard.assertReady()).not.toThrow();
  context.raw.edit("New input", "");
  context.raw.edit("Saved", "");
  expect(() => context.guard.assertReady()).toThrow("Confirm or recover");
  const retired = fixture();
  await retired.guard.prepare();
  retired.state("access_denied");
  retired.state("connected");
  expect(() => retired.guard.assertReady()).toThrow("Confirm or recover");
});

it("isolates same note IDs across scopes and rejects replacement drafts in the same scope", async () => {
  const first = fixture(),
    other = fixture();
  other.raw.edit("Another vault", "");
  await first.guard.prepare();
  expect(() => first.guard.assertReady()).not.toThrow();
  const replacement = noteDrafts.open(first.client, "same", note());
  expect(replacement).toBe(first.raw);
  noteDrafts.clearSaved(first.client.scope);
  noteDrafts.open(first.client, "same", note());
  expect(() => first.guard.assertReady()).toThrow("Confirm or recover");
});

it("refuses dirty/conflicted content, pending rich captures and exact-session device recovery", async () => {
  const dirty = fixture();
  dirty.raw.edit("Unsent", "");
  dirty.raw.receive(note("Elsewhere", "n3"));
  await expect(dirty.guard.prepare()).rejects.toThrow("Confirm or recover");
  const rich = fixture();
  const key = JSON.stringify(["vault-note", rich.client.scope, "same"]);
  pending.push(key);
  captureNoteDraft(key, () => ({ type: "doc", content: [] }));
  await expect(rich.guard.prepare()).rejects.toThrow("Confirm or recover");
  const recovery = fixture();
  inventory.mockResolvedValueOnce([
    {
      id: "id",
      sessionId: '["template_output","same","output"]',
      title: "Private",
      uncertain: false,
    },
  ]);
  await expect(recovery.guard.prepare()).rejects.toThrow("Confirm or recover");
});

it("keeps offline IPC login-free while refusing online-only or reader admission", async () => {
  const local = fixture(),
    remote = fixture(),
    reader = fixture();
  Object.assign(remote.client, { networkMode: "online" });
  Object.assign(reader.client, { canEdit: false });
  onlineManager.setOnline(false);
  await local.guard.prepare();
  await expect(remote.guard.prepare()).rejects.toThrow("Confirm or recover");
  await expect(reader.guard.prepare()).rejects.toThrow("Confirm or recover");
});

it("times out optional generation preparation without cancelling durable save ownership", async () => {
  vi.useFakeTimers();
  const context = fixture();
  const response = deferred({ kind: "saved", note: note("Owned", "n1") });
  context.client.saveNote.mockReturnValueOnce(response.promise);
  context.raw.edit("Owned", "");
  context.flush(async () => {
    await context.raw.save();
  });
  const failure = expect(context.guard.prepare()).rejects.toThrow(
    "Confirm or recover",
  );
  await vi.advanceTimersByTimeAsync(3000);
  await failure;
  response.resolve({ kind: "saved", note: note("Owned", "n1") });
  await vi.advanceTimersByTimeAsync(0);
  expect(context.raw.getSnapshot().baseline.markdown).toBe("Owned");
  expect(() => context.guard.assertReady()).toThrow("Confirm or recover");
});

it.each([50, 150, 500])(
  "waits for a %dms save confirmation before reading device recovery",
  async (latency) => {
    vi.useFakeTimers();
    const context = fixture();
    context.raw.edit("Network typing", "");
    context.client.saveNote.mockImplementationOnce(
      (_, input) =>
        new Promise((resolve) => {
          setTimeout(
            () => resolve({ kind: "saved", note: note(input.markdown, "n1") }),
            latency,
          );
        }),
    );
    context.flush(async () => {
      await context.raw.save();
    });
    const prepared = context.guard.prepare();
    await vi.advanceTimersByTimeAsync(latency - 1);
    expect(inventory).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    await prepared;
    expect(inventory).toHaveBeenCalledOnce();
    expect(context.raw.getSnapshot().baseline.markdown).toBe("Network typing");
  },
);

it("refuses an unreadable recovery inventory and a permission loss during save confirmation", async () => {
  const context = fixture();
  inventory.mockRejectedValueOnce(new Error("Recovery inventory unavailable"));
  await expect(context.guard.prepare()).rejects.toThrow(
    "Recovery inventory unavailable",
  );
  const retired = fixture();
  retired.raw.edit("Owned save", "");
  retired.flush(async () => {
    retired.state("closed");
    await retired.raw.save();
    retired.state("connected");
  });
  await expect(retired.guard.prepare()).rejects.toThrow("Confirm or recover");
  expect(retired.raw.getSnapshot().baseline.markdown).toBe("Owned save");
  expect(() => retired.guard.assertReady()).toThrow("Confirm or recover");
});

function enhancedDraft(
  context: ReturnType<typeof fixture>,
  target: DocumentTarget = { kind: "summary" },
) {
  return documentEditorDrafts.open(context.client, "same", target, {
    note: note("Enhanced body", "d0"),
    metadata: {
      meta: {
        id: "same",
        title: "",
        created_at: "",
        started_at: null,
        ended_at: null,
        tags: [],
      },
      revision: "m0",
    },
    tasks: { tasks: [], revision: "t0" },
  });
}
it.each([
  { kind: "summary" },
  { kind: "template_output", document_id: "output" },
] as const)(
  "refuses dirty compound $kind edits and a replacement created after capture",
  async (target) => {
    const context = fixture(target);
    const draft = enhancedDraft(context, target);
    const captured = captureSessionGenerationGuard(
      context.scope,
      "same",
      target,
    );
    stops.push(() => captured.dispose());
    draft.edit({
      ...editorText(draft.getSnapshot().baseline),
      markdown: "First 500ms typing",
    });
    await expect(captured.prepare()).rejects.toMatchObject({
      detail: { mutation_outcome: "not_committed" },
    });
    await expect(context.guard.prepare()).rejects.toThrow("Confirm or recover");
  },
);
it("latches compound target edits after preparation even when they return to their old text", async () => {
  const context = fixture();
  const draft = enhancedDraft(context);
  const captured = captureSessionGenerationGuard(context.scope, "same", {
    kind: "summary",
  });
  stops.push(() => captured.dispose());
  await captured.prepare();
  const original = editorText(draft.getSnapshot().baseline);
  draft.edit({ ...original, title: "New title" });
  draft.edit(original);
  expect(() => captured.assertReady()).toThrow("Confirm or recover");
});
it("rejects version3 compound recovery inventory for the owning session", async () => {
  const context = fixture();
  inventory.mockResolvedValue([
    {
      sessionId: JSON.stringify([
        "document_editor",
        "same",
        "template_output",
        "output",
      ]),
      id: "recovery",
      title: "",
      uncertain: false,
    },
  ]);
  await expect(context.guard.prepare()).rejects.toThrow("Confirm or recover");
});

import { onlineManager } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { documentDraftClient, documentDraftId } from "./document-draft";
import { NativeDraftStore } from "./draft-store";
import { noteDrafts } from "./note-draft";
import { sessionEditorFlushKey } from "./session-cleanup-guard";
import { captureSessionTitleGuard } from "./session-title-guard";

import {
  captureNoteDraft,
  discardNoteDraft,
} from "~/session/pending-note-drafts";
import { createSessionSourceHost } from "~/session/session-source.test-support";
import {
  drainDatabaseWrites,
  registerWriteFlusher,
} from "~/shared/write-queue";

const inventory = vi.spyOn(NativeDraftStore.prototype, "inventory");
const stops: (() => void)[] = [];
const hosts: ReturnType<typeof createSessionSourceHost>[] = [];
const note = (
  markdown = "Saved",
  title = "Original title",
  revision = "n0",
) => ({ markdown, title, revision });
function fixture(mode: "always" | "online" = "always") {
  const host = createSessionSourceHost(mode);
  hosts.push(host);
  const save = vi.fn(
    async (_id: string, input: { markdown: string; title: string | null }) => ({
      kind: "saved" as const,
      note: note(input.markdown, input.title ?? "", "n1"),
    }),
  );
  const client = { ...host.client, saveNote: save };
  const raw = noteDrafts.open(client, "session-1", note());
  let current = true;
  const guard = captureSessionTitleGuard(
    host.scope,
    "session-1",
    () => current,
  );
  stops.push(() => guard.dispose());
  return {
    ...host,
    save,
    raw,
    guard,
    retire: () => {
      current = false;
    },
    flush: (flush: () => Promise<void>) =>
      stops.push(
        registerWriteFlusher(
          sessionEditorFlushKey(client.scope, "session-1"),
          flush,
        ),
      ),
  };
}
beforeEach(() => {
  inventory.mockReset().mockResolvedValue([]);
  onlineManager.setOnline(true);
});
afterEach(async () => {
  stops.splice(0).forEach((stop) => stop());
  hosts.splice(0).forEach((host) => host.dispose());
  onlineManager.setOnline(true);
  vi.useRealTimers();
  await drainDatabaseWrites();
});

it("flushes first-500ms body typing and admits unchanged save confirmation", async () => {
  const value = fixture();
  value.raw.capture(() => ({
    markdown: "Typed just now",
    title: "Original title",
  }));
  value.flush(async () => {
    await value.raw.save();
  });
  await value.guard.prepare();
  expect(value.save).toHaveBeenCalledOnce();
  expect(value.raw.getSnapshot().baseline.markdown).toBe("Typed just now");
  expect(() => value.guard.assertReady()).not.toThrow();
});
it("includes every existing target, not only the visible summary", async () => {
  const value = fixture();
  value.guard.dispose();
  const target = {
    kind: "template_output" as const,
    document_id: "other-output",
  };
  const output = noteDrafts.open(
    documentDraftClient(value.client, "session-1", target),
    documentDraftId("session-1", target),
    note("Output"),
  );
  output.edit("Unsaved hidden output", "");
  const guard = captureSessionTitleGuard(value.scope, "session-1", () => true);
  stops.push(() => guard.dispose());
  await expect(guard.prepare()).rejects.toThrow("Confirm or recover");
  expect(output.getSnapshot().draft.markdown).toBe("Unsaved hidden output");
});
it.each(["rejected", "lost"])(
  "does not admit a title after a %s body save",
  async (kind) => {
    const value = fixture();
    value.raw.edit("Private body", "Original title");
    value.save.mockRejectedValueOnce(new Error(kind));
    value.flush(async () => {
      await value.raw.save();
    });
    await expect(value.guard.prepare()).rejects.toThrow(kind);
    expect(value.raw.getSnapshot().draft.markdown).toBe("Private body");
    expect(value.raw.recovery().uncertain).toBe(true);
  },
);
it("latches body edits after preparation, including edit-then-revert", async () => {
  const value = fixture();
  await value.guard.prepare();
  value.raw.edit("New user edit", "Original title");
  value.raw.edit("Saved", "Original title");
  expect(() => value.guard.assertReady()).toThrow("Confirm or recover");
});
it("rejects new same-session drafts, but ignores another vault's identical ID", async () => {
  const value = fixture();
  const other = fixture();
  other.raw.edit("Other vault", "Other");
  await value.guard.prepare();
  expect(() => value.guard.assertReady()).not.toThrow();
  noteDrafts.open(
    value.client,
    documentDraftId("session-1", { kind: "summary" }),
    note("new"),
  );
  expect(() => value.guard.assertReady()).toThrow("Confirm or recover");
});
it("refuses pending rich captures and native inventory that is unknown or contains session recovery", async () => {
  const value = fixture();
  const key = JSON.stringify(["vault-note", value.client.scope, "session-1"]);
  captureNoteDraft(key, () => ({ type: "doc", content: [] }));
  await expect(value.guard.prepare()).rejects.toThrow("Confirm or recover");
  discardNoteDraft(key);
  inventory.mockRejectedValueOnce(new Error("Inventory unavailable"));
  await expect(value.guard.prepare()).rejects.toThrow("Inventory unavailable");
  inventory.mockResolvedValueOnce([
    { sessionId: documentDraftId("session-1", { kind: "summary" }) },
  ] as Awaited<ReturnType<NativeDraftStore["inventory"]>>);
  await expect(value.guard.prepare()).rejects.toThrow("Confirm or recover");
});
it("latches scope access loss and captured host closure during inventory", async () => {
  const value = fixture();
  let finish!: () => void;
  inventory.mockImplementationOnce(
    () =>
      new Promise((done) => {
        finish = () => done([]);
      }),
  );
  const preparing = value.guard.prepare();
  const rejected = expect(preparing).rejects.toThrow("Confirm or recover");
  await vi.waitFor(() => expect(inventory).toHaveBeenCalled());
  value.emit({ kind: "connection", state: "access_denied", error: null });
  value.emit({ kind: "connection", state: "connected", error: null });
  finish();
  await rejected;
  const closed = fixture();
  closed.retire();
  await expect(closed.guard.prepare()).rejects.toThrow("Confirm or recover");
});
it("keeps local offline admission and refuses disconnected remote writes", async () => {
  const local = fixture();
  const remote = fixture("online");
  onlineManager.setOnline(false);
  await local.guard.prepare();
  expect(() => local.guard.assertReady()).not.toThrow();
  await expect(remote.guard.prepare()).rejects.toThrow("Confirm or recover");
});
it("times out optional preparation without abandoning durable body saves", async () => {
  vi.useFakeTimers();
  const value = fixture();
  let finish!: () => void;
  value.raw.edit("Durable body", "Original title");
  value.save.mockImplementationOnce(
    () =>
      new Promise((done) => {
        finish = () =>
          done({
            kind: "saved",
            note: note("Durable body", "Original title", "n1"),
          });
      }),
  );
  value.flush(async () => {
    await value.raw.save();
  });
  const preparing = value.guard.prepare();
  const rejected = expect(preparing).rejects.toThrow("Confirm or recover");
  await vi.advanceTimersByTimeAsync(3001);
  await rejected;
  value.guard.dispose();
  expect(value.raw.getSnapshot().dirty).toBe(true);
  finish();
  await vi.advanceTimersByTimeAsync(0);
  await drainDatabaseWrites();
  expect(value.raw.getSnapshot().baseline.markdown).toBe("Durable body");
  expect(value.raw.getSnapshot().dirty).toBe(false);
});

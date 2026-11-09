import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { JSONContent, EditorSaveResult } from "@hypr/editor/note";

import {
  captureNoteDraft,
  persistNoteDraft,
  usePendingNoteDraft,
  enhancedNoteDraftKey,
  hasUnsavedSessionNoteDrafts,
  discardNoteDraft,
} from "./pending-note-drafts";

vi.mock("@hypr/editor/note", () => ({
  areEquivalentEditorContents: (left: unknown, right: unknown) =>
    JSON.stringify(left) === JSON.stringify(right),
}));

afterEach(cleanup);

function doc(text: string): JSONContent {
  return {
    type: "doc",
    content: [{ type: "paragraph", content: [{ type: "text", text }] }],
  };
}

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe("pending note drafts", () => {
  it("keeps raw and enhanced dirty guards scoped across identical note IDs and prefix-like IDs", async () => {
    const raw = JSON.stringify(["vault-note", "guard:first", "same-id"]);
    const enhanced = enhancedNoteDraftKey("guard:second", "same-id", "doc");
    const otherId = enhancedNoteDraftKey(
      "guard:first",
      "same-id-longer",
      "doc",
    );
    captureNoteDraft(raw, () => doc("Raw unsent"));
    captureNoteDraft(enhanced, () => doc("Other vault unsent"));
    captureNoteDraft(otherId, () => doc("Other note unsent"));
    expect(hasUnsavedSessionNoteDrafts("guard:first", "same-id")).toBe(true);
    expect(hasUnsavedSessionNoteDrafts("guard:second", "same-id")).toBe(true);
    expect(hasUnsavedSessionNoteDrafts("guard:third", "same-id")).toBe(false);
    await persistNoteDraft(raw, doc("Raw saved"), async () => undefined);
    expect(hasUnsavedSessionNoteDrafts("guard:first", "same-id")).toBe(false);
    expect(hasUnsavedSessionNoteDrafts("guard:second", "same-id")).toBe(true);
    for (const key of [raw, enhanced, otherId]) discardNoteDraft(key);
  });

  it("retains scoped summary ownership after a rejected save and handles escaped scope identifiers", async () => {
    const scope = 'account,"vault"';
    const key = enhancedNoteDraftKey(scope, "scoped-note", "output");
    await expect(
      persistNoteDraft(key, doc("Private unsaved summary"), async () => {
        throw new Error("write failed");
      }),
    ).rejects.toThrow("write failed");
    expect(hasUnsavedSessionNoteDrafts(scope, "scoped-note")).toBe(true);
    expect(hasUnsavedSessionNoteDrafts("account", "scoped-note")).toBe(false);
    discardNoteDraft(key);
  });
  it("survives remount and waits for both save completion and query acknowledgment", async () => {
    const key = "remount";
    const original = doc("old");
    const edited = doc("new");
    const write = deferred();
    const mounted = renderHook(() => usePendingNoteDraft(key, original));
    let save!: Promise<EditorSaveResult>;
    act(() => {
      save = persistNoteDraft(key, edited, () => write.promise);
    });
    expect(mounted.result.current.initialDraft()).toEqual(edited);
    mounted.unmount();

    const reopened = renderHook(
      ({ content }) => usePendingNoteDraft(key, content),
      {
        initialProps: { content: original },
      },
    );
    expect(reopened.result.current.initialDraft()).toEqual(edited);
    reopened.rerender({ content: edited });
    expect(reopened.result.current.initialDraft()).toEqual(edited);

    await act(async () => {
      write.resolve();
      await save;
    });
    expect(reopened.result.current.initialDraft()).toBeUndefined();
  });

  it("retains a failed draft across remount and clears after a successful retry is observed", async () => {
    const key = "failure";
    const edited = doc("unsaved");
    await expect(
      persistNoteDraft(key, edited, () =>
        Promise.reject(new Error("disk full")),
      ),
    ).rejects.toThrow("disk full");
    const mounted = renderHook(() => usePendingNoteDraft(key, edited));
    expect(mounted.result.current.initialDraft()).toEqual(edited);
    mounted.unmount();
    const reopened = renderHook(
      ({ content }) => usePendingNoteDraft(key, content),
      {
        initialProps: { content: doc("old") },
      },
    );
    expect(reopened.result.current.initialDraft()).toEqual(edited);
    await act(async () => {
      await persistNoteDraft(key, edited, () => Promise.resolve());
    });
    expect(reopened.result.current.initialDraft()).toEqual(edited);
    reopened.rerender({ content: edited });
    expect(reopened.result.current.initialDraft()).toBeUndefined();
  });

  it("does not let an older completion or echo acknowledge a newer draft", async () => {
    const key = "out-of-order";
    const first = doc("first");
    const second = doc("second");
    const firstWrite = deferred();
    const secondWrite = deferred();
    const firstSave = persistNoteDraft(key, first, () => firstWrite.promise);
    const secondSave = persistNoteDraft(key, second, () => secondWrite.promise);
    const mounted = renderHook(
      ({ content }) => usePendingNoteDraft(key, content),
      {
        initialProps: { content: first },
      },
    );
    await act(async () => {
      firstWrite.resolve();
      await firstSave;
    });
    expect(mounted.result.current.initialDraft()).toEqual(second);
    await act(async () => {
      secondWrite.resolve();
      await secondSave;
    });
    expect(mounted.result.current.initialDraft()).toEqual(second);
    mounted.rerender({ content: second });
    expect(mounted.result.current.initialDraft()).toBeUndefined();
  });

  it("does not expose or acknowledge a draft in preview mode", async () => {
    const key = "preview";
    const edited = doc("edited");
    await persistNoteDraft(key, edited, () => Promise.resolve());
    const preview = renderHook(() => usePendingNoteDraft(undefined, edited));
    expect(preview.result.current.initialDraft()).toBeUndefined();
    const persisted = renderHook(() => usePendingNoteDraft(key, doc("old")));
    expect(persisted.result.current.initialDraft()).toEqual(edited);
  });

  it("captures changes without serializing or notifying the mounted editor", () => {
    const key = "lazy-capture";
    const original = doc("old");
    const edited = doc("new");
    let renders = 0;
    const mounted = renderHook(() => {
      renders++;
      return usePendingNoteDraft(key, original);
    });
    const getContent = vi.fn(() => edited);
    const before = renders;
    act(() => captureNoteDraft(key, getContent));
    expect(getContent).not.toHaveBeenCalled();
    expect(renders).toBe(before);
    expect(mounted.result.current.initialDraft()).toEqual(edited);
    expect(getContent).toHaveBeenCalledOnce();
  });

  it("accepts a confirmed fresh read when a coalesced update skips the submitted content", async () => {
    const key = "confirmed-newer";
    const edited = doc("my saved edit");
    const confirmed = doc("external edit after my save");
    const readContent = () => confirmed;
    await expect(
      persistNoteDraft(key, edited, () => Promise.resolve(readContent)),
    ).resolves.toBe(readContent);
    const mounted = renderHook(
      ({ content }) => usePendingNoteDraft(key, content),
      {
        initialProps: { content: doc("old") },
      },
    );
    expect(mounted.result.current.initialDraft()).toEqual(confirmed);
    mounted.rerender({ content: confirmed });
    expect(mounted.result.current.initialDraft()).toBeUndefined();
  });

  it("uses the latest confirmed cache content for remount and acknowledgment even when the first confirmation was skipped", async () => {
    const key = "live-confirmation";
    const edited = doc("my saved edit");
    let latest = doc("first refreshed result");
    const readContent = () => latest;
    await expect(
      persistNoteDraft(key, edited, () => Promise.resolve(readContent)),
    ).resolves.toBe(readContent);
    latest = doc("newer query result before callback delivery");
    const mounted = renderHook(
      ({ content }) => usePendingNoteDraft(key, content),
      {
        initialProps: { content: doc("old") },
      },
    );
    expect(mounted.result.current.initialDraft()).toEqual(latest);
    mounted.rerender({ content: latest });
    expect(mounted.result.current.initialDraft()).toBeUndefined();
  });

  it("retains the last confirmed document if an inactive query cache disappears before remount", async () => {
    const key = "expired-confirmation";
    const confirmed = doc("confirmed edit");
    const reader = vi.fn(() => confirmed);
    await persistNoteDraft(key, doc("submitted"), () =>
      Promise.resolve(reader),
    );
    reader.mockImplementation(() => {
      throw new Error("query removed");
    });
    const mounted = renderHook(() => usePendingNoteDraft(key, doc("stale")));
    expect(mounted.result.current.initialDraft()).toEqual(confirmed);
  });

  it("discards saved reader carryovers from an older generation while preserving unsaved edits", async () => {
    const savedKey = "saved-older-generation";
    await persistNoteDraft(
      savedKey,
      doc("old saved"),
      () => Promise.resolve(() => doc("old source")),
      "old-generation",
    );
    const saved = renderHook(() =>
      usePendingNoteDraft(
        savedKey,
        doc("new generated content"),
        "new-generation",
      ),
    );
    expect(saved.result.current.initialDraft()).toBeUndefined();
    expect(saved.result.current.confirmedDraft).toBeUndefined();

    const unsavedKey = "unsaved-older-generation";
    captureNoteDraft(unsavedKey, () => doc("local unsaved"), "old-generation");
    const unsaved = renderHook(() =>
      usePendingNoteDraft(
        unsavedKey,
        doc("new generated content"),
        "new-generation",
      ),
    );
    expect(unsaved.result.current.initialDraft()).toEqual(doc("local unsaved"));
    expect(unsaved.result.current.confirmedDraft).toBeUndefined();
  });
});

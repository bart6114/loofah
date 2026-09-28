import { beforeEach, describe, expect, it, vi } from "vitest";

import { enqueueDatabaseWrite } from "~/shared/write-queue";

const mocks = vi.hoisted(() => ({
  sessionUpdateEnhancedDoc: vi.fn(async () => ({ status: "ok", data: null })),
  sessionGet: vi.fn(
    (): Promise<
      | { status: "ok"; data: Record<string, unknown> | null }
      | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionUpdateMeta: vi.fn(
    (): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
  sessionUpdateSummary: vi.fn(
    (): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionGet: mocks.sessionGet,
    sessionUpdateMeta: mocks.sessionUpdateMeta,
    sessionUpdateSummary: mocks.sessionUpdateSummary,
    sessionUpdateEnhancedDoc: mocks.sessionUpdateEnhancedDoc,
  },
}));

import {
  applyGeneratedSessionTitle,
  persistGeneratedEnhancedNote,
} from "./content-mutations";

function sessionRecord(overrides?: {
  title?: string;
  tags?: string[];
}): Record<string, unknown> {
  return {
    meta: {
      id: "session-1",
      title: overrides?.title ?? "",
      created_at: "2026-07-10T09:00:00.000Z",
      tags: overrides?.tags ?? [],
      event: null,
    },
    note_markdown: null,
  };
}

describe("session content corrections", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.sessionGet.mockResolvedValue({
      status: "ok",
      data: sessionRecord(),
    });
    mocks.sessionUpdateMeta.mockResolvedValue({ status: "ok", data: null });
    mocks.sessionUpdateSummary.mockResolvedValue({
      status: "ok",
      data: null,
    });
  });

  it("saves content and suggestions through one guarded store command", async () => {
    await persistGeneratedEnhancedNote({
      sessionId: "session-1",
      ownerUserId: "user-1",
      note: {
        id: "session-1",
        currentMarkdown: "old summary",
        nextMarkdown: "# New summary",
      },
      suggestedTags: ["launch", "launch", "prep"],
    });

    // The doc body goes file-first through the store, guarded by the file's current
    // markdown -- never a raw session_documents UPDATE.
    expect(mocks.sessionUpdateSummary).toHaveBeenCalledWith(
      "session-1",
      "# New summary",
      "old summary",
      true,
      ["launch", "launch", "prep"],
    );

    expect(mocks.sessionGet).not.toHaveBeenCalled();
    expect(mocks.sessionUpdateMeta).not.toHaveBeenCalled();
  });

  it("also reconciles generated template document tasks", async () => {
    await persistGeneratedEnhancedNote({
      sessionId: "session-1",
      ownerUserId: "user-1",
      note: {
        id: "template-1",
        currentMarkdown: "old",
        nextMarkdown: "- [ ] Send proposal",
      },
      suggestedTags: [],
    });
    expect(mocks.sessionUpdateEnhancedDoc).toHaveBeenCalledWith(
      "session-1",
      "template-1",
      {
        markdown: "- [ ] Send proposal",
        expected_markdown: "old",
        reconcile_tasks: true,
        suggested_tags: [],
      },
    );
    expect(mocks.sessionUpdateSummary).not.toHaveBeenCalled();
  });

  it("rejects (and skips the tag write) when the store CAS finds a stale summary", async () => {
    mocks.sessionUpdateSummary.mockResolvedValueOnce({
      status: "error",
      error: "conflict: enhanced doc summary-1 body changed since it was read",
    });

    await expect(
      persistGeneratedEnhancedNote({
        sessionId: "session-1",
        ownerUserId: "user-1",
        note: {
          id: "session-1",
          currentMarkdown: "stale summary",
          nextMarkdown: "# New summary",
        },
        suggestedTags: ["launch"],
      }),
    ).rejects.toThrow("conflict");
    expect(mocks.sessionUpdateMeta).not.toHaveBeenCalled();
  });

  it.each([undefined, []])(
    "distinguishes unavailable suggestions from an empty selection (%j)",
    async (suggestedTags) => {
      await persistGeneratedEnhancedNote({
        sessionId: "session-1",
        ownerUserId: "user-1",
        note: {
          id: "session-1",
          currentMarkdown: "old summary",
          nextMarkdown: "# New summary",
        },
        suggestedTags,
      });

      expect(mocks.sessionUpdateSummary).toHaveBeenCalledWith(
        "session-1",
        "# New summary",
        "old summary",
        true,
        suggestedTags ?? null,
      );
      expect(mocks.sessionGet).not.toHaveBeenCalled();
      expect(mocks.sessionUpdateMeta).not.toHaveBeenCalled();
    },
  );

  it("does not persist a cancelled generation waiting in the write queue", async () => {
    let release!: () => void;
    const wait = new Promise<void>((resolve) => {
      release = resolve;
    });
    const blocker = enqueueDatabaseWrite("session:session-1", () => wait);
    const controller = new AbortController();
    const pending = persistGeneratedEnhancedNote({
      sessionId: "session-1",
      ownerUserId: "user-1",
      note: {
        id: "session-1",
        currentMarkdown: "old",
        nextMarkdown: "# Cancelled",
      },
      suggestedTags: ["cancelled"],
      signal: controller.signal,
    });
    const rejection = expect(pending).rejects.toMatchObject({
      name: "AbortError",
    });
    controller.abort();
    release();
    await blocker;
    await rejection;
    expect(mocks.sessionUpdateSummary).not.toHaveBeenCalled();
    expect(mocks.sessionUpdateMeta).not.toHaveBeenCalled();
  });

  it("applies a generated title through the store after the document guards pass", async () => {
    mocks.sessionGet.mockResolvedValueOnce({
      status: "ok",
      data: sessionRecord({ title: "" }),
    });

    await applyGeneratedSessionTitle({
      sessionId: "session-1",
      currentTitle: "",
      nextTitle: "Planning",
      documents: [
        {
          id: "session-1",
          currentMarkdown: "old summary",
          nextMarkdown: "# Planning\n\nold summary",
        },
      ],
    });

    // Each summary is stamped file-first through the store's markdown CAS -- never raw
    // session_documents SQL, and never the raw note (which title-success stamps
    // separately through session_read_note/session_write_note).
    expect(mocks.sessionUpdateSummary).toHaveBeenCalledWith(
      "session-1",
      "# Planning\n\nold summary",
      "old summary",
      false,
      null,
    );
    // The title itself is store-canonical, never a raw `UPDATE sessions`.
    expect(mocks.sessionUpdateMeta).toHaveBeenCalledWith("session-1", {
      title: "Planning",
    });
  });

  it("applies nothing when the session title changed while generating", async () => {
    mocks.sessionGet.mockResolvedValueOnce({
      status: "ok",
      data: sessionRecord({ title: "User renamed this" }),
    });

    await expect(
      applyGeneratedSessionTitle({
        sessionId: "session-1",
        currentTitle: "",
        nextTitle: "Planning",
        documents: [],
      }),
    ).rejects.toThrow("title changed while generating");

    expect(mocks.sessionUpdateSummary).not.toHaveBeenCalled();
    expect(mocks.sessionUpdateMeta).not.toHaveBeenCalled();
  });

  it("applies nothing when the session no longer exists", async () => {
    mocks.sessionGet.mockResolvedValueOnce({ status: "ok", data: null });

    await expect(
      applyGeneratedSessionTitle({
        sessionId: "session-1",
        currentTitle: "",
        nextTitle: "Planning",
        documents: [],
      }),
    ).rejects.toThrow("title changed while generating");
    expect(mocks.sessionUpdateMeta).not.toHaveBeenCalled();
  });

  it("rolls back the generated title when any enhanced-note document guard is stale", async () => {
    mocks.sessionGet.mockResolvedValueOnce({
      status: "ok",
      data: sessionRecord({ title: "" }),
    });
    mocks.sessionUpdateSummary.mockResolvedValueOnce({
      status: "error",
      error: "conflict: enhanced doc summary-1 body changed since it was read",
    });

    await expect(
      applyGeneratedSessionTitle({
        sessionId: "session-1",
        currentTitle: "",
        nextTitle: "Planning",
        documents: [
          {
            id: "session-1",
            currentMarkdown: "old summary",
            nextMarkdown: "# Planning\n\nold summary",
          },
        ],
      }),
    ).rejects.toThrow("conflict");

    expect(mocks.sessionUpdateMeta).not.toHaveBeenCalled();
  });
});

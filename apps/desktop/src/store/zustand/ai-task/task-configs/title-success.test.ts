import type { LanguageModel } from "ai";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { TaskConfig } from ".";
import { persistGeneratedTitle, titleSuccess } from "./title-success";

import { createSessionSourceHost } from "~/session/session-source.test-support";
import { useLiveTitle } from "~/store/zustand/live-title";

const mocks = vi.hoisted(() => ({
  loadSessionContentSnapshot: vi.fn(),
  applyGeneratedSessionTitle: vi.fn().mockResolvedValue(undefined),
  sessionReadNote: vi.fn(
    (): Promise<
      { status: "ok"; data: string | null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: "Raw note" }),
  ),
  sessionWriteNote: vi.fn(
    (): Promise<
      { status: "ok"; data: null } | { status: "error"; error: string }
    > => Promise.resolve({ status: "ok", data: null }),
  ),
}));

vi.mock("~/session/content-queries", () => ({
  loadSessionContentSnapshot: mocks.loadSessionContentSnapshot,
}));

vi.mock("~/session/content-mutations", () => ({
  applyGeneratedSessionTitle: mocks.applyGeneratedSessionTitle,
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionReadNote: mocks.sessionReadNote,
    sessionWriteNote: mocks.sessionWriteNote,
  },
}));

type TitleSuccessParams = Parameters<
  NonNullable<TaskConfig<"title">["onSuccess"]>
>[0];

function createSnapshot(title = "") {
  return {
    sessionId: "session-1",
    ownerUserId: "user-1",
    title,
    createdAt: "2026-07-10T00:00:00.000Z",
    event: null,
    eventId: null,
    rawNoteId: "session-1",
    rawContent: "Raw note",
    rawContentFormat: "markdown",
    rawMarkdown: "Raw note",
    enhancedNotes: [
      {
        id: "note-1",
        title: "",
        markdown: "# Summary section",
        content: "# Summary section",
        contentFormat: "markdown",
        templateId: "",
        position: 0,
      },
    ],
    transcripts: [],
    participants: [],
  };
}

function createParams(
  overrides: Partial<TitleSuccessParams> = {},
): TitleSuccessParams {
  return {
    taskId: "session-1-title",
    text: "Meeting title",
    model: {} as LanguageModel,
    args: { sessionId: "session-1" },
    transformedArgs: {} as TitleSuccessParams["transformedArgs"],
    signal: new AbortController().signal,
    startTask: vi.fn().mockResolvedValue(undefined),
    getTaskState: vi.fn().mockReturnValue(undefined),
    ...overrides,
  };
}

describe("titleSuccess.onSuccess", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useLiveTitle.setState({ titles: {} });
    mocks.loadSessionContentSnapshot.mockResolvedValue(createSnapshot());
    mocks.applyGeneratedSessionTitle.mockResolvedValue(undefined);
    mocks.sessionReadNote.mockResolvedValue({ status: "ok", data: "Raw note" });
    mocks.sessionWriteNote.mockResolvedValue({ status: "ok", data: null });
  });

  it("persists a trimmed title, the enhanced-note documents via the store, and the raw note file-first", async () => {
    await titleSuccess.onSuccess?.(createParams({ text: "  Weekly sync  " }));

    expect(mocks.applyGeneratedSessionTitle).toHaveBeenCalledWith({
      sessionId: "session-1",
      currentTitle: "",
      nextTitle: "Weekly sync",
      documents: [
        expect.objectContaining({
          id: "note-1",
          currentMarkdown: "# Summary section",
          nextMarkdown: expect.stringContaining("Weekly sync"),
        }),
      ],
    });

    // The raw note is stamped separately: fresh read, CAS-compare to the snapshot, write.
    expect(mocks.sessionReadNote).toHaveBeenCalledWith("session-1");
    expect(mocks.sessionWriteNote).toHaveBeenCalledWith(
      "session-1",
      expect.stringContaining("Weekly sync"),
    );
  });

  it("skips the raw note stamp when the file changed since the snapshot was taken", async () => {
    mocks.sessionReadNote.mockResolvedValue({
      status: "ok",
      data: "Someone else's edit",
    });

    await titleSuccess.onSuccess?.(createParams({ text: "Weekly sync" }));

    expect(mocks.applyGeneratedSessionTitle).toHaveBeenCalled();
    expect(mocks.sessionWriteNote).not.toHaveBeenCalled();
  });

  it("skips the raw note stamp when the note file does not exist yet", async () => {
    mocks.sessionReadNote.mockResolvedValue({ status: "ok", data: null });

    await titleSuccess.onSuccess?.(createParams({ text: "Weekly sync" }));

    expect(mocks.sessionWriteNote).not.toHaveBeenCalled();
  });

  it("does not let a failed note read block the title/summary transaction", async () => {
    mocks.sessionReadNote.mockResolvedValue({
      status: "error",
      error: "disk error",
    });

    await expect(
      titleSuccess.onSuccess?.(createParams({ text: "Weekly sync" })),
    ).resolves.toBeUndefined();

    expect(mocks.applyGeneratedSessionTitle).toHaveBeenCalled();
    expect(mocks.sessionWriteNote).not.toHaveBeenCalled();
  });

  it("does not overwrite an existing session title", async () => {
    mocks.loadSessionContentSnapshot.mockResolvedValue(
      createSnapshot("Custom title"),
    );

    await expect(
      persistGeneratedTitle({
        text: "Generated title",
        args: { sessionId: "session-1" },
      }),
    ).resolves.toBe(false);
    expect(mocks.applyGeneratedSessionTitle).not.toHaveBeenCalled();
  });

  it("does not overwrite an active title edit, including a blank draft", async () => {
    const local = createSessionSourceHost("always", {
      features: ["legacy_desktop_enhance"],
    });
    useLiveTitle.getState().setTitle({
      scope: local.scope,
      sessionId: "session-1",
      owner: {},
      title: "",
      revision: "m0",
      baselineTitle: "",
      uncertain: false,
      legacy: true,
    });
    local.dispose();

    await titleSuccess.onSuccess?.(createParams());

    expect(mocks.loadSessionContentSnapshot).not.toHaveBeenCalled();
    expect(mocks.applyGeneratedSessionTitle).not.toHaveBeenCalled();
  });

  it("ignores empty and placeholder title outputs", async () => {
    await titleSuccess.onSuccess?.(createParams({ text: "   " }));
    await titleSuccess.onSuccess?.(createParams({ text: "<EMPTY>" }));

    expect(mocks.loadSessionContentSnapshot).not.toHaveBeenCalled();
    expect(mocks.applyGeneratedSessionTitle).not.toHaveBeenCalled();
  });

  it("does not persist a title task marked skipPersist", async () => {
    await titleSuccess.onSuccess?.(
      createParams({ args: { sessionId: "session-1", skipPersist: true } }),
    );

    expect(mocks.loadSessionContentSnapshot).not.toHaveBeenCalled();
  });

  it("propagates a stale transaction failure", async () => {
    mocks.applyGeneratedSessionTitle.mockRejectedValueOnce(
      new Error("unexpected rows affected"),
    );

    await expect(titleSuccess.onSuccess?.(createParams())).rejects.toThrow(
      "unexpected rows affected",
    );
  });
});

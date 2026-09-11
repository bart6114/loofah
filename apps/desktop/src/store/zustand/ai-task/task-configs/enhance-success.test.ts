import type { LanguageModel } from "ai";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { TaskConfig } from ".";
import { enhanceSuccess } from "./enhance-success";

import { MIN_SUMMARY_CHARACTERS } from "~/services/enhancer/summary-length";
import { useLiveTitle } from "~/store/zustand/live-title";

const mocks = vi.hoisted(() => ({
  loadSessionContentSnapshot: vi.fn(),
  persistGeneratedEnhancedNote: vi.fn().mockResolvedValue(undefined),
  persistGeneratedTitle: vi.fn().mockResolvedValue(true),
}));

vi.mock("~/session/content-queries", () => ({
  loadSessionContentSnapshot: mocks.loadSessionContentSnapshot,
}));

vi.mock("~/session/content-mutations", () => ({
  persistGeneratedEnhancedNote: mocks.persistGeneratedEnhancedNote,
}));

vi.mock("./title-success", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./title-success")>()),
  persistGeneratedTitle: mocks.persistGeneratedTitle,
}));

type EnhanceSuccessParams = Parameters<
  NonNullable<TaskConfig<"enhance">["onSuccess"]>
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
    rawContent: "",
    rawContentFormat: "prosemirror_json",
    rawMarkdown: "",
    enhancedNotes: [
      {
        id: "note-1",
        title: "",
        markdown: "old content",
        content: "old content",
        contentFormat: "md",
        position: 0,
      },
    ],
    transcripts: [],
    participants: [],
  };
}

function createTransformedArgs(): EnhanceSuccessParams["transformedArgs"] {
  return {
    language: "en",
    promptOverride: "",
    session: {
      title: "Weekly Review",
      startedAt: null,
      endedAt: null,
      event: null,
    },
    participants: [],
    preMeetingMemo: "",
    postMeetingMemo: "",
    transcripts: [],
    imageContext: [],
  };
}

function createParams(
  overrides: Partial<EnhanceSuccessParams> = {},
): EnhanceSuccessParams {
  return {
    taskId: "note-1-enhance",
    text: "# Summary\n\n- Point",
    model: {} as LanguageModel,
    args: {
      sessionId: "session-1",
      enhancedNoteId: "note-1",
    },
    transformedArgs: createTransformedArgs(),
    signal: new AbortController().signal,
    startTask: vi.fn().mockResolvedValue(undefined),
    getTaskState: vi.fn().mockReturnValue(undefined),
    ...overrides,
  };
}

describe("enhanceSuccess.onSuccess", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useLiveTitle.setState({ titles: {} });
    mocks.loadSessionContentSnapshot.mockResolvedValue(createSnapshot());
    mocks.persistGeneratedEnhancedNote.mockResolvedValue(undefined);
    mocks.persistGeneratedTitle.mockResolvedValue(true);
  });

  it("persists generated content and tags through one guarded store write", async () => {
    const params = createParams({
      text: "# Summary\n\nDiscussed #Launch.",
      transformedArgs: {
        ...createTransformedArgs(),
        preMeetingMemo: "Prep #prep #Launch",
      },
    });

    await enhanceSuccess.onSuccess?.(params);

    // Markdown-based since D-3: the store CASes on the doc file's current markdown, and
    // there is no separate single-slot `summary.md` mirror or shadow-row cleanup anymore.
    expect(mocks.persistGeneratedEnhancedNote).toHaveBeenCalledWith({
      sessionId: "session-1",
      ownerUserId: "user-1",
      note: {
        id: "note-1",
        currentMarkdown: "old content",
        nextMarkdown: expect.any(String),
      },
      tagNames: ["launch", "prep"],
    });
    const markdown =
      mocks.persistGeneratedEnhancedNote.mock.calls[0][0].note.nextMarkdown;
    expect(markdown.trim()).toBe(
      "# Summary\n\nDiscussed #Launch.\n\n#launch #prep",
    );
  });

  it("waits for a generated title, saves the note, then persists the title", async () => {
    const startTask = vi.fn().mockImplementation(async (_taskId, config) => {
      config.onComplete?.("Generated title");
    });

    await enhanceSuccess.onSuccess?.(createParams({ startTask }));

    expect(startTask).toHaveBeenCalledWith(
      "session-1-title",
      expect.objectContaining({
        taskType: "title",
        args: expect.objectContaining({ skipPersist: true }),
      }),
    );
    expect(mocks.persistGeneratedEnhancedNote).toHaveBeenCalledBefore(
      mocks.persistGeneratedTitle,
    );
    const markdown =
      mocks.persistGeneratedEnhancedNote.mock.calls[0][0].note.nextMarkdown;
    expect(markdown.trim()).toBe("# Generated title\n\n# Summary\n\n- Point");
    expect(mocks.persistGeneratedTitle).toHaveBeenCalledWith({
      text: "Generated title",
      args: { sessionId: "session-1" },
    });
  });

  it("uses an existing title without starting title generation", async () => {
    mocks.loadSessionContentSnapshot.mockResolvedValue(
      createSnapshot("Existing title"),
    );
    const params = createParams();

    await enhanceSuccess.onSuccess?.(params);

    expect(params.startTask).not.toHaveBeenCalled();
    expect(mocks.persistGeneratedTitle).not.toHaveBeenCalled();
    const markdown =
      mocks.persistGeneratedEnhancedNote.mock.calls[0][0].note.nextMarkdown;
    expect(markdown.trim()).toBe("# Existing title\n\n# Summary\n\n- Point");
  });

  it("persists a short summary and tags within the transcript length and section cap", async () => {
    mocks.loadSessionContentSnapshot.mockResolvedValue(
      createSnapshot("Meeting title"),
    );
    const transformedArgs = createTransformedArgs();
    transformedArgs.preMeetingMemo = "Follow up with #launch";
    transformedArgs.transcripts = [
      {
        startedAt: null,
        endedAt: null,
        segments: [{ speaker: "John", text: "x".repeat(160) }],
      },
    ];

    await enhanceSuccess.onSuccess?.(
      createParams({
        text: `# First

- ${"a".repeat(100)}

# Second

- ${"b".repeat(100)}

# Third

- ${"c".repeat(100)}`,
        transformedArgs,
      }),
    );

    const markdown =
      mocks.persistGeneratedEnhancedNote.mock.calls[0][0].note.nextMarkdown.trim();
    expect(markdown).toContain("# First");
    expect(markdown).toContain("# Second");
    expect(markdown).not.toContain("# Third");
    expect(markdown).toContain("#launch");
    expect(
      Array.from(markdown.replace(/\s+/gu, " ")).length,
    ).toBeLessThanOrEqual(MIN_SUMMARY_CHARACTERS);
  });

  it("does not claim success when the guarded store write fails", async () => {
    mocks.persistGeneratedEnhancedNote.mockRejectedValueOnce(
      new Error("stale summary"),
    );

    await expect(
      enhanceSuccess.onSuccess?.(
        createParams({
          getTaskState: vi.fn().mockReturnValue({
            taskType: "title",
            status: "generating",
            streamedText: "",
            abortController: null,
          }),
        }),
      ),
    ).rejects.toThrow("stale summary");
    expect(mocks.persistGeneratedTitle).not.toHaveBeenCalled();
  });

  it("does not save after cancellation during title generation", async () => {
    const abortController = new AbortController();
    const startTask = vi.fn().mockImplementation(async (_taskId, config) => {
      abortController.abort();
      config.onComplete?.("Generated title");
    });

    await enhanceSuccess.onSuccess?.(
      createParams({ signal: abortController.signal, startTask }),
    );

    expect(mocks.persistGeneratedEnhancedNote).not.toHaveBeenCalled();
  });

  it("rejects persistence when the target summary disappeared", async () => {
    const snapshot = createSnapshot("Existing title");
    snapshot.enhancedNotes = [];
    mocks.loadSessionContentSnapshot.mockResolvedValue(snapshot);

    await expect(enhanceSuccess.onSuccess?.(createParams())).rejects.toThrow(
      "Summary note-1 no longer exists",
    );
  });

  it("does not generate a title while a live title edit exists", async () => {
    useLiveTitle.getState().setTitle("session-1", "Draft title");
    const params = createParams();

    await enhanceSuccess.onSuccess?.(params);

    expect(params.startTask).not.toHaveBeenCalled();
    expect(mocks.persistGeneratedTitle).not.toHaveBeenCalled();
  });
});

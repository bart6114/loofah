import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { enhanceTransform } from "./enhance-transform";

const mocks = vi.hoisted(() => ({
  collectEnhanceImageContext: vi.fn(),
  loadSessionContentSnapshot: vi.fn(),
  buildRenderTranscriptRequestFromRows: vi.fn(),
  renderTranscriptSegments: vi.fn(),
}));

vi.mock("./enhance-images", () => ({
  collectEnhanceImageContext: mocks.collectEnhanceImageContext,
}));

vi.mock("~/session/content-queries", () => ({
  loadSessionContentSnapshot: mocks.loadSessionContentSnapshot,
}));

vi.mock("~/stt/render-transcript", () => ({
  buildRenderTranscriptRequestFromRows:
    mocks.buildRenderTranscriptRequestFromRows,
  renderTranscriptSegments: mocks.renderTranscriptSegments,
}));

function createSnapshot() {
  return {
    sessionId: "session-1",
    ownerUserId: "user-1",
    title: "Weekly Review",
    createdAt: "2026-07-10T00:00:00.000Z",
    event: null,
    eventId: null,
    rawNoteId: "session-1",
    rawContent: "![post](asset://localhost/post.png)",
    rawContentFormat: "markdown",
    rawMarkdown: "![post](asset://localhost/post.png)",
    enhancedNotes: [],
    transcripts: [
      {
        id: "transcript-1",
        started_at: 100,
        ended_at: 200,
        memo: "![pre](asset://localhost/pre.png)",
        wordsJson: "[]",
        words: [{ text: "Discussed the release" }],
        speaker_hints: [],
      },
    ],
  };
}

const settingsValues = { ai_language: "en" } as const;

describe("enhanceTransform.transformArgs", () => {
  let consoleError: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    vi.clearAllMocks();
    mocks.collectEnhanceImageContext.mockResolvedValue([]);
    mocks.loadSessionContentSnapshot.mockResolvedValue(createSnapshot());
    mocks.buildRenderTranscriptRequestFromRows.mockReturnValue(null);
    mocks.renderTranscriptSegments.mockResolvedValue([]);
    consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    consoleError.mockRestore();
  });

  it("uses note text without rendering a transcript", async () => {
    const snapshot = {
      ...createSnapshot(),
      rawMarkdown: "Ship Friday",
      transcripts: [],
    };
    mocks.loadSessionContentSnapshot.mockResolvedValue(snapshot);
    const result = await enhanceTransform.transformArgs(
      { sessionId: "session-1", enhancedNoteId: "note-1" },
      settingsValues,
    );
    expect(result.postMeetingMemo).toBe("Ship Friday");
    expect(result.transcripts).toEqual([]);
    expect(mocks.renderTranscriptSegments).not.toHaveBeenCalled();
  });

  it("rejects empty source before starting a model", async () => {
    mocks.loadSessionContentSnapshot.mockResolvedValue({
      ...createSnapshot(),
      rawMarkdown: "&nbsp;",
      transcripts: [],
    });
    await expect(
      enhanceTransform.transformArgs(
        { sessionId: "session-1", enhancedNoteId: "note-1" },
        settingsValues,
      ),
    ).rejects.toThrow("Add a note or transcript");
  });

  it("uses the saved prompt override for summaries", async () => {
    const result = await enhanceTransform.transformArgs(
      { sessionId: "session-1", enhancedNoteId: "note-1" },
      {
        ...settingsValues,
        auto_summary_prompt: "  Start with decisions.  ",
      },
    );

    expect(result.promptOverride).toBe("  Start with decisions.  ");
  });

  it("uses the shared prompt when regenerating a legacy summary", async () => {
    mocks.loadSessionContentSnapshot.mockResolvedValue({
      ...createSnapshot(),
      enhancedNotes: [
        { id: "note-1", kind: "template_output", templateId: "retired" },
      ],
    });
    const result = await enhanceTransform.transformArgs(
      {
        sessionId: "session-1",
        enhancedNoteId: "note-1",
      },
      {
        ...settingsValues,
        auto_summary_prompt: "Start with decisions.",
      },
    );

    expect(result.promptOverride).toBe("Start with decisions.");
    expect(result).not.toHaveProperty("template");
  });

  it("uses the built-in prompt when no override is saved", async () => {
    const result = await enhanceTransform.transformArgs(
      { sessionId: "session-1", enhancedNoteId: "note-1" },
      settingsValues,
    );

    expect(result.promptOverride).toBe("");
  });

  it("collects image context from canonical transcript and note content", async () => {
    await enhanceTransform.transformArgs(
      {
        sessionId: "session-1",
        enhancedNoteId: "note-1",
      },
      {
        current_llm_provider: "openai",
        current_llm_model: "gpt-4o",
        ai_language: "en",
      },
    );

    expect(mocks.collectEnhanceImageContext).toHaveBeenCalledWith("session-1", [
      "![pre](asset://localhost/pre.png)",
      "![post](asset://localhost/post.png)",
    ]);
  });

  it("builds the render request straight from the transcript's own owner, without a humans lookup", async () => {
    await enhanceTransform.transformArgs(
      { sessionId: "session-1", enhancedNoteId: "note-1" },
      settingsValues,
    );

    expect(mocks.buildRenderTranscriptRequestFromRows).toHaveBeenCalledWith(
      expect.any(Array),
      {
        selfHumanId: "user-1",
        humans: [],
      },
    );
  });

  it("rejects generation when the session no longer exists", async () => {
    mocks.loadSessionContentSnapshot.mockResolvedValue(null);

    await expect(
      enhanceTransform.transformArgs(
        { sessionId: "missing", enhancedNoteId: "note-1" },
        settingsValues,
      ),
    ).rejects.toThrow("Session missing no longer exists");
  });
});

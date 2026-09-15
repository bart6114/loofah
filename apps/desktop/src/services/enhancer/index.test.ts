import type { LanguageModel } from "ai";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { EnhancerService } from ".";
import { selectSummaryDocument } from "./storage";

import { enqueueDatabaseWrite } from "~/shared/write-queue";

const mocks = vi.hoisted(() => ({
  loadSessionContentSnapshot: vi.fn(),
  ensureSummaryDocument: vi.fn(),
  listenerSubscribe: vi.fn(),
}));

vi.mock("~/session/content-queries", () => ({
  loadSessionContentSnapshot: mocks.loadSessionContentSnapshot,
}));

vi.mock("./storage", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./storage")>()),
  ensureSummaryDocument: mocks.ensureSummaryDocument,
}));

vi.mock("~/store/zustand/listener/instance", () => ({
  listenerStore: {
    subscribe: mocks.listenerSubscribe,
  },
}));

function createNote(overrides: Record<string, any> = {}): any {
  return {
    id: "note-1",
    title: "Summary",
    markdown: "",
    content: "",
    contentFormat: "prosemirror_json",
    templateId: "",
    kind: "summary",
    position: 1,
    ...overrides,
  };
}

function createSnapshot({
  notes = [],
  wordCount = 0,
}: {
  notes?: any[];
  wordCount?: number;
} = {}) {
  return {
    sessionId: "session-1",
    ownerUserId: "user-1",
    title: "Planning",
    createdAt: "2026-07-10T00:00:00.000Z",
    event: null,
    eventId: null,
    rawNoteId: "session-1",
    rawContent: "",
    rawContentFormat: "prosemirror_json",
    rawMarkdown: "Planning notes",
    enhancedNotes: notes,
    transcripts:
      wordCount > 0
        ? [
            {
              id: "transcript-1",
              started_at: 0,
              ended_at: 1,
              memo: "",
              wordsJson: "[]",
              words: Array.from({ length: wordCount }, (_, index) => ({
                id: `word-${index}`,
                text: "word",
                start_ms: index,
                end_ms: index + 1,
              })),
              speaker_hints: [],
            },
          ]
        : [],
    participants: [],
  };
}

function createMockAITaskStore(
  getTaskState: (taskId: string) => unknown = () => undefined,
) {
  const generate = vi.fn().mockResolvedValue(undefined);
  const reset = vi.fn();
  const store = {
    getState: vi.fn(() => ({
      generate,
      reset,
      getState: vi.fn(getTaskState),
    })),
  } as unknown as ConstructorParameters<
    typeof EnhancerService
  >[0]["aiTaskStore"];

  return {
    generate,
    reset,
    store,
  };
}

function createDeps(
  overrides: Partial<ConstructorParameters<typeof EnhancerService>[0]> = {},
): ConstructorParameters<typeof EnhancerService>[0] {
  return {
    aiTaskStore: createMockAITaskStore().store,
    getModel: () => ({}) as LanguageModel,
    ...overrides,
  };
}

describe("EnhancerService", () => {
  let snapshot: ReturnType<typeof createSnapshot>;
  let listener: ((state: any) => void) | undefined;
  let unsubscribe: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    snapshot = createSnapshot();
    unsubscribe = vi.fn();
    mocks.listenerSubscribe.mockImplementation((callback) => {
      listener = callback;
      return unsubscribe;
    });
    mocks.loadSessionContentSnapshot.mockImplementation(async () => snapshot);
    mocks.ensureSummaryDocument.mockImplementation(async () => {
      const existing = selectSummaryDocument(snapshot.enhancedNotes);
      if (existing) return existing;

      const note = createNote({
        id: `note-${snapshot.enhancedNotes.length + 1}`,
        position: snapshot.enhancedNotes.length + 1,
      });
      snapshot.enhancedNotes.push(note);
      return note;
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("rejects empty input before creating or clearing a document", async () => {
    const service = new EnhancerService(createDeps());
    snapshot.rawMarkdown = "&nbsp;";
    await expect(
      service.enhance("session-1", { targetNoteId: "note-1" }),
    ).rejects.toThrow("Add a note or transcript");
    expect(mocks.ensureSummaryDocument).not.toHaveBeenCalled();
  });

  it("waits for pending note writes before reading the source", async () => {
    const service = new EnhancerService(createDeps());
    snapshot.rawMarkdown = "";
    let finishWrite!: () => void;
    const gate = new Promise<void>((resolve) => {
      finishWrite = resolve;
    });
    const write = enqueueDatabaseWrite("session:session-1:note", async () => {
      await gate;
      snapshot.rawMarkdown = "Ship Friday";
    });
    const generation = service.enhance("session-1");
    await Promise.resolve();
    expect(mocks.loadSessionContentSnapshot).not.toHaveBeenCalled();
    finishWrite();
    await write;
    await expect(generation).resolves.toMatchObject({ type: "started" });
  });

  it("deduplicates simultaneous starts", async () => {
    const ai = createMockAITaskStore();
    const service = new EnhancerService(createDeps({ aiTaskStore: ai.store }));
    await Promise.all([
      service.enhance("session-1"),
      service.enhance("session-1"),
    ]);
    expect(mocks.ensureSummaryDocument).toHaveBeenCalledTimes(1);
    expect(ai.generate).toHaveBeenCalledTimes(1);
  });

  it("returns no_model without touching session storage", async () => {
    const service = new EnhancerService(createDeps({ getModel: () => null }));

    await expect(service.enhance("session-1")).resolves.toEqual({
      type: "no_model",
    });
    expect(mocks.loadSessionContentSnapshot).not.toHaveBeenCalled();
  });

  it("creates the stored summary before starting generation", async () => {
    const ai = createMockAITaskStore();
    const service = new EnhancerService(createDeps({ aiTaskStore: ai.store }));

    const result = await service.enhance("session-1");

    expect(result).toEqual({ type: "started", noteId: "note-1" });
    expect(mocks.ensureSummaryDocument).toHaveBeenCalledWith("session-1");
    expect(mocks.ensureSummaryDocument).toHaveBeenCalledBefore(ai.generate);
    expect(ai.generate).toHaveBeenCalledWith("note-1-enhance", {
      model: expect.any(Object),
      taskType: "enhance",
      args: {
        sessionId: "session-1",
        enhancedNoteId: "note-1",
      },
    });
  });

  it("reuses an existing legacy summary for automatic generation", async () => {
    snapshot = createSnapshot({
      notes: [createNote({ id: "existing", templateId: "one-on-one" })],
    });
    const ai = createMockAITaskStore();
    const service = new EnhancerService(createDeps({ aiTaskStore: ai.store }));

    const result = await service.enhance("session-1", { isAuto: true });

    expect(result).toEqual({ type: "started", noteId: "existing" });
    expect(mocks.ensureSummaryDocument).not.toHaveBeenCalled();
    expect(ai.generate).toHaveBeenCalledWith(
      "existing-enhance",
      expect.objectContaining({
        args: { sessionId: "session-1", enhancedNoteId: "existing" },
      }),
    );
  });

  it("returns already_active while the note task is generating", async () => {
    snapshot = createSnapshot({ notes: [createNote()] });
    const ai = createMockAITaskStore(() => ({ status: "generating" }));
    const service = new EnhancerService(createDeps({ aiTaskStore: ai.store }));

    await expect(service.enhance("session-1")).resolves.toEqual({
      type: "already_active",
      noteId: "note-1",
    });
    expect(ai.generate).not.toHaveBeenCalled();
  });

  it("does not rerun a successful task with durable summary content", async () => {
    snapshot = createSnapshot({
      notes: [
        createNote({
          content:
            '{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Saved"}]}]}',
        }),
      ],
    });
    const ai = createMockAITaskStore(() => ({ status: "success" }));
    const service = new EnhancerService(createDeps({ aiTaskStore: ai.store }));

    await expect(service.enhance("session-1")).resolves.toMatchObject({
      type: "already_active",
    });
    expect(ai.generate).not.toHaveBeenCalled();
  });

  it("refreshes the same successful summary after a resumed recording", async () => {
    snapshot = createSnapshot({
      notes: [createNote({ content: "Old recording" })],
      wordCount: 50,
    });
    const ai = createMockAITaskStore(() => ({ status: "success" }));
    const service = new EnhancerService(createDeps({ aiTaskStore: ai.store }));
    await expect(
      service.enhance("session-1", { isAuto: true }),
    ).resolves.toEqual({ type: "started", noteId: "note-1" });
    expect(ai.generate).toHaveBeenCalledOnce();
    expect(mocks.ensureSummaryDocument).not.toHaveBeenCalled();
  });

  it("reruns a successful task whose summary is still empty", async () => {
    snapshot = createSnapshot({ notes: [createNote()] });
    const ai = createMockAITaskStore(() => ({ status: "success" }));
    const service = new EnhancerService(createDeps({ aiTaskStore: ai.store }));

    await expect(service.enhance("session-1")).resolves.toMatchObject({
      type: "started",
    });
    expect(ai.generate).toHaveBeenCalledOnce();
  });

  it("regenerates the selected legacy document without replacing its metadata", async () => {
    const note = createNote({
      kind: "template_output",
      templateId: "old",
      title: "Customer review",
      content: "Saved",
    });
    snapshot = createSnapshot({
      notes: [createNote({ id: "ordinary" }), note],
    });
    const ai = createMockAITaskStore(() => ({ status: "success" }));
    const service = new EnhancerService(createDeps({ aiTaskStore: ai.store }));
    await expect(
      service.enhance("session-1", { targetNoteId: note.id }),
    ).resolves.toEqual({ type: "started", noteId: note.id });
    expect(ai.generate).toHaveBeenCalledWith(
      "note-1-enhance",
      expect.objectContaining({
        args: { sessionId: "session-1", enhancedNoteId: note.id },
      }),
    );
    expect(note).toMatchObject({
      title: "Customer review",
      templateId: "old",
      content: "Saved",
    });
    expect(mocks.ensureSummaryDocument).not.toHaveBeenCalled();
  });

  it("does not queue auto-enhance when a durable summary exists", async () => {
    snapshot = createSnapshot({
      notes: [createNote({ content: "Saved summary" })],
      wordCount: 10,
    });
    const service = new EnhancerService(createDeps());
    const queueSpy = vi.spyOn(service, "queueAutoEnhance");

    await expect(
      service.queueAutoEnhanceIfSummaryEmpty("session-1"),
    ).resolves.toEqual({ type: "summary_exists", noteId: "note-1" });
    expect(queueSpy).not.toHaveBeenCalled();
  });

  it("creates a visible empty summary when a short transcript cannot enhance", async () => {
    snapshot = createSnapshot({ wordCount: 2 });
    const service = new EnhancerService(createDeps());
    const queueSpy = vi
      .spyOn(service, "queueAutoEnhance")
      .mockImplementation(() => {});

    await expect(
      service.queueAutoEnhanceIfSummaryEmpty("session-1"),
    ).resolves.toEqual({ type: "queued" });
    expect(mocks.ensureSummaryDocument).toHaveBeenCalledWith("session-1");
    expect(queueSpy).toHaveBeenCalledWith("session-1");
  });

  it("computes eligibility from canonical transcript words", async () => {
    const service = new EnhancerService(createDeps());

    snapshot = createSnapshot();
    await expect(service.checkEligibility("session-1")).resolves.toMatchObject({
      eligible: false,
      reason: "No transcript recorded",
    });
    snapshot = createSnapshot({ wordCount: 4 });
    await expect(service.checkEligibility("session-1")).resolves.toMatchObject({
      eligible: false,
      wordCount: 4,
    });
    snapshot = createSnapshot({ wordCount: 5 });
    await expect(service.checkEligibility("session-1")).resolves.toMatchObject({
      eligible: false,
      characterCount: 24,
      reason: "Transcript too short to summarize (24/160 characters minimum)",
      wordCount: 5,
    });
    snapshot = createSnapshot({ wordCount: 40 });
    await expect(service.checkEligibility("session-1")).resolves.toEqual({
      eligible: true,
      characterCount: 199,
      wordCount: 40,
    });
  });

  it("resets every canonical summary task", async () => {
    snapshot = createSnapshot({
      notes: [createNote({ id: "one" }), createNote({ id: "two" })],
    });
    const ai = createMockAITaskStore();
    const service = new EnhancerService(createDeps({ aiTaskStore: ai.store }));

    await service.resetEnhanceTasks("session-1");

    expect(ai.reset).toHaveBeenCalledWith("one-enhance");
    expect(ai.reset).toHaveBeenCalledWith("two-enhance");
  });

  it("deduplicates eligible auto-enhance requests", async () => {
    snapshot = createSnapshot({ wordCount: 40 });
    const ai = createMockAITaskStore();
    const service = new EnhancerService(createDeps({ aiTaskStore: ai.store }));

    service.queueAutoEnhance("session-1");
    service.queueAutoEnhance("session-1");

    await vi.waitFor(() => expect(ai.generate).toHaveBeenCalledOnce());
  });

  it("emits no-model and started auto-enhance outcomes", async () => {
    snapshot = createSnapshot({ wordCount: 40 });
    const noModelService = new EnhancerService(
      createDeps({ getModel: () => null }),
    );
    const noModelEvent = vi.fn();
    noModelService.on(noModelEvent);

    noModelService.queueAutoEnhance("session-1");
    await vi.waitFor(() =>
      expect(noModelEvent).toHaveBeenCalledWith({
        type: "auto-enhance-no-model",
        sessionId: "session-1",
      }),
    );

    const startedService = new EnhancerService(createDeps());
    const startedEvent = vi.fn();
    startedService.on(startedEvent);
    startedService.queueAutoEnhance("session-1");
    await vi.waitFor(() =>
      expect(startedEvent).toHaveBeenCalledWith({
        type: "auto-enhance-started",
        sessionId: "session-1",
        noteId: "note-1",
      }),
    );
  });

  it("retries short transcripts and eventually emits the skip reason", async () => {
    vi.useFakeTimers();
    snapshot = createSnapshot({ wordCount: 1 });
    const service = new EnhancerService(createDeps());
    const event = vi.fn();
    service.on(event);

    service.queueAutoEnhance("session-1");
    await vi.advanceTimersByTimeAsync(10_500);

    expect(event).toHaveBeenCalledWith({
      type: "auto-enhance-skipped",
      sessionId: "session-1",
      reason: "Not enough words recorded (1/5 minimum)",
      reasonCode: "transcript_too_short",
    });
  });

  it("cancels a pending retry when the session becomes active", async () => {
    vi.useFakeTimers();
    snapshot = createSnapshot({ wordCount: 1 });
    const service = new EnhancerService(createDeps());
    const event = vi.fn();
    service.on(event);
    service.start();

    service.queueAutoEnhance("session-1");
    await vi.advanceTimersByTimeAsync(100);
    listener?.({ live: { status: "active", sessionId: "session-1" } });
    await vi.advanceTimersByTimeAsync(20_000);

    expect(event).not.toHaveBeenCalled();
  });

  it("disposes listener subscriptions and pending timers", async () => {
    vi.useFakeTimers();
    snapshot = createSnapshot({ wordCount: 1 });
    const service = new EnhancerService(createDeps());
    service.start();
    service.queueAutoEnhance("session-1");
    await vi.advanceTimersByTimeAsync(100);

    service.dispose();
    await vi.advanceTimersByTimeAsync(20_000);

    expect(unsubscribe).toHaveBeenCalledOnce();
  });
});

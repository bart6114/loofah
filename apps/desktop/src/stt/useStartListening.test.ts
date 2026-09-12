import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { getSessionKeywords } from "./useKeywords";
import { getPostCaptureAction, useStartListening } from "./useStartListening";

import { enqueueSessionAudioOperation } from "~/session/audio-operations";

const {
  queueAutoEnhanceMock,
  queueAutoEnhanceIfSummaryEmptyMock,
  resetEnhanceTasksMock,
  startMock,
  runBatchMock,
  canRunBatchTranscriptionMock,
  useListenerMock,
  useSessionMock,
  useSessionHasTranscriptMock,
  sessionAppendTranscriptMock,
  sessionFlushTranscriptMock,
  softDeleteTranscriptMock,
  useConfigValueMock,
  useSTTConnectionMock,
  isSupportedLanguagesLiveMock,
  leftSidebarExpanded,
  setLeftSidebarExpandedMock,
  sonnerToastWarningMock,
  sonnerToastErrorMock,
  catalogLocalSessionAudioMock,
  getEnhancerServiceMock,
  requestMainAutoEnhanceMock,
  queueTagSuggestionsMock,
} = vi.hoisted(() => ({
  queueAutoEnhanceMock: vi.fn(),
  queueAutoEnhanceIfSummaryEmptyMock: vi.fn(),
  resetEnhanceTasksMock: vi.fn(),
  startMock: vi.fn(),
  runBatchMock: vi.fn(),
  canRunBatchTranscriptionMock: vi.fn(() => true),
  useListenerMock: vi.fn(),
  useSessionMock: vi.fn(),
  useSessionHasTranscriptMock: vi.fn(),
  sessionAppendTranscriptMock: vi.fn(),
  sessionFlushTranscriptMock: vi.fn(),
  softDeleteTranscriptMock: vi.fn(),
  useConfigValueMock: vi.fn(),
  useSTTConnectionMock: vi.fn(),
  isSupportedLanguagesLiveMock: vi.fn(),
  leftSidebarExpanded: { value: true },
  setLeftSidebarExpandedMock: vi.fn(),
  sonnerToastWarningMock: vi.fn(),
  sonnerToastErrorMock: vi.fn(),
  catalogLocalSessionAudioMock: vi.fn(),
  getEnhancerServiceMock: vi.fn(),
  requestMainAutoEnhanceMock: vi.fn(),
  queueTagSuggestionsMock: vi.fn(),
}));

vi.mock("@hypr/plugin-transcription", () => ({
  commands: {
    isSupportedLanguagesLive: isSupportedLanguagesLiveMock,
  },
}));

vi.mock("./contexts", () => ({
  useListener: useListenerMock,
}));

vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: {
    warning: sonnerToastWarningMock,
    error: sonnerToastErrorMock,
  },
}));

vi.mock("~/ai/task-window-sync", () => ({
  requestMainAutoEnhance: requestMainAutoEnhanceMock,
}));

vi.mock("./useKeywords", () => ({
  getSessionKeywords: vi.fn(async () => []),
  useKeywords: vi.fn(() => []),
}));

vi.mock("./useRunBatch", () => ({
  STOPPED_TRANSCRIPTION_ERROR_MESSAGE: "Transcription stopped.",
  canRunBatchTranscription: canRunBatchTranscriptionMock,
  isStoppedTranscriptionError: vi.fn(
    (error: unknown) =>
      (error instanceof Error ? error.message : String(error)) ===
      "Transcription stopped.",
  ),
  useRunBatch: vi.fn(() => runBatchMock),
}));

vi.mock("./useSTTConnection", () => ({
  useSTTConnection: useSTTConnectionMock,
}));

vi.mock("~/services/enhancer", () => ({
  getEnhancerService: getEnhancerServiceMock,
}));

vi.mock("~/session/attachments", () => ({
  catalogLocalSessionAudio: catalogLocalSessionAudioMock,
}));

vi.mock("~/contexts/shell", () => ({
  useShell: vi.fn(() => ({
    leftsidebar: {
      expanded: leftSidebarExpanded.value,
      setExpanded: setLeftSidebarExpandedMock,
    },
  })),
}));

vi.mock("~/session/utils", () => ({
  getSessionEvent: vi.fn(() => null),
}));

vi.mock("~/session/queries", () => ({
  useSession: useSessionMock,
  useSessionHasTranscript: useSessionHasTranscriptMock,
}));

vi.mock("~/shared/config", () => ({
  useConfigValue: useConfigValueMock,
}));

vi.mock("~/shared/utils", () => ({
  id: vi.fn(() => "generated-id"),
}));

vi.mock("~/stt/queries", () => ({
  softDeleteTranscript: softDeleteTranscriptMock,
}));

vi.mock("~/tags/suggestions", () => ({
  queueTagSuggestions: queueTagSuggestionsMock,
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionAppendTranscript: sessionAppendTranscriptMock,
    sessionFlushTranscript: sessionFlushTranscriptMock,
  },
}));

describe("getPostCaptureAction", () => {
  test("runs batch then enhance after record-only capture finishes when audio is available", () => {
    expect(
      getPostCaptureAction(
        {
          audioPath: "/tmp/session.wav",
          liveTranscriptionActive: false,
          needsBatchRepair: false,
        },
        true,
      ),
    ).toBe("batch_then_enhance");
  });

  test("enhances immediately when live transcription already completed during recording", () => {
    expect(
      getPostCaptureAction(
        {
          audioPath: "/tmp/session.wav",
          liveTranscriptionActive: true,
          needsBatchRepair: false,
        },
        true,
      ),
    ).toBe("enhance_only");
  });

  test("repairs the full transcript after live transcription recovered", () => {
    expect(
      getPostCaptureAction(
        {
          audioPath: "/tmp/session.wav",
          liveTranscriptionActive: true,
          needsBatchRepair: true,
        },
        true,
      ),
    ).toBe("batch_then_enhance");
  });

  // REGRESSION: `liveTranscriptionActive` reports the configured transcription *mode*, not
  // whether the stream ever emitted a word. A live stream that opened and died produced a
  // session with no transcript, no batch fallback and no error at all.
  test("falls back to batch when live transcription was active but produced no words", () => {
    expect(
      getPostCaptureAction(
        {
          audioPath: "/tmp/session.wav",
          liveTranscriptionActive: true,
          needsBatchRepair: false,
        },
        true,
        true,
      ),
    ).toBe("batch_then_enhance");
  });

  test("enhances without re-transcribing when live transcription did produce words", () => {
    expect(
      getPostCaptureAction(
        {
          audioPath: "/tmp/session.wav",
          liveTranscriptionActive: true,
          needsBatchRepair: false,
        },
        true,
        false,
      ),
    ).toBe("enhance_only");
  });

  test("does nothing when batch fallback is needed but no transcription connection is available", () => {
    expect(
      getPostCaptureAction(
        {
          audioPath: "/tmp/session.wav",
          liveTranscriptionActive: false,
          needsBatchRepair: false,
        },
        false,
      ),
    ).toBe("none");
  });

  test("does nothing when capture finishes without a saved audio path", () => {
    expect(
      getPostCaptureAction(
        {
          audioPath: null,
          liveTranscriptionActive: false,
          needsBatchRepair: false,
        },
        true,
      ),
    ).toBe("none");
  });
});

describe("useStartListening", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    queueTagSuggestionsMock.mockResolvedValue(undefined);

    getEnhancerServiceMock.mockImplementation(() => ({
      queueAutoEnhance: queueAutoEnhanceMock,
      queueAutoEnhanceIfSummaryEmpty: queueAutoEnhanceIfSummaryEmptyMock,
      resetEnhanceTasks: resetEnhanceTasksMock,
    }));
    useListenerMock.mockImplementation((selector) =>
      selector({
        start: startMock,
      }),
    );
    useSessionMock.mockReturnValue({
      id: "session-1",
      user_id: "user-1",
      raw_md: "Existing memo",
    });
    useSessionHasTranscriptMock.mockReturnValue(false);
    sessionAppendTranscriptMock.mockResolvedValue({ status: "ok", data: null });
    sessionFlushTranscriptMock.mockResolvedValue({ status: "ok", data: null });
    softDeleteTranscriptMock.mockResolvedValue(undefined);
    catalogLocalSessionAudioMock.mockResolvedValue(
      "/vault/sessions/session-1/audio.wav",
    );
    useConfigValueMock.mockImplementation((key) =>
      key === "ai_language"
        ? "en"
        : key === "meeting_languages"
          ? undefined
          : [],
    );
    leftSidebarExpanded.value = true;
    useSTTConnectionMock.mockReturnValue({
      conn: {
        provider: "fmtr",
        model: "am-test",
        baseUrl: "http://localhost:8080",
        apiKey: "",
      },
    });
    startMock.mockResolvedValue(true);
    runBatchMock.mockResolvedValue(undefined);
    canRunBatchTranscriptionMock.mockReturnValue(true);
    isSupportedLanguagesLiveMock.mockResolvedValue({
      status: "ok",
      data: true,
    });
  });

  test("collapses the left sidebar after listening starts", async () => {
    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(setLeftSidebarExpandedMock).toHaveBeenCalledWith(false);
  });

  test("sets the left sidebar collapsed after listening starts even if render state is stale", async () => {
    leftSidebarExpanded.value = false;

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(setLeftSidebarExpandedMock).toHaveBeenCalledWith(false);
  });

  test("keeps the left sidebar state when listening fails to start", async () => {
    startMock.mockResolvedValue(false);
    useConfigValueMock.mockImplementation((key: string) =>
      key === "ai_language"
        ? "en"
        : key === "meeting_languages"
          ? undefined
          : [],
    );

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(setLeftSidebarExpandedMock).not.toHaveBeenCalled();
  });

  test("reads keywords from the same pre-start snapshot as the transcript memo", async () => {
    const calls: string[] = [];
    vi.mocked(getSessionKeywords).mockImplementation(async () => {
      calls.push("keywords");
      return ["launch"];
    });
    startMock.mockImplementation(async () => {
      calls.push("start");
      return true;
    });

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(calls).toEqual(["keywords", "start"]);
    expect(startMock.mock.calls[0]?.[0]).toMatchObject({
      keywords: ["launch"],
    });
  });

  test("runs batch transcription after record-only capture stops", async () => {
    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    expect(onStopped).toBeTypeOf("function");

    await act(async () => {
      await onStopped?.("session-1", {
        durationSeconds: 42,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: false,
        liveTranscriptionActive: false,
        needsBatchRepair: false,
      });
    });

    // REGRESSION: cataloging can relocate the recording, and this ran batch transcription
    // against the pre-catalog path — which failed with ENOENT, leaving every record-only
    // session with no transcript at all. Batch must read where the audio ended up.
    expect(catalogLocalSessionAudioMock).toHaveBeenCalledWith(
      "session-1",
      "/tmp/session.wav",
    );
    expect(
      catalogLocalSessionAudioMock.mock.invocationCallOrder[0],
    ).toBeLessThan(runBatchMock.mock.invocationCallOrder[0]!);
    expect(runBatchMock).toHaveBeenCalledWith(
      "/vault/sessions/session-1/audio.wav",
    );
    expect(queueAutoEnhanceIfSummaryEmptyMock).toHaveBeenCalledWith(
      "session-1",
    );
  });

  test("skips post-capture batch when the connection cannot run batch transcription", async () => {
    canRunBatchTranscriptionMock.mockReturnValue(false);

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    await act(async () => {
      await onStopped?.("session-1", {
        durationSeconds: 42,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: false,
        liveTranscriptionActive: false,
        needsBatchRepair: false,
      });
    });

    expect(canRunBatchTranscriptionMock).toHaveBeenCalledWith(
      expect.objectContaining({ provider: "fmtr", model: "am-test" }),
    );
    expect(runBatchMock).not.toHaveBeenCalled();
    expect(queueAutoEnhanceIfSummaryEmptyMock).not.toHaveBeenCalled();
  });

  test("shows a toast when moving recorded audio into the session folder fails", async () => {
    catalogLocalSessionAudioMock.mockRejectedValueOnce(new Error("disk full"));
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    await act(async () => {
      await onStopped?.("session-1", {
        durationSeconds: 42,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: false,
        liveTranscriptionActive: false,
        needsBatchRepair: false,
      });
    });

    expect(sonnerToastErrorMock).toHaveBeenCalledWith(
      "Recording audio could not be moved into the session folder — it remains at its original location",
      { id: "audio-catalog-failed" },
    );
    consoleError.mockRestore();
  });

  // REGRESSION: this is the shape of a real lost recording — live mode selected, the stream
  // reported itself active, but not one word was persisted. The app concluded live
  // transcription had succeeded and skipped the batch fallback, so the session ended up with
  // no transcript and no error anywhere.
  test("transcribes the recording when a live capture persists no words at all", async () => {
    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    await act(async () => {
      await onStopped?.("session-1", {
        durationSeconds: 12,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: true,
        liveTranscriptionActive: true,
        needsBatchRepair: false,
      });
    });

    expect(runBatchMock).toHaveBeenCalledWith(
      "/vault/sessions/session-1/audio.wav",
    );
    expect(queueAutoEnhanceIfSummaryEmptyMock).toHaveBeenCalledWith(
      "session-1",
    );
  });

  test("does not re-transcribe when a live capture already persisted words", async () => {
    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const callbacks = startMock.mock.calls[0]?.[1];
    callbacks?.handlePersist?.({
      new_words: [
        { id: "word-1", text: "hello", start_ms: 0, end_ms: 100, channel: 0 },
      ],
      replaced_ids: [],
      partials: [],
    });

    await act(async () => {
      await callbacks?.onStopped?.("session-1", {
        durationSeconds: 12,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: true,
        liveTranscriptionActive: true,
        needsBatchRepair: false,
      });
    });

    expect(runBatchMock).not.toHaveBeenCalled();
  });

  test("transcribes the recording where capture left it when cataloging fails", async () => {
    catalogLocalSessionAudioMock.mockRejectedValueOnce(new Error("disk full"));
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    await act(async () => {
      await onStopped?.("session-1", {
        durationSeconds: 42,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: false,
        liveTranscriptionActive: false,
        needsBatchRepair: false,
      });
    });

    expect(runBatchMock).toHaveBeenCalledWith("/tmp/session.wav");
    consoleError.mockRestore();
  });

  test("skips audio cataloging when capture produces no final file", async () => {
    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    await act(async () => {
      await onStopped?.("session-1", {
        durationSeconds: 0,
        audioPath: null,
        requestedLiveTranscription: false,
        liveTranscriptionActive: false,
        needsBatchRepair: false,
      });
    });

    expect(catalogLocalSessionAudioMock).not.toHaveBeenCalled();
  });

  test("catalogs finalized audio even when transcript persistence fails", async () => {
    sessionAppendTranscriptMock.mockResolvedValueOnce({
      status: "error",
      error: "write failed",
    });
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const callbacks = startMock.mock.calls[0]?.[1];
    callbacks?.handlePersist?.({
      new_words: [
        {
          id: "word-1",
          text: "hello",
          start_ms: 0,
          end_ms: 100,
          channel: 0,
        },
      ],
      replaced_ids: [],
      partials: [],
    });

    await act(async () => {
      await callbacks?.onStopped?.("session-1", {
        durationSeconds: 1,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: true,
        liveTranscriptionActive: true,
        needsBatchRepair: false,
      });
    });

    expect(catalogLocalSessionAudioMock).toHaveBeenCalledWith(
      "session-1",
      "/tmp/session.wav",
    );
    expect(runBatchMock).not.toHaveBeenCalled();
    expect(sonnerToastErrorMock).toHaveBeenCalledWith(
      expect.stringContaining("Transcript is NOT being saved"),
      { id: "live-transcript-persist-failed", duration: Infinity },
    );
    expect(queueAutoEnhanceIfSummaryEmptyMock).toHaveBeenCalledWith(
      "session-1",
    );
    consoleError.mockRestore();
  });

  test("still summarizes the live transcript when the batch repair fails", async () => {
    useSessionHasTranscriptMock.mockReturnValue(true);
    runBatchMock.mockRejectedValueOnce(new Error("upload failed"));
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    await act(async () => {
      await onStopped?.("session-1", {
        durationSeconds: 42,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: true,
        liveTranscriptionActive: true,
        needsBatchRepair: true,
      });
    });

    expect(sonnerToastErrorMock).toHaveBeenCalledWith(
      "Post-meeting transcription failed. Summarizing the live transcript instead.",
      { id: "post-capture-batch-failed" },
    );
    expect(queueAutoEnhanceIfSummaryEmptyMock).toHaveBeenCalledWith(
      "session-1",
    );
    expect(queueAutoEnhanceMock).not.toHaveBeenCalled();
    consoleError.mockRestore();
  });

  test("still tries to summarize the live transcript when the batch repair fails without other transcripts", async () => {
    runBatchMock.mockRejectedValueOnce(new Error("upload failed"));
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    await act(async () => {
      await onStopped?.("session-1", {
        durationSeconds: 42,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: false,
        liveTranscriptionActive: false,
        needsBatchRepair: false,
      });
    });

    expect(queueAutoEnhanceIfSummaryEmptyMock).toHaveBeenCalledWith(
      "session-1",
    );
    consoleError.mockRestore();
  });

  test("stays quiet when a record-only stop leaves nothing to summarize", async () => {
    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    await act(async () => {
      await onStopped?.("session-1", {
        durationSeconds: 1,
        audioPath: null,
        requestedLiveTranscription: false,
        liveTranscriptionActive: false,
        needsBatchRepair: false,
      });
    });

    expect(runBatchMock).not.toHaveBeenCalled();
    expect(queueAutoEnhanceMock).not.toHaveBeenCalled();
    expect(queueAutoEnhanceIfSummaryEmptyMock).not.toHaveBeenCalled();
    expect(requestMainAutoEnhanceMock).not.toHaveBeenCalled();
  });

  test("does not auto-enhance after the user cancels the batch repair", async () => {
    useSessionHasTranscriptMock.mockReturnValue(true);
    runBatchMock.mockRejectedValueOnce(new Error("Transcription stopped."));

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    await act(async () => {
      await onStopped?.("session-1", {
        durationSeconds: 42,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: false,
        liveTranscriptionActive: false,
        needsBatchRepair: false,
      });
    });

    expect(queueAutoEnhanceMock).not.toHaveBeenCalled();
    expect(queueAutoEnhanceIfSummaryEmptyMock).not.toHaveBeenCalled();
    expect(sonnerToastErrorMock).not.toHaveBeenCalled();
  });

  test("forwards auto-enhance to the main window when no enhancer service exists", async () => {
    getEnhancerServiceMock.mockReturnValue(null);
    useSessionHasTranscriptMock.mockReturnValue(true);

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    await act(async () => {
      await onStopped?.("session-1", {
        durationSeconds: 42,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: false,
        liveTranscriptionActive: false,
        needsBatchRepair: false,
      });
    });

    expect(requestMainAutoEnhanceMock).toHaveBeenCalledWith(
      "session-1",
      "regenerate",
    );
    expect(queueAutoEnhanceMock).not.toHaveBeenCalled();
    expect(queueAutoEnhanceIfSummaryEmptyMock).not.toHaveBeenCalled();
  });

  test("catalogs finalized audio through the session audio queue", async () => {
    let releaseBlocker: (() => void) | undefined;
    const blocker = enqueueSessionAudioOperation(
      "session-1",
      () =>
        new Promise<void>((resolve) => {
          releaseBlocker = resolve;
        }),
    );
    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    const stopped = onStopped?.("session-1", {
      durationSeconds: 1,
      audioPath: "/tmp/session.wav",
      requestedLiveTranscription: true,
      liveTranscriptionActive: true,
      needsBatchRepair: false,
    });
    await Promise.resolve();
    expect(catalogLocalSessionAudioMock).not.toHaveBeenCalled();

    releaseBlocker?.();
    await blocker;
    await act(async () => await stopped);
    expect(catalogLocalSessionAudioMock).toHaveBeenCalledWith(
      "session-1",
      "/tmp/session.wav",
    );
  });

  test("cleans up processed audio after live capture stops", async () => {
    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const callbacks = startMock.mock.calls[0]?.[1];
    callbacks?.handlePersist?.({
      new_words: [
        { id: "word-1", text: "hello", start_ms: 0, end_ms: 100, channel: 0 },
      ],
      replaced_ids: [],
      partials: [],
    });

    await act(async () => {
      await callbacks?.onStopped?.("session-1", {
        durationSeconds: 42,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: true,
        liveTranscriptionActive: true,
        needsBatchRepair: false,
      });
    });

    expect(runBatchMock).not.toHaveBeenCalled();
    expect(queueAutoEnhanceIfSummaryEmptyMock).toHaveBeenCalledWith(
      "session-1",
    );
  });

  test("regenerates the summary after resumed live capture writes transcript", async () => {
    let resolveTranscriptWrite:
      | ((value: { status: "ok"; data: null }) => void)
      | undefined;
    sessionAppendTranscriptMock.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveTranscriptWrite = resolve;
        }),
    );
    useSessionHasTranscriptMock.mockReturnValue(true);

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const handlePersist = startMock.mock.calls[0]?.[1]?.handlePersist;
    expect(handlePersist).toBeTypeOf("function");

    act(() => {
      handlePersist?.({
        new_words: [
          {
            id: "new-word",
            text: "new",
            start_ms: 100,
            end_ms: 200,
            channel: 0,
          },
        ],
        replaced_ids: [],
        partials: [],
      });
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;
    const stopped = onStopped?.("session-1", {
      durationSeconds: 42,
      audioPath: "/tmp/session.wav",
      requestedLiveTranscription: true,
      liveTranscriptionActive: true,
      needsBatchRepair: false,
    });

    expect(resetEnhanceTasksMock).not.toHaveBeenCalled();
    resolveTranscriptWrite?.({ status: "ok", data: null });
    await act(async () => await stopped);

    expect(sessionAppendTranscriptMock).toHaveBeenCalledTimes(1);
    expect(resetEnhanceTasksMock).toHaveBeenCalledWith("session-1");
    expect(queueAutoEnhanceMock).toHaveBeenCalledWith("session-1");
    expect(queueAutoEnhanceIfSummaryEmptyMock).not.toHaveBeenCalled();
  });

  test("regenerates the summary after resumed batch capture completes", async () => {
    useSessionHasTranscriptMock.mockReturnValue(true);

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    const onStopped = startMock.mock.calls[0]?.[1]?.onStopped;

    await act(async () => {
      await onStopped?.("session-1", {
        durationSeconds: 42,
        audioPath: "/tmp/session.wav",
        requestedLiveTranscription: false,
        liveTranscriptionActive: false,
        needsBatchRepair: false,
      });
    });

    expect(runBatchMock).toHaveBeenCalledWith(
      "/vault/sessions/session-1/audio.wav",
    );
    expect(resetEnhanceTasksMock).toHaveBeenCalledWith("session-1");
    expect(queueAutoEnhanceMock).toHaveBeenCalledWith("session-1");
    expect(queueAutoEnhanceIfSummaryEmptyMock).not.toHaveBeenCalled();
  });

  test("forces batch transcription for batch-only local models with realtime stored", async () => {
    useSTTConnectionMock.mockReturnValue({
      conn: {
        provider: "fmtr",
        model: "soniqo-qwen3-small",
        baseUrl: "http://localhost:8080",
        apiKey: "",
      },
    });

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(startMock.mock.calls[0]?.[0]).toMatchObject({
      transcription_mode: "batch",
    });
  });

  test("uses live transcription for realtime local models", async () => {
    useSTTConnectionMock.mockReturnValue({
      conn: {
        provider: "fmtr",
        model: "soniqo-parakeet-streaming",
        baseUrl: "http://localhost:8080",
        apiKey: "",
      },
    });

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(startMock.mock.calls[0]?.[0]).toMatchObject({
      transcription_mode: "live",
    });
  });

  test.each([
    { summaryLanguage: "en", meetingLanguage: "nl", mode: "batch" },
    { summaryLanguage: "nl", meetingLanguage: "en", mode: "live" },
  ])(
    "uses $meetingLanguage for capture independently of $summaryLanguage summary output",
    async ({ summaryLanguage, meetingLanguage, mode }) => {
      useConfigValueMock.mockImplementation((key) => {
        if (key === "ai_language") return summaryLanguage;
        if (key === "meeting_languages") return [meetingLanguage];
        if (key === "spoken_languages") return ["fr"];
        return [];
      });
      useSTTConnectionMock.mockReturnValue({
        conn: {
          provider: "fmtr",
          model: "soniqo-parakeet-streaming",
          baseUrl: "http://localhost:8080",
          apiKey: "",
        },
      });
      const { result } = renderHook(() => useStartListening("session-1"));
      await act(async () => {
        await result.current();
      });
      expect(startMock.mock.calls[0]?.[0]).toMatchObject({
        languages: [meetingLanguage],
        transcription_mode: mode,
      });
    },
  );

  test("demotes non-English main language to batch with the full language list", async () => {
    useConfigValueMock.mockImplementation((key) =>
      key === "ai_language"
        ? "de"
        : key === "meeting_languages"
          ? undefined
          : ["en"],
    );
    useSTTConnectionMock.mockReturnValue({
      conn: {
        provider: "fmtr",
        model: "soniqo-parakeet-streaming",
        baseUrl: "http://localhost:8080",
        apiKey: "",
      },
    });

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(startMock.mock.calls[0]?.[0]).toMatchObject({
      languages: ["de", "en"],
      transcription_mode: "batch",
    });
  });

  test("demotes to batch instead of filtering unsupported extra spoken languages", async () => {
    useConfigValueMock.mockImplementation((key) =>
      key === "ai_language"
        ? "en"
        : key === "meeting_languages"
          ? undefined
          : ["ko"],
    );
    useSTTConnectionMock.mockReturnValue({
      conn: {
        provider: "fmtr",
        model: "soniqo-parakeet-streaming",
        baseUrl: "http://localhost:8080",
        apiKey: "",
      },
    });

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(startMock.mock.calls[0]?.[0]).toMatchObject({
      languages: ["en", "ko"],
      transcription_mode: "batch",
    });
  });

  test("uses the main language for Deepgram live capture when extras are unsupported", async () => {
    useConfigValueMock.mockImplementation((key) =>
      key === "ai_language"
        ? "en"
        : key === "meeting_languages"
          ? undefined
          : ["ko"],
    );
    useSTTConnectionMock.mockReturnValue({
      conn: {
        provider: "deepgram",
        model: "nova-3-general",
        baseUrl: "https://api.deepgram.com/v1/listen",
        apiKey: "test-key",
      },
    });
    isSupportedLanguagesLiveMock.mockImplementation(
      (_provider, _model, languages) =>
        Promise.resolve({
          status: "ok",
          data: languages.length === 1 && languages[0] === "en",
        }),
    );

    const { result } = renderHook(() => useStartListening("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(startMock.mock.calls[0]?.[0]).toMatchObject({
      languages: ["en"],
      transcription_mode: undefined,
    });
  });
});

import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  Transcript as ScopedTranscript,
  TranscriptContent as Transcript,
} from "./index";
import { renderedTranscriptPrefix } from "./render-cache";
import { createRenderCacheHost } from "./render-cache.test-support";

import type { TranscriptRecord } from "~/stt/queries";

const {
  useListenerMock,
  useAudioPlayerMock,
  regenerateTranscriptMock,
  transcriptQueryMock,
} = vi.hoisted(() => ({
  useListenerMock: vi.fn(),
  transcriptQueryMock: vi.fn(),
  useAudioPlayerMock: vi.fn(),
  regenerateTranscriptMock: vi.fn(),
}));

vi.mock("~/stt/queries", () => ({
  useSessionTranscriptsQuery: transcriptQueryMock,
}));

vi.mock("./actions", () => ({
  useRegenerateTranscript: () => regenerateTranscriptMock,
}));

vi.mock("~/stt/contexts", () => ({
  useListener: useListenerMock,
}));

vi.mock("~/audio-player", () => ({
  useAudioPlayer: useAudioPlayerMock,
}));

vi.mock("./screens/batch", () => ({
  BatchState: () => <div data-testid="batch-state" />,
}));

vi.mock("./screens/empty", () => ({
  TranscriptEmptyState: ({
    error,
    onRetranscribe,
  }: {
    error?: string | null;
    onRetranscribe?: () => void;
  }) => (
    <div data-testid="empty-state">
      {error && <div role="alert">{error}</div>}
      {onRetranscribe && (
        <button onClick={onRetranscribe}>Re-transcribe</button>
      )}
    </div>
  ),
}));

vi.mock("./screens/listening", () => ({
  TranscriptListeningState: ({ status }: { status: string }) => (
    <div data-testid="listening-state">{status}</div>
  ),
}));

vi.mock("./renderer", () => ({
  TranscriptViewer: () => <div data-testid="transcript-viewer" />,
}));

vi.mock("~/stt/useUploadFile", () => ({
  useUploadFile: vi.fn(() => ({
    uploadAudio: vi.fn(),
    uploadTranscript: vi.fn(),
    processFile: vi.fn(),
  })),
}));

vi.mock("~/stt/pending-upload", () => ({
  consumePendingUpload: vi.fn(() => null),
}));

describe("Transcript", () => {
  const sessionId = "session-1";

  let listenerState: {
    getSessionMode: (id: string) => "inactive" | "active" | "finalizing";
    batch: Record<string, { error?: string | null }>;
    live: {
      degraded: null;
      requestedLiveTranscription: boolean;
      liveTranscriptionActive: boolean;
    };
    liveSegments: unknown[];
    partialWordsByChannel: Record<number, unknown[]>;
    partialHintsByChannel: Record<number, unknown[]>;
  };
  let transcripts: TranscriptRecord[];
  let animationFrames: FrameRequestCallback[];
  let cacheHosts: ReturnType<typeof createRenderCacheHost>[];

  afterEach(() => {
    cleanup();
    for (const host of cacheHosts) host.dispose();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  beforeEach(() => {
    cacheHosts = [];
    regenerateTranscriptMock.mockClear();
    animationFrames = [];
    vi.stubGlobal(
      "requestAnimationFrame",
      vi.fn((callback: FrameRequestCallback) => {
        animationFrames.push(callback);
        return animationFrames.length;
      }),
    );
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
    transcripts = [makeTranscript([])];

    listenerState = {
      getSessionMode: () => "active",
      batch: {},
      live: {
        degraded: null,
        requestedLiveTranscription: true,
        liveTranscriptionActive: true,
      },
      liveSegments: [],
      partialWordsByChannel: {},
      partialHintsByChannel: {},
    };

    useListenerMock.mockImplementation((selector) => selector(listenerState));
    useAudioPlayerMock.mockReturnValue({ audioExists: false });
  });

  it.each([
    "global",
    "other-session",
    "other-transcript",
    "same-scope-replacement",
    "owned",
  ])(
    "uses only the captured owning render cache for warm readiness: %s",
    (location) => {
      const host = createRenderCacheHost("same-scope");
      const other = createRenderCacheHost("same-scope");
      cacheHosts.push(host, other);
      const row = makeTranscript([
        { id: "w", text: "hello", start_ms: 0, end_ms: 100, channel: 0 },
      ]);
      transcriptQueryMock.mockReturnValue({
        data: [row],
        isPending: false,
        isError: false,
        refetch: vi.fn(),
      });
      const cache =
        location === "global"
          ? host.rootClient
          : location === "same-scope-replacement"
            ? other.scope.queryClient
            : host.scope.queryClient;
      const key = [
        ...renderedTranscriptPrefix(
          host.scope,
          location === "other-session" ? "other" : sessionId,
          location === "other-transcript" ? "other" : row.id,
        ),
        "request-hash",
      ];
      cache.setQueryData(key, []);
      render(
        <host.Host>
          <ScopedTranscript sessionId={sessionId} scrollRef={createRef()} />
        </host.Host>,
      );
      expect(screen.queryByTestId("transcript-viewer") !== null).toBe(
        location === "owned",
      );
      expect(screen.queryByText("Loading transcript...") !== null).toBe(
        location !== "owned",
      );
    },
  );

  it("does not mistake delayed loading or a failed read for an empty transcript", () => {
    listenerState.getSessionMode = () => "inactive";
    const retry = vi.fn();
    const scrollRef = createRef<HTMLDivElement>();
    const view = render(
      <Transcript
        sessionId={sessionId}
        transcripts={[]}
        scrollRef={scrollRef}
        pending
        retry={retry}
      />,
    );
    expect(screen.getByText("Loading transcript...")).toBeTruthy();
    expect(screen.queryByTestId("empty-state")).toBeNull();
    view.rerender(
      <Transcript
        sessionId={sessionId}
        transcripts={[]}
        scrollRef={scrollRef}
        error
        retry={retry}
      />,
    );
    expect(screen.queryByTestId("empty-state")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(retry).toHaveBeenCalledOnce();
    view.rerender(
      <Transcript
        sessionId={sessionId}
        transcripts={[]}
        scrollRef={scrollRef}
      />,
    );
    expect(screen.getByTestId("empty-state")).toBeTruthy();
  });

  it("shows live words while stored history is loading", () => {
    listenerState.liveSegments = [{ id: "live", words: [{ text: "now" }] }];
    render(
      <Transcript
        sessionId={sessionId}
        transcripts={[]}
        scrollRef={createRef<HTMLDivElement>()}
        pending
      />,
    );
    flushAnimationFrame(animationFrames);
    flushAnimationFrame(animationFrames);
    expect(screen.getByTestId("transcript-viewer")).toBeTruthy();
  });

  it("shows the failure and retry action when a batch fails before producing words", () => {
    listenerState.getSessionMode = () => "inactive";
    listenerState.batch[sessionId] = {
      error: "Transcription timed out after 60 seconds without progress.",
    };
    useAudioPlayerMock.mockReturnValue({ audioExists: true });

    render(
      <Transcript
        sessionId={sessionId}
        transcripts={[]}
        scrollRef={createRef<HTMLDivElement>()}
      />,
    );

    expect(screen.getByRole("alert").textContent).toBe(
      "Transcription timed out after 60 seconds without progress.",
    );
    expect(screen.queryByTestId("transcript-viewer")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Re-transcribe" }));
    expect(regenerateTranscriptMock).toHaveBeenCalledOnce();
  });

  it("keeps transcript words visible after a batch failure", () => {
    listenerState.getSessionMode = () => "inactive";
    listenerState.batch[sessionId] = { error: "Transcription failed." };

    render(
      <Transcript
        sessionId={sessionId}
        transcripts={[
          makeTranscript([
            { id: "w", text: "hello", start_ms: 0, end_ms: 10, channel: 0 },
          ]),
        ]}
        scrollRef={createRef<HTMLDivElement>()}
        initiallyReady
      />,
    );

    expect(screen.getByTestId("transcript-viewer")).toBeTruthy();
    expect(screen.queryByTestId("empty-state")).toBeNull();
  });

  it("does not add a loading delay when rendered data is cached", () => {
    render(
      <Transcript
        sessionId={sessionId}
        transcripts={[
          makeTranscript([
            { id: "w", text: "hello", start_ms: 0, end_ms: 10, channel: 0 },
          ]),
        ]}
        scrollRef={createRef<HTMLDivElement>()}
        initiallyReady
      />,
    );
    expect(screen.queryByText("Loading transcript...")).toBeNull();
    expect(screen.getByTestId("transcript-viewer")).toBeTruthy();
  });

  it("switches to transcript viewer after transcript words persist", () => {
    const scrollRef = createRef<HTMLDivElement>();
    const view = render(
      <Transcript
        sessionId={sessionId}
        transcripts={transcripts}
        scrollRef={scrollRef}
      />,
    );

    expect(screen.getByTestId("listening-state").textContent).toBe("listening");

    transcripts = [
      makeTranscript([
        { id: "word-1", text: " Hello", start_ms: 0, end_ms: 1, channel: 0 },
      ]),
    ];

    view.rerender(
      <Transcript
        sessionId={sessionId}
        transcripts={transcripts}
        scrollRef={scrollRef}
      />,
    );

    expect(screen.getByText("Loading transcript...")).not.toBeNull();
    expect(screen.queryByTestId("transcript-viewer")).toBeNull();

    flushAnimationFrame(animationFrames);
    expect(screen.queryByTestId("transcript-viewer")).toBeNull();

    flushAnimationFrame(animationFrames);
    expect(screen.getByTestId("transcript-viewer")).not.toBeNull();
  });

  it("finishes loading when WebKit does not deliver animation frames", async () => {
    vi.useFakeTimers();
    transcripts = [
      makeTranscript([
        { id: "word-1", text: "Hello", start_ms: 0, end_ms: 1, channel: 0 },
      ]),
    ];
    render(
      <Transcript
        sessionId={sessionId}
        transcripts={transcripts}
        scrollRef={createRef()}
      />,
    );
    expect(screen.getByText("Loading transcript...")).not.toBeNull();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(screen.getByTestId("transcript-viewer")).not.toBeNull();
  });

  it("keeps existing transcript content unobstructed while finalizing", () => {
    listenerState = {
      ...listenerState,
      getSessionMode: () => "finalizing",
    };
    transcripts = [
      makeTranscript([
        { id: "word-1", text: " Hello", start_ms: 0, end_ms: 1, channel: 0 },
      ]),
    ];

    render(
      <Transcript
        sessionId={sessionId}
        transcripts={transcripts}
        scrollRef={createRef()}
      />,
    );

    expect(screen.queryByText("Finalizing transcript...")).toBeNull();
    flushAnimationFrame(animationFrames);
    flushAnimationFrame(animationFrames);
    expect(screen.getByTestId("transcript-viewer")).not.toBeNull();
  });

  it("shows recording state for record-only capture sessions", () => {
    listenerState = {
      ...listenerState,
      live: {
        ...listenerState.live,
        requestedLiveTranscription: false,
        liveTranscriptionActive: false,
      },
    };

    render(
      <Transcript
        sessionId={sessionId}
        transcripts={transcripts}
        scrollRef={createRef()}
      />,
    );

    expect(screen.queryByTestId("listening-state")).toBeNull();
    expect(screen.getByTestId("batch-state")).not.toBeNull();
  });
});

function makeTranscript(words: TranscriptRecord["words"]): TranscriptRecord {
  return {
    id: "transcript-1",
    ownerUserId: "self",
    sessionId: "session-1",
    startedAt: 0,
    words,
    speakerHints: [],
  };
}

function flushAnimationFrame(animationFrames: FrameRequestCallback[]) {
  const callbacks = animationFrames.splice(0);
  act(() => {
    for (const callback of callbacks) {
      callback(performance.now());
    }
  });
}

import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TranscriptContent as Transcript } from "./index";

import type { TranscriptRecord } from "~/stt/queries";

const { useListenerMock, useAudioPlayerMock, regenerateTranscriptMock } =
  vi.hoisted(() => ({
    useListenerMock: vi.fn(),
    useAudioPlayerMock: vi.fn(),
    regenerateTranscriptMock: vi.fn(),
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
  TranscriptEmptyState: () => <div data-testid="empty-state" />,
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

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  beforeEach(() => {
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

import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  audioPath: vi.fn(),
  confirmRegenerateSpeakerReset: vi.fn(),
  handleBatchFailed: vi.fn(),
  queueAutoEnhanceIfSummaryEmpty: vi.fn(),
  runBatch: vi.fn(),
  sessionTranscripts: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@hypr/plugin-fs-sync", () => ({
  commands: { audioPath: mocks.audioPath },
}));

vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: mocks.toastError },
}));

vi.mock("~/services/enhancer", () => ({
  getEnhancerService: () => ({
    queueAutoEnhanceIfSummaryEmpty: mocks.queueAutoEnhanceIfSummaryEmpty,
  }),
}));

vi.mock("~/stt/contexts", () => ({
  useListener: (selector: (state: unknown) => unknown) =>
    selector({ handleBatchFailed: mocks.handleBatchFailed }),
}));

vi.mock("~/stt/useRunBatch", () => ({
  isStoppedTranscriptionError: (error: unknown) =>
    error instanceof Error && error.message === "Transcription stopped.",
  useRunBatch: () => mocks.runBatch,
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: { sessionTranscripts: mocks.sessionTranscripts },
}));

vi.mock("./regenerate-confirm", () => ({
  confirmRegenerateSpeakerReset: mocks.confirmRegenerateSpeakerReset,
}));

import { useRegenerateTranscript } from "./actions";

function transcriptWithSpeakerLabels(...labels: string[]) {
  return {
    id: "transcript-1",
    session_id: "session-1",
    speaker_hints: labels.map((value, index) => ({
      id: `hint-${index}`,
      word_id: `word-${index}`,
      type: "speaker_label",
      value,
    })),
  };
}

describe("useRegenerateTranscript", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.audioPath.mockResolvedValue({
      status: "ok",
      data: "/tmp/session.wav",
    });
    mocks.sessionTranscripts.mockResolvedValue({ status: "ok", data: [] });
    mocks.runBatch.mockResolvedValue(undefined);
  });

  it.each(["audio", "speakers", "confirmation"])(
    "refuses regeneration after a delayed %s read loses its owner",
    async (boundary) => {
      let allowed = true;
      let finish!: () => void;
      const blocker = new Promise<void>((done) => {
        finish = done;
      });
      const rows = {
        status: "ok",
        data: [transcriptWithSpeakerLabels("Alice")],
      };
      mocks.sessionTranscripts.mockResolvedValue(rows);
      if (boundary === "audio")
        mocks.audioPath.mockImplementationOnce(async () => {
          await blocker;
          return { status: "ok", data: "/tmp/file.wav" };
        });
      if (boundary === "speakers")
        mocks.sessionTranscripts.mockImplementationOnce(async () => {
          await blocker;
          return rows;
        });
      if (boundary === "confirmation")
        mocks.confirmRegenerateSpeakerReset.mockImplementationOnce(async () => {
          await blocker;
          return true;
        });
      const { result } = renderHook(() =>
        useRegenerateTranscript("session-1", () => {
          if (!allowed) throw new Error("retired");
        }),
      );
      const pending = result.current();
      const rejected = expect(pending).rejects.toThrow("retired");
      await waitFor(() =>
        expect(
          boundary === "audio"
            ? mocks.audioPath
            : boundary === "speakers"
              ? mocks.sessionTranscripts
              : mocks.confirmRegenerateSpeakerReset,
        ).toHaveBeenCalledOnce(),
      );
      allowed = false;
      await act(async () => {
        finish();
        await rejected;
      });
      expect(mocks.runBatch).not.toHaveBeenCalled();
      expect(mocks.queueAutoEnhanceIfSummaryEmpty).not.toHaveBeenCalled();
    },
  );

  it("shows batch transcription failures even when an old transcript exists", async () => {
    mocks.runBatch.mockRejectedValue(new Error("Authentication failed"));
    const { result } = renderHook(() => useRegenerateTranscript("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(mocks.handleBatchFailed).toHaveBeenCalledWith(
      "session-1",
      "Authentication failed",
    );
    expect(mocks.toastError).toHaveBeenCalledWith("Re-transcription failed", {
      id: "transcript-regenerate-failed-session-1",
      description: "Authentication failed",
    });
  });

  it("regenerates without confirmation when no speakers are assigned", async () => {
    const { result } = renderHook(() => useRegenerateTranscript("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(mocks.confirmRegenerateSpeakerReset).not.toHaveBeenCalled();
    expect(mocks.runBatch).toHaveBeenCalledWith("/tmp/session.wav");
  });

  it("asks for confirmation when speaker names are assigned and aborts on cancel", async () => {
    mocks.sessionTranscripts.mockResolvedValue({
      status: "ok",
      data: [transcriptWithSpeakerLabels("Alice", "Bob")],
    });
    mocks.confirmRegenerateSpeakerReset.mockResolvedValue(false);
    const { result } = renderHook(() => useRegenerateTranscript("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(mocks.confirmRegenerateSpeakerReset).toHaveBeenCalledWith(2);
    expect(mocks.runBatch).not.toHaveBeenCalled();
  });

  it("regenerates after the speaker reset is confirmed", async () => {
    mocks.sessionTranscripts.mockResolvedValue({
      status: "ok",
      data: [transcriptWithSpeakerLabels("Alice")],
    });
    mocks.confirmRegenerateSpeakerReset.mockResolvedValue(true);
    const { result } = renderHook(() => useRegenerateTranscript("session-1"));

    await act(async () => {
      await result.current();
    });

    expect(mocks.confirmRegenerateSpeakerReset).toHaveBeenCalledWith(1);
    expect(mocks.runBatch).toHaveBeenCalledWith("/tmp/session.wav");
  });
});

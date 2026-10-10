import { describe, expect, it } from "vitest";

import {
  type GeneralState,
  initialGeneralState,
  noteLiveTranscriptActivity,
  TRANSCRIPTION_FINAL_STALL_AUDIBLE_SECONDS,
  TRANSCRIPTION_STALL_AUDIBLE_SECONDS,
  tickTranscriptionStallWatchdog,
  updateLiveProgress,
} from "./general-shared";

function createActiveLive(): GeneralState["live"] {
  return {
    ...initialGeneralState.live,
    status: "active",
    sessionId: "session-1",
    requestedLiveTranscription: true,
    liveTranscriptionActive: true,
    amplitude: { mic: 0.4, speaker: 0.4 },
    finalizingBySession: {},
    eventUnlistenersBySession: {},
  };
}

it("keeps recording through device recovery and clears its warning on recovery", () => {
  const live = createActiveLive();
  updateLiveProgress(live, {
    type: "audio_error",
    session_id: "session-1",
    error: "Microphone disconnected. Reconnecting.",
    device: null,
    is_fatal: false,
  });
  expect(live.status).toBe("active");
  expect(live.lastError).toContain("Reconnecting");
  updateLiveProgress(live, {
    type: "audio_error",
    session_id: "session-1",
    error: "",
    device: null,
    is_fatal: false,
  });
  expect(live.lastError).toBeNull();
  expect(live.status).toBe("active");
});

describe("tickTranscriptionStallWatchdog", () => {
  it("flags a stalled live transcription after sustained audible silence", () => {
    const live = createActiveLive();

    let stalledAt: number | null = null;
    for (
      let second = 1;
      second <= TRANSCRIPTION_STALL_AUDIBLE_SECONDS + 5;
      second += 1
    ) {
      if (tickTranscriptionStallWatchdog(live)) {
        stalledAt = second;
        break;
      }
    }

    expect(stalledAt).toBe(TRANSCRIPTION_STALL_AUDIBLE_SECONDS);
    expect(live.transcriptionStalled).toBe(true);
    expect(live.needsBatchRepair).toBe(true);
  });

  it("only counts seconds with audible audio", () => {
    const live = createActiveLive();
    live.amplitude = { mic: 0, speaker: 0 };

    for (
      let second = 0;
      second < TRANSCRIPTION_STALL_AUDIBLE_SECONDS * 2;
      second += 1
    ) {
      expect(tickTranscriptionStallWatchdog(live)).toBe(false);
    }

    expect(live.transcriptionStalled).toBe(false);
    expect(live.needsBatchRepair).toBe(false);
    expect(live.stallAudibleSeconds).toBe(0);
  });

  it("resets the stall counter when transcript activity arrives", () => {
    const live = createActiveLive();

    for (
      let second = 0;
      second < TRANSCRIPTION_STALL_AUDIBLE_SECONDS - 1;
      second += 1
    ) {
      tickTranscriptionStallWatchdog(live);
    }
    expect(live.stallAudibleSeconds).toBe(
      TRANSCRIPTION_STALL_AUDIBLE_SECONDS - 1,
    );

    noteLiveTranscriptActivity(live, { hasFinalWords: true });
    expect(live.stallAudibleSeconds).toBe(0);
    expect(live.finalStallAudibleSeconds).toBe(0);

    expect(tickTranscriptionStallWatchdog(live)).toBe(false);
    expect(live.transcriptionStalled).toBe(false);
  });

  it("flags a stall when partials keep flowing but nothing finalizes", () => {
    const live = createActiveLive();

    let stalledAt: number | null = null;
    for (
      let second = 1;
      second <= TRANSCRIPTION_FINAL_STALL_AUDIBLE_SECONDS + 5;
      second += 1
    ) {
      if (tickTranscriptionStallWatchdog(live)) {
        stalledAt = second;
        break;
      }
      noteLiveTranscriptActivity(live, { hasFinalWords: false });
    }

    expect(stalledAt).toBe(TRANSCRIPTION_FINAL_STALL_AUDIBLE_SECONDS);
    expect(live.transcriptionStalled).toBe(true);
    expect(live.needsBatchRepair).toBe(true);
  });

  it("keeps the stalled flag until finalized words arrive", () => {
    const live = createActiveLive();
    live.transcriptionStalled = true;
    live.needsBatchRepair = true;

    noteLiveTranscriptActivity(live, { hasFinalWords: false });
    expect(live.transcriptionStalled).toBe(true);

    noteLiveTranscriptActivity(live, { hasFinalWords: true });
    expect(live.transcriptionStalled).toBe(false);
    expect(live.needsBatchRepair).toBe(true);
  });

  it("stays quiet for record-only sessions and repeated stalls", () => {
    const recordOnly = createActiveLive();
    recordOnly.requestedLiveTranscription = false;
    recordOnly.liveTranscriptionActive = false;
    expect(tickTranscriptionStallWatchdog(recordOnly)).toBe(false);

    const stalled = createActiveLive();
    stalled.transcriptionStalled = true;
    stalled.needsBatchRepair = true;
    expect(tickTranscriptionStallWatchdog(stalled)).toBe(false);
  });

  it("keeps watching audible speaker audio while the mic is silent", () => {
    const live = createActiveLive();
    live.amplitude = { mic: 0, speaker: 1 };

    tickTranscriptionStallWatchdog(live);
    expect(live.stallAudibleSeconds).toBe(1);
  });

  it("keeps the batch repair flag after transcript activity resumes", () => {
    const live = createActiveLive();

    for (
      let second = 0;
      second < TRANSCRIPTION_STALL_AUDIBLE_SECONDS;
      second += 1
    ) {
      tickTranscriptionStallWatchdog(live);
    }
    expect(live.needsBatchRepair).toBe(true);

    noteLiveTranscriptActivity(live, { hasFinalWords: true });
    expect(live.transcriptionStalled).toBe(false);
    expect(live.needsBatchRepair).toBe(true);
  });
});

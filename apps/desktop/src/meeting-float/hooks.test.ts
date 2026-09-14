import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  sessionListHeaders: vi.fn(),
  subscribeIndexChanged: vi.fn(),
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionListHeaders: mocks.sessionListHeaders,
  },
}));

vi.mock("~/shared/index-query", () => ({
  subscribeIndexChanged: mocks.subscribeIndexChanged,
}));

import {
  createMeetingFloatLabelContext,
  loadMeetingFloatData,
  subscribeMeetingFloatData,
} from "./hooks";

import { DEFAULT_USER_ID } from "~/shared/utils";

const entries = [
  {
    id: "session-1",
    title: "Planning",
    has_transcript_words: false,
  },
] as const;

describe("meeting float index data", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.sessionListHeaders.mockResolvedValue({ status: "ok", data: entries });
    mocks.subscribeIndexChanged.mockReturnValue(() => {});
  });

  it("loads titles and speaker identity from one canonical snapshot", async () => {
    const data = await loadMeetingFloatData();
    const labels = createMeetingFloatLabelContext(data, "session-1");

    expect(data.sessions["session-1"]).toEqual({
      title: "Planning",
      ownerUserId: DEFAULT_USER_ID,
    });
    expect(labels.getSelfHumanId()).toBe(DEFAULT_USER_ID);
    expect(labels.getParticipantHumanIds?.()).toEqual([]);
    expect(labels.getHumanName("Remote speaker")).toBe("Remote speaker");
  });

  it("pushes an initial snapshot and refreshes on index changes", async () => {
    let onIndexChange: (() => void) | undefined;
    const unsubscribe = vi.fn();
    mocks.subscribeIndexChanged.mockImplementation(
      (_entity: unknown, onChange: () => void) => {
        onIndexChange = onChange;
        return unsubscribe;
      },
    );
    const onData = vi.fn();

    const stop = await subscribeMeetingFloatData(onData, vi.fn());

    expect(mocks.subscribeIndexChanged).toHaveBeenCalledWith(
      "session_headers",
      expect.any(Function),
    );
    expect(onData).toHaveBeenCalledWith(
      expect.objectContaining({
        sessions: expect.objectContaining({
          "session-1": expect.objectContaining({ title: "Planning" }),
        }),
      }),
    );

    mocks.sessionListHeaders.mockResolvedValue({
      status: "ok",
      data: [
        {
          id: "session-2",
          title: "Retro",
          has_transcript_words: false,
        },
      ],
    });
    onIndexChange?.();
    await vi.waitFor(() =>
      expect(onData).toHaveBeenCalledWith(
        expect.objectContaining({
          sessions: expect.objectContaining({
            "session-2": expect.objectContaining({ title: "Retro" }),
          }),
        }),
      ),
    );

    await stop();
    expect(unsubscribe).toHaveBeenCalledTimes(1);
  });

  it("reports load failures through onError", async () => {
    mocks.sessionListHeaders.mockResolvedValue({
      status: "error",
      error: "index unavailable",
    });
    const onData = vi.fn();
    const onError = vi.fn();

    await subscribeMeetingFloatData(onData, onError);

    expect(onData).not.toHaveBeenCalled();
    expect(onError).toHaveBeenCalledWith("index unavailable");
  });
});

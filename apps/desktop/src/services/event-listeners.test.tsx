import { render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { EventListeners } from "./event-listeners";

import { useAudioImport } from "~/store/zustand/audio-import";
import { AUDIO_IMPORT_COMPLETED_NOTIFICATION_KEY } from "~/stt/audio-import-notification";
import { createAutoStopEndedNotificationKey } from "~/stt/auto-stop-notification";
import { createBatchCompletedNotificationKey } from "~/stt/batch-completed-notification";

const {
  notificationListenMock,
  updaterListenMock,
  maybeEmitUpdatedMock,
  getCurrentWebviewWindowLabelMock,
  sessionIdsMock,
  subscribeIndexChangedMock,
  listenerSubscribeMock,
  useConfigValueMock,
  useConfigValuesMock,
  setSettingValueMock,
  openNewMock,
  createSessionMock,
  setTriggerAppIdsMock,
  stopMock,
  updateCaptureConfigMock,
  getListenerStateMock,
} = vi.hoisted(() => ({
  notificationListenMock: vi.fn(),
  updaterListenMock: vi.fn(),
  maybeEmitUpdatedMock: vi.fn(),
  getCurrentWebviewWindowLabelMock: vi.fn(() => "main"),
  sessionIdsMock: vi.fn(),
  subscribeIndexChangedMock: vi.fn(),
  listenerSubscribeMock: vi.fn(),
  useConfigValueMock: vi.fn((): string[] => []),
  useConfigValuesMock: vi.fn(),
  setSettingValueMock: vi.fn(async () => {}),
  openNewMock: vi.fn(),
  createSessionMock: vi.fn(async () => "session-new"),
  setTriggerAppIdsMock: vi.fn(),
  stopMock: vi.fn(),
  updateCaptureConfigMock: vi.fn(),
  getListenerStateMock: vi.fn(),
}));

vi.mock("@hypr/plugin-notification", () => ({
  events: {
    notificationEvent: {
      listen: notificationListenMock,
    },
  },
}));

vi.mock("@hypr/plugin-updater2", () => ({
  commands: {
    maybeEmitUpdated: maybeEmitUpdatedMock,
  },
  events: {
    updatedEvent: {
      listen: updaterListenMock,
    },
  },
}));

vi.mock("@hypr/plugin-windows", () => ({
  getCurrentWebviewWindowLabel: getCurrentWebviewWindowLabelMock,
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionIds: sessionIdsMock,
  },
}));

vi.mock("~/shared/index-query", () => ({
  subscribeIndexChanged: subscribeIndexChangedMock,
}));

vi.mock("~/shared/config", () => ({
  useConfigValue: useConfigValueMock,
  useConfigValues: useConfigValuesMock,
}));

vi.mock("~/settings/queries", () => ({
  setSettingValue: setSettingValueMock,
}));

vi.mock("~/session/queries", () => ({
  createSession: createSessionMock,
}));

vi.mock("~/store/zustand/tabs", () => ({
  useTabs: (selector: (state: { openNew: typeof openNewMock }) => unknown) =>
    selector({ openNew: openNewMock }),
}));

vi.mock("~/store/zustand/listener/instance", () => ({
  listenerStore: {
    getState: getListenerStateMock,
    subscribe: listenerSubscribeMock,
  },
}));

describe("EventListeners notification events", () => {
  beforeEach(() => {
    notificationListenMock.mockReset();
    updaterListenMock.mockReset();
    maybeEmitUpdatedMock.mockReset();
    getCurrentWebviewWindowLabelMock.mockReset();
    sessionIdsMock.mockReset();
    subscribeIndexChangedMock.mockReset();
    listenerSubscribeMock.mockReset();
    useConfigValueMock.mockReset();
    useConfigValuesMock.mockReset();
    setSettingValueMock.mockReset();
    openNewMock.mockReset();
    createSessionMock.mockReset();
    setTriggerAppIdsMock.mockReset();
    stopMock.mockReset();
    updateCaptureConfigMock.mockReset();
    getListenerStateMock.mockReset();

    getCurrentWebviewWindowLabelMock.mockReturnValue("main");
    notificationListenMock.mockResolvedValue(() => {});
    updaterListenMock.mockResolvedValue(() => {});
    createSessionMock.mockResolvedValue("session-new");
    sessionIdsMock.mockResolvedValue({ status: "ok", data: [] });
    subscribeIndexChangedMock.mockReturnValue(() => {});
    listenerSubscribeMock.mockReturnValue(() => {});
    useConfigValueMock.mockReturnValue([]);
    useConfigValuesMock.mockReturnValue({
      ai_language: "en",
      spoken_languages: [],
      current_stt_provider: undefined,
      current_stt_model: undefined,
    });
    setSettingValueMock.mockResolvedValue(undefined);
    getListenerStateMock.mockReturnValue({
      setTriggerAppIds: setTriggerAppIdsMock,
      stop: stopMock,
      updateCaptureConfig: updateCaptureConfigMock,
      live: { status: "active", sessionId: "session-1" },
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  test("stores mic-detected footer actions as ignored platforms", async () => {
    useConfigValueMock.mockReturnValue(["com.existing.app"]);

    render(<EventListeners />);

    await vi.waitFor(() =>
      expect(notificationListenMock).toHaveBeenCalledTimes(1),
    );

    const handler = notificationListenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "notification_footer_action",
        key: "mic-1",
        source: {
          type: "mic_detected",
          app_names: ["Zoom"],
          app_ids: ["us.zoom.xos", "com.existing.app"],
        },
      },
    });

    expect(setSettingValueMock).toHaveBeenCalledWith(
      "ignored_platforms",
      JSON.stringify(["com.existing.app", "us.zoom.xos"]),
    );
    expect(openNewMock).not.toHaveBeenCalled();
  });

  test("notification_accept with auto-stop prompt stops the active session", async () => {
    render(<EventListeners />);

    await vi.waitFor(() =>
      expect(notificationListenMock).toHaveBeenCalledTimes(1),
    );

    const handler = notificationListenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "notification_accept",
        key: createAutoStopEndedNotificationKey("session-1"),
        source: null,
      },
    });

    expect(stopMock).toHaveBeenCalledTimes(1);
    expect(createSessionMock).not.toHaveBeenCalled();
    expect(openNewMock).not.toHaveBeenCalled();
  });

  test("live capture config sync defaults to English independently of legacy language settings", async () => {
    vi.useFakeTimers();
    useConfigValuesMock.mockReturnValue({
      ai_language: "ko",
      spoken_languages: ["ko"],
      current_stt_provider: "soniox",
      current_stt_model: "stt-v4",
    });
    sessionIdsMock.mockResolvedValue({ status: "ok", data: ["session-1"] });

    render(<EventListeners />);

    await vi.waitFor(() => expect(sessionIdsMock).toHaveBeenCalledTimes(1));
    expect(subscribeIndexChangedMock).toHaveBeenCalledWith(
      "session_headers",
      expect.any(Function),
    );
    const listener = listenerSubscribeMock.mock.calls[0]![0];
    const state = getListenerStateMock();
    for (let tick = 0; tick < 10; tick++) {
      listener(
        { ...state, live: { ...state.live, amplitude: tick, seconds: tick } },
        state,
      );
      await vi.advanceTimersByTimeAsync(100);
    }

    expect(updateCaptureConfigMock).toHaveBeenCalledWith({
      session_id: "session-1",
      languages: ["en"],
      participant_human_ids: [],
      // The owner concept died (D10): an indexed session maps to the default user.
      self_human_id: "00000000-0000-0000-0000-000000000000",
    });
  });

  test("notification_confirm with auto-stop prompt ignores collapsed body click", async () => {
    render(<EventListeners />);

    await vi.waitFor(() =>
      expect(notificationListenMock).toHaveBeenCalledTimes(1),
    );

    const handler = notificationListenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "notification_confirm",
        key: createAutoStopEndedNotificationKey("session-1"),
        source: null,
      },
    });

    expect(stopMock).not.toHaveBeenCalled();
    expect(createSessionMock).not.toHaveBeenCalled();
    expect(openNewMock).not.toHaveBeenCalled();
  });

  test("notification_confirm with session source opens that session", async () => {
    render(<EventListeners />);

    await vi.waitFor(() =>
      expect(notificationListenMock).toHaveBeenCalledTimes(1),
    );

    const handler = notificationListenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "notification_confirm",
        key: "batch-completed-session-1",
        source: { type: "session", session_id: "session-1" },
      },
    });

    expect(createSessionMock).not.toHaveBeenCalled();
    expect(openNewMock).toHaveBeenCalledWith({
      type: "sessions",
      id: "session-1",
      state: { view: null, autoStart: null },
    });
  });

  test("notification_confirm with batch key opens that session without source", async () => {
    render(<EventListeners />);

    await vi.waitFor(() =>
      expect(notificationListenMock).toHaveBeenCalledTimes(1),
    );

    const handler = notificationListenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "notification_confirm",
        key: createBatchCompletedNotificationKey("session-1"),
        source: null,
      },
    });

    expect(createSessionMock).not.toHaveBeenCalled();
    expect(openNewMock).toHaveBeenCalledWith({
      type: "sessions",
      id: "session-1",
      state: { view: null, autoStart: null },
    });
  });

  test("notification_confirm with audio-import key opens the import dialog", async () => {
    useAudioImport.getState().setDialogOpen(false);

    render(<EventListeners />);

    await vi.waitFor(() =>
      expect(notificationListenMock).toHaveBeenCalledTimes(1),
    );

    const handler = notificationListenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "notification_confirm",
        key: AUDIO_IMPORT_COMPLETED_NOTIFICATION_KEY,
        source: null,
      },
    });

    expect(useAudioImport.getState().dialogOpen).toBe(true);
    expect(createSessionMock).not.toHaveBeenCalled();
    expect(openNewMock).not.toHaveBeenCalled();
  });

  test("notification_confirm with unknown key does not create a session or start recording", async () => {
    render(<EventListeners />);

    await vi.waitFor(() =>
      expect(notificationListenMock).toHaveBeenCalledTimes(1),
    );

    const handler = notificationListenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "notification_confirm",
        key: "some-future-notification",
        source: null,
      },
    });

    expect(createSessionMock).not.toHaveBeenCalled();
    expect(openNewMock).not.toHaveBeenCalled();
  });

  test("notification_confirm with mic_detected source creates a session and sets triggerAppIds", async () => {
    render(<EventListeners />);

    await vi.waitFor(() =>
      expect(notificationListenMock).toHaveBeenCalledTimes(1),
    );

    const handler = notificationListenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "notification_confirm",
        source: {
          type: "mic_detected",
          app_names: ["Zoom"],
          app_ids: ["us.zoom.xos"],
        },
      },
    });

    await vi.waitFor(() => expect(openNewMock).toHaveBeenCalledTimes(1));

    expect(createSessionMock).toHaveBeenCalledTimes(1);
    expect(setTriggerAppIdsMock).toHaveBeenCalledWith(["us.zoom.xos"]);
    expect(openNewMock).toHaveBeenCalledWith({
      type: "sessions",
      id: "session-new",
      state: { view: null, autoStart: true },
    });
  });

  test("notification_option_selected with mic_detected source sets triggerAppIds", async () => {
    render(<EventListeners />);

    await vi.waitFor(() =>
      expect(notificationListenMock).toHaveBeenCalledTimes(1),
    );

    const handler = notificationListenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "notification_option_selected",
        selected_index: 0,
        source: {
          type: "mic_detected",
          app_names: ["Zoom"],
          app_ids: ["us.zoom.xos"],
        },
      },
    });

    expect(setTriggerAppIdsMock).toHaveBeenCalledWith(["us.zoom.xos"]);
    await vi.waitFor(() => expect(openNewMock).toHaveBeenCalledTimes(1));
  });

  test("notification_confirm opens without waiting for the legacy store", async () => {
    render(<EventListeners />);

    await vi.waitFor(() =>
      expect(notificationListenMock).toHaveBeenCalledTimes(1),
    );

    const handler = notificationListenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "notification_confirm",
        source: {
          type: "mic_detected",
          app_names: ["Zoom"],
          app_ids: ["us.zoom.xos"],
        },
      },
    });

    await vi.waitFor(() =>
      expect(setTriggerAppIdsMock).toHaveBeenCalledWith(["us.zoom.xos"]),
    );
    expect(openNewMock).toHaveBeenCalledTimes(1);
  });
});

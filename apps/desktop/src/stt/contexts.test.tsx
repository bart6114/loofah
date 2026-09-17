import { resolveResource } from "@tauri-apps/api/path";
import { cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { AUTO_STOP_CONFIRM_DELAY_MS, ListenerProvider } from "./contexts";

import { createListenerStore } from "~/store/zustand/listener";

const {
  listMicUsingApplicationsMock,
  listenMock,
  showNotificationMock,
  useConfigValueMock,
} = vi.hoisted(() => ({
  listMicUsingApplicationsMock: vi.fn(),
  listenMock: vi.fn(),
  showNotificationMock: vi.fn(),
  useConfigValueMock: vi.fn((_key: string) => true),
}));

vi.mock("@hypr/plugin-detect", () => ({
  commands: {
    listMicUsingApplications: listMicUsingApplicationsMock,
  },
  events: {
    detectEvent: {
      listen: listenMock,
    },
  },
}));

vi.mock("@hypr/plugin-notification", () => ({
  commands: {
    showNotification: showNotificationMock,
  },
}));

vi.mock("~/shared/config", () => ({
  useConfigValue: useConfigValueMock,
}));

function setStoreActive(
  store: ReturnType<typeof createListenerStore>,
  sessionId = "session-1",
) {
  store.setState((state) => ({
    live: { ...state.live, sessionId, status: "active" },
  }));
}

describe("ListenerProvider detect events", () => {
  beforeEach(() => {
    listenMock.mockReset();
    showNotificationMock.mockReset();
    useConfigValueMock.mockReset();
    useConfigValueMock.mockReturnValue(true);
    listenMock.mockResolvedValue(() => {});
    listMicUsingApplicationsMock.mockResolvedValue({ status: "ok", data: [] });
    Object.defineProperty(window.navigator, "onLine", {
      configurable: true,
      value: true,
    });
    vi.useRealTimers();
  });

  afterEach(() => {
    cleanup();
    vi.clearAllTimers();
    vi.useRealTimers();
  });

  test("does not stop listening on MicStopped when no trigger apps are set (manual session — regression: #5120)", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "micStopped",
        apps: [
          { id: "/opt/homebrew/bin/ffmpeg", name: "ffmpeg" },
          { id: "us.zoom.xos", name: "Zoom" },
        ],
      },
    });

    expect(stopSpy).not.toHaveBeenCalled();
  });

  test("stops listening after confirming a trigger app remains stopped", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store.getState().setTriggerAppIds(["us.zoom.xos"]);
    setStoreActive(store);

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    vi.useFakeTimers();
    listMicUsingApplicationsMock.mockClear();

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "us.zoom.xos", name: "Zoom" }],
      },
    });

    expect(stopSpy).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS);

    expect(listMicUsingApplicationsMock).toHaveBeenCalledTimes(1);
    expect(stopSpy).toHaveBeenCalledTimes(1);
  });

  test("does not stop when a trigger app resumes during the auto-stop grace period", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store.getState().setTriggerAppIds(["us.zoom.xos"]);
    setStoreActive(store);
    listMicUsingApplicationsMock.mockResolvedValue({
      status: "ok",
      data: [{ id: "us.zoom.xos", name: "Zoom" }],
    });

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    vi.useFakeTimers();
    listMicUsingApplicationsMock.mockClear();

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "us.zoom.xos", name: "Zoom" }],
      },
    });

    await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS);

    expect(listMicUsingApplicationsMock).toHaveBeenCalledTimes(1);
    expect(stopSpy).not.toHaveBeenCalled();
  });

  test("keeps standard auto-stop behavior for offline ad-hoc meetings", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store.getState().setTriggerAppIds(["us.zoom.xos"]);
    setStoreActive(store);

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));
    const handler = listenMock.mock.calls[0]?.[0];

    vi.useFakeTimers();
    window.dispatchEvent(new Event("offline"));
    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "us.zoom.xos", name: "Zoom" }],
      },
    });

    await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS);
    expect(stopSpy).toHaveBeenCalledTimes(1);
  });

  test("does not stop on MicStopped when auto-stop is disabled", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store.getState().setTriggerAppIds(["us.zoom.xos"]);
    useConfigValueMock.mockReturnValue(false);

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "us.zoom.xos", name: "Zoom" }],
      },
    });

    expect(stopSpy).not.toHaveBeenCalled();
  });

  test("does not stop on MicStopped when only a non-trigger app stops (auto-session — regression: #4846)", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store.getState().setTriggerAppIds(["us.zoom.xos"]);

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "/opt/homebrew/bin/ffmpeg", name: "ffmpeg" }],
      },
    });

    expect(stopSpy).not.toHaveBeenCalled();
  });

  test("does not stop after non-trigger MicStopped when a trigger app is still active", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store.getState().setTriggerAppIds(["us.zoom.xos"]);
    setStoreActive(store);
    listMicUsingApplicationsMock.mockResolvedValue({
      status: "ok",
      data: [{ id: "us.zoom.xos", name: "Zoom" }],
    });

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    vi.useFakeTimers();
    listMicUsingApplicationsMock.mockClear();

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "/opt/homebrew/bin/ffmpeg", name: "ffmpeg" }],
      },
    });

    await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS);

    expect(listMicUsingApplicationsMock).toHaveBeenCalledTimes(1);
    expect(stopSpy).not.toHaveBeenCalled();
  });

  test.each([
    [[{ id: "com.kakao.KakaoTalkMac", name: "KakaoTalk" }]],
    [[{ id: "pid:42", name: "KakaoTalk Helper" }]],
  ])(
    "does not auto-stop KakaoTalk sessions from screen-share mic transitions",
    async (stoppedApps) => {
      const store = createListenerStore();
      const stopSpy = vi.fn();

      store.setState({ stop: stopSpy });
      store.getState().setTriggerAppIds(["com.kakao.KakaoTalkMac"]);
      setStoreActive(store);

      render(
        <ListenerProvider store={store}>
          <div>child</div>
        </ListenerProvider>,
      );

      await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

      const handler = listenMock.mock.calls[0]?.[0];
      expect(handler).toBeTypeOf("function");

      vi.useFakeTimers();
      listMicUsingApplicationsMock.mockClear();

      handler({
        payload: {
          type: "micStopped",
          apps: stoppedApps,
        },
      });

      await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS);

      expect(listMicUsingApplicationsMock).not.toHaveBeenCalled();
      expect(stopSpy).not.toHaveBeenCalled();
    },
  );

  test("does not auto-stop co-trigger sessions while KakaoTalk remains active after a helper stop", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store
      .getState()
      .setTriggerAppIds(["com.kakao.KakaoTalkMac", "us.zoom.xos"]);
    setStoreActive(store);
    listMicUsingApplicationsMock.mockResolvedValue({
      status: "ok",
      data: [{ id: "com.kakao.KakaoTalkMac", name: "KakaoTalk" }],
    });

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    vi.useFakeTimers();
    listMicUsingApplicationsMock.mockClear();

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "pid:42", name: "KakaoTalk Helper" }],
      },
    });

    await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS);

    expect(listMicUsingApplicationsMock).toHaveBeenCalledTimes(1);
    expect(stopSpy).not.toHaveBeenCalled();
  });

  test("auto-stops co-trigger sessions after a helper stop when no trigger app remains active", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store
      .getState()
      .setTriggerAppIds(["com.kakao.KakaoTalkMac", "us.zoom.xos"]);
    setStoreActive(store);

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    vi.useFakeTimers();
    listMicUsingApplicationsMock.mockClear();

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "pid:42", name: "KakaoTalk Helper" }],
      },
    });

    await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS);

    expect(listMicUsingApplicationsMock).toHaveBeenCalledTimes(1);
    expect(stopSpy).toHaveBeenCalledTimes(1);
  });

  test("auto-stops when MicStopped omits the trigger app and no trigger app remains active (regression: #5436)", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store.getState().setTriggerAppIds(["com.microsoft.teams2"]);
    setStoreActive(store);

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    vi.useFakeTimers();
    listMicUsingApplicationsMock.mockClear();

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "pid:42", name: "Microsoft Teams Helper" }],
      },
    });

    await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS);

    expect(listMicUsingApplicationsMock).toHaveBeenCalledTimes(1);
    expect(stopSpy).toHaveBeenCalledTimes(1);
  });

  test("auto-stops Teams running in a browser when the browser no longer uses the mic (regression: #5436)", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store.getState().setTriggerAppIds(["company.thebrowser.Browser"]);
    setStoreActive(store, "session-1");

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    vi.useFakeTimers();
    listMicUsingApplicationsMock.mockClear();

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "company.thebrowser.Browser", name: "Arc" }],
      },
    });

    await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS);

    expect(listMicUsingApplicationsMock).toHaveBeenCalledTimes(1);
    expect(stopSpy).toHaveBeenCalledTimes(1);
    expect(showNotificationMock).not.toHaveBeenCalled();
  });

  test("keeps direct trigger auto-stop confidence when a later helper stop arrives", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store.getState().setTriggerAppIds(["us.zoom.xos"]);
    setStoreActive(store);
    listMicUsingApplicationsMock.mockResolvedValue({
      status: "error",
      error: "failed to read mic snapshot",
    });

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    vi.useFakeTimers();
    listMicUsingApplicationsMock.mockClear();

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "us.zoom.xos", name: "Zoom" }],
      },
    });

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "pid:42", name: "Zoom Helper" }],
      },
    });

    await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS);

    expect(listMicUsingApplicationsMock).toHaveBeenCalledTimes(1);
    expect(stopSpy).toHaveBeenCalledTimes(1);
  });

  test("passes ignorable app ids and footer metadata through mic-detected notifications", async () => {
    const store = createListenerStore();

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "micDetected",
        key: "mic-1",
        apps: [
          { id: "pid:42", name: "Zoom" },
          { id: "us.zoom.xos", name: "Zoom" },
        ],
        duration_secs: 0,
      },
    });

    await vi.waitFor(() =>
      expect(showNotificationMock).toHaveBeenCalledWith(
        expect.objectContaining({
          timeout: { secs: 30, nanos: 0 },
          source: {
            type: "mic_detected",
            app_names: ["Zoom", "Zoom"],
            app_ids: ["us.zoom.xos"],
          },
          footer: {
            text: "Zoom",
            actionLabel: "Always ignore",
            icon: {
              type: "path",
              path: "/resources/notification-icons/zoom.svg",
            },
          },
          icon: null,
        }),
      ),
    );
  });

  test("does not show mic-detected prompts when detection notifications are disabled", async () => {
    const store = createListenerStore();
    useConfigValueMock.mockImplementation((key: string) =>
      key === "notification_detect" ? false : true,
    );

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "micDetected",
        key: "mic-1",
        apps: [{ id: "us.zoom.xos", name: "Zoom" }],
        duration_secs: 0,
      },
    });

    await Promise.resolve();

    expect(showNotificationMock).not.toHaveBeenCalled();
  });

  test("shows iPhone call icon and label for AV Capture mic notifications", async () => {
    const store = createListenerStore();

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "micDetected",
        key: "mic-1",
        apps: [{ id: "pid:42", name: "AV Capture" }],
        duration_secs: 0,
      },
    });

    await vi.waitFor(() =>
      expect(showNotificationMock).toHaveBeenCalledWith(
        expect.objectContaining({
          source: {
            type: "mic_detected",
            app_names: ["iPhone Call"],
            app_ids: [],
          },
          footer: null,
          icon: null,
        }),
      ),
    );
  });

  test("shows iPhone call icon and label for avconferenced mic notifications", async () => {
    const store = createListenerStore();

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "micDetected",
        key: "mic-1",
        apps: [{ id: "/usr/libexec/avconferenced", name: "avconferenced" }],
        duration_secs: 0,
      },
    });

    await vi.waitFor(() =>
      expect(showNotificationMock).toHaveBeenCalledWith(
        expect.objectContaining({
          source: {
            type: "mic_detected",
            app_names: ["iPhone Call"],
            app_ids: ["/usr/libexec/avconferenced"],
          },
          footer: {
            text: "iPhone Call",
            actionLabel: "Always ignore",
            icon: {
              type: "path",
              path: "/resources/notification-icons/phone.png",
            },
          },
          icon: null,
        }),
      ),
    );
  });

  test("does not show a stale mic prompt when listening starts while icons resolve", async () => {
    const store = createListenerStore();
    let iconResolverReady = false;
    let resolveIcon = () => {};

    vi.mocked(resolveResource).mockImplementationOnce(
      (path: string) =>
        new Promise((resolve) => {
          resolveIcon = () => {
            resolve(`/resources/${path}`);
          };
          iconResolverReady = true;
        }),
    );
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-24T02:09:00.000Z"));

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "micDetected",
        key: "mic-1",
        apps: [{ id: "com.microsoft.teams", name: "Microsoft Teams" }],
        duration_secs: 0,
      },
    });

    await vi.waitFor(() => expect(iconResolverReady).toBe(true));

    setStoreActive(store);
    resolveIcon();

    await vi.waitFor(() =>
      expect(store.getState().live.triggerAppIds).toEqual([
        "com.microsoft.teams",
      ]),
    );
    expect(showNotificationMock).not.toHaveBeenCalled();
  });

  test("does not show duplicate mic prompts while icons resolve", async () => {
    const store = createListenerStore();
    let iconResolverReady = false;
    let resolveIcon = () => {};

    vi.mocked(resolveResource).mockImplementationOnce(
      (path: string) =>
        new Promise((resolve) => {
          resolveIcon = () => {
            resolve(`/resources/${path}`);
          };
          iconResolverReady = true;
        }),
    );
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-24T02:09:00.000Z"));

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "micDetected",
        key: "mic-1",
        apps: [{ id: "com.slack.Slack", name: "Slack" }],
        duration_secs: 0,
      },
    });

    await vi.waitFor(() => expect(iconResolverReady).toBe(true));

    handler({
      payload: {
        type: "micDetected",
        key: "mic-2",
        apps: [{ id: "com.slack.Slack", name: "Slack" }],
        duration_secs: 0,
      },
    });

    resolveIcon();

    await vi.waitFor(() =>
      expect(showNotificationMock).toHaveBeenCalledTimes(1),
    );
    expect(showNotificationMock).toHaveBeenCalledWith(
      expect.objectContaining({
        key: "mic-1",
        source: expect.objectContaining({
          app_names: ["Slack"],
        }),
      }),
    );
  });

  test("records trigger app ids from micDetected while already listening", async () => {
    const store = createListenerStore();

    setStoreActive(store);

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "micDetected",
        key: "mic-1",
        apps: [
          { id: "pid:42", name: "Chrome Helper" },
          { id: "com.google.Chrome", name: "Google Chrome" },
        ],
        duration_secs: 0,
      },
    });

    expect(showNotificationMock).not.toHaveBeenCalled();
    expect(store.getState().live.triggerAppIds).toEqual(["com.google.Chrome"]);
  });

  test("records trigger app ids from micDetected while listening is starting", async () => {
    const store = createListenerStore();

    store.setState((state) => ({
      live: {
        ...state.live,
        loading: true,
        sessionId: "session-1",
        status: "inactive",
      },
    }));

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "micDetected",
        key: "mic-1",
        apps: [
          { id: "pid:42", name: "Chrome Helper" },
          { id: "com.google.Chrome", name: "Google Chrome" },
        ],
        duration_secs: 0,
      },
    });

    expect(showNotificationMock).not.toHaveBeenCalled();
    expect(store.getState().live.triggerAppIds).toEqual(["com.google.Chrome"]);
  });

  test("auto-stops after a trigger app learned during active listening stops", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    setStoreActive(store);

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    vi.useFakeTimers();
    listMicUsingApplicationsMock.mockClear();

    handler({
      payload: {
        type: "micDetected",
        key: "mic-1",
        apps: [{ id: "us.zoom.xos", name: "Zoom" }],
        duration_secs: 0,
      },
    });

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "us.zoom.xos", name: "Zoom" }],
      },
    });

    await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS);

    expect(listMicUsingApplicationsMock).toHaveBeenCalledTimes(1);
    expect(stopSpy).toHaveBeenCalledTimes(1);
  });

  test.each([
    { id: "com.google.Chrome", name: "Google Chrome" },
    { id: "at.studio.AsideBrowser", name: "Aside" },
    { id: "net.imput.helium", name: "Helium" },
  ])("auto-stops when $name's mic use stops", async (browser) => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store.getState().setTriggerAppIds([browser.id]);
    setStoreActive(store);

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    vi.useFakeTimers();
    listMicUsingApplicationsMock.mockClear();

    handler({
      payload: {
        type: "micStopped",
        apps: [browser],
      },
    });

    await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS);

    expect(listMicUsingApplicationsMock).toHaveBeenCalledTimes(1);
    expect(stopSpy).toHaveBeenCalledTimes(1);
    expect(showNotificationMock).not.toHaveBeenCalled();
  });

  test("cancels pending auto-stop when a browser trigger restarts", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });
    store.getState().setTriggerAppIds(["com.google.Chrome"]);
    setStoreActive(store);

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    vi.useFakeTimers();
    listMicUsingApplicationsMock.mockClear();

    handler({
      payload: {
        type: "micStopped",
        apps: [{ id: "com.google.Chrome", name: "Google Chrome" }],
      },
    });

    await vi.advanceTimersByTimeAsync(AUTO_STOP_CONFIRM_DELAY_MS - 1);

    handler({
      payload: {
        type: "micDetected",
        key: "mic-1",
        apps: [{ id: "com.google.Chrome", name: "Google Chrome" }],
        duration_secs: 0,
      },
    });

    await vi.advanceTimersByTimeAsync(1);

    expect(listMicUsingApplicationsMock).not.toHaveBeenCalled();
    expect(stopSpy).not.toHaveBeenCalled();
  });

  test("stops listening when sleep starts", async () => {
    const store = createListenerStore();
    const stopSpy = vi.fn();

    store.setState({ stop: stopSpy });

    render(
      <ListenerProvider store={store}>
        <div>child</div>
      </ListenerProvider>,
    );

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    const handler = listenMock.mock.calls[0]?.[0];
    expect(handler).toBeTypeOf("function");

    handler({
      payload: {
        type: "sleepStateChanged",
        value: true,
      },
    });

    expect(stopSpy).toHaveBeenCalledTimes(1);
  });
});

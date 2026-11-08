import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
const fake = vi.hoisted(() => ({
  actions: { startPending: vi.fn() },
  detect: null as ((event: any) => void) | null,
  notification: null as ((event: any) => void) | null,
  stopEvent: null as ((event: any) => void) | null,
  show: vi.fn(),
  update: vi.fn(),
  hide: vi.fn(),
  notice: vi.fn(),
  mic: vi.fn(),
  ready: true,
  initialize: vi.fn(),
  options: {
    notification_detect: true,
    auto_stop_meetings: true,
    floating_bar_enabled: false,
    floating_bar_opacity: 0.7,
    ignored_platforms: [],
  },
}));
vi.mock("./device-recording-controls", () => ({
  useDeviceStartActions: () => fake.actions,
  activeDeviceCapture: (item: any) =>
    ["preparing", "starting", "capturing", "stopping", "pending"].includes(
      item.phase,
    ),
}));
vi.mock("@tauri-apps/api/app", () => ({ getIdentifier: async () => "loofah" }));
vi.mock("@hypr/plugin-detect", () => ({
  commands: { listMicUsingApplications: fake.mic },
  events: {
    detectEvent: {
      listen: vi.fn(async (callback: any) => {
        fake.detect = callback;
        return vi.fn();
      }),
    },
  },
}));
vi.mock("@hypr/plugin-notification", () => ({
  commands: { showNotification: fake.notice },
  events: {
    notificationEvent: {
      listen: vi.fn(async (callback: any) => {
        fake.notification = callback;
        return vi.fn();
      }),
    },
  },
}));
vi.mock("@hypr/plugin-windows", () => ({
  getCurrentWebviewWindowLabel: () => "main",
  commands: {
    floatingBarUpdate: fake.update,
    floatingBarShow: fake.show,
    floatingBarHide: fake.hide,
    windowShow: vi.fn(),
  },
  events: {
    floatingBarStop: {
      listen: vi.fn(async (callback: any) => {
        fake.stopEvent = callback;
        return vi.fn();
      }),
    },
    floatingBarOpenMain: { listen: vi.fn(async () => vi.fn()) },
    floatingBarSettingsChange: { listen: vi.fn(async () => vi.fn()) },
  },
}));
vi.mock("~/shared/config", () => ({ useConfigValues: () => fake.options }));
vi.mock("~/settings/queries", () => ({
  useSettingsReady: () => fake.ready,
  initializeDeviceSettings: fake.initialize,
  setSettingValue: vi.fn(),
  setSettingValues: vi.fn(),
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: vi.fn() },
}));
import { DeviceRecordingShell } from "./device-recording-shell";
const identity = (id = "first") => ({
  scope: { server: "https://server/", member_id: "member", vault_id: "vault" },
  capture_id: id,
  request_id: id,
  target: { kind: "pending_session", create_request_id: id },
});
const item = (id = "first") => ({
  profile_id: "profile",
  identity: identity(id),
  phase: "capturing",
  failure: null,
});
const caches: QueryClient[] = [];
beforeEach(() => {
  vi.clearAllMocks();
  fake.actions = { startPending: vi.fn() };
  fake.options = { ...fake.options, floating_bar_enabled: false };
  fake.initialize.mockResolvedValue(undefined);
  fake.mic.mockResolvedValue({ status: "ok", data: [] });
  fake.notice.mockResolvedValue(undefined);
  fake.update.mockResolvedValue({ status: "ok", data: null });
  fake.show.mockResolvedValue({ status: "ok", data: null });
  fake.hide.mockResolvedValue({ status: "ok", data: null });
});
afterEach(() => {
  cleanup();
  for (const cache of caches.splice(0)) cache.clear();
  vi.useRealTimers();
});
function mount(records: any[] = []) {
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  caches.push(cache);
  cache.setQueryData(["device", "recordings", "profile"], records);
  const monitor = {
    recoveryInventory: vi.fn(async () => records),
    stop: vi.fn(async () => true),
    telemetry: vi.fn(async () => ({ mic: 500, speaker: 200 })),
  };
  const ui = () => (
    <QueryClientProvider client={cache}>
      <DeviceRecordingShell profileId="profile" monitor={monitor as any} />
    </QueryClientProvider>
  );
  const view = render(ui());
  return { cache, monitor, ...view, redraw: () => view.rerender(ui()) };
}
it("binds a reminder to the original start action and never redirects acceptance to a new destination", async () => {
  const original = fake.actions.startPending.mockResolvedValue(item());
  const view = mount();
  await waitFor(() => expect(fake.detect).toBeTruthy());
  act(() =>
    fake.detect!({
      payload: {
        type: "micDetected",
        key: "prompt",
        apps: [{ id: "meeting", name: "Meeting" }],
      },
    }),
  );
  await waitFor(() => expect(fake.notice).toHaveBeenCalledOnce());
  const next = vi.fn();
  fake.actions = { startPending: next };
  view.redraw();
  act(() =>
    fake.notification!({
      payload: {
        type: "notification_accept",
        key: "prompt",
        source: { type: "mic_detected" },
      },
    }),
  );
  await waitFor(() =>
    expect(original).toHaveBeenCalledWith({ triggerAppIds: ["meeting"] }),
  );
  expect(next).not.toHaveBeenCalled();
  act(() =>
    fake.notification!({
      payload: {
        type: "notification_accept",
        key: "prompt",
        source: { type: "mic_detected" },
      },
    }),
  );
  expect(original).toHaveBeenCalledOnce();
});
it("stops only the exact rendered original identity after another recording becomes current", async () => {
  fake.options.floating_bar_enabled = true;
  const view = mount([item()]);
  await waitFor(() => expect(fake.update).toHaveBeenCalled());
  const originalKey = fake.update.mock.calls[0][0].captureKey;
  expect(originalKey).toBeTruthy();
  act(() =>
    view.cache.setQueryData(
      ["device", "recordings", "profile"],
      [item("second")],
    ),
  );
  await waitFor(() =>
    expect(
      fake.update.mock.calls.some(
        ([state]) => state.captureKey !== originalKey,
      ),
    ).toBe(true),
  );
  act(() => fake.stopEvent!({ payload: { capture_key: originalKey } }));
  await waitFor(() =>
    expect(view.monitor.stop).toHaveBeenCalledWith(identity()),
  );
  act(() => fake.stopEvent!({ payload: {} }));
  act(() => fake.stopEvent!({ payload: { capture_key: "foreign" } }));
  expect(view.monitor.stop).toHaveBeenCalledOnce();
});
it("drops delayed telemetry from the retired overlay and ends polling on unmount", async () => {
  fake.options.floating_bar_enabled = true;
  const view = mount([item()]);
  await waitFor(() => expect(view.monitor.telemetry).toHaveBeenCalled());
  view.unmount();
  const count = view.monitor.telemetry.mock.calls.length;
  await new Promise((resolve) => setTimeout(resolve, 150));
  expect(view.monitor.telemetry).toHaveBeenCalledTimes(count);
  act(() => fake.stopEvent!({ payload: { capture_key: "anything" } }));
  expect(view.monitor.stop).not.toHaveBeenCalled();
});

it("rechecks the exact capture and microphone snapshot before automatic stop", async () => {
  const view = mount([item()]);
  await waitFor(() => expect(fake.detect).toBeTruthy());
  act(() =>
    fake.detect!({
      payload: {
        type: "micDetected",
        key: "active",
        apps: [{ id: "meeting", name: "Meeting" }],
      },
    }),
  );
  vi.useFakeTimers();
  act(() =>
    fake.detect!({
      payload: {
        type: "micStopped",
        apps: [{ id: "meeting", name: "Meeting" }],
      },
    }),
  );
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(view.monitor.stop).toHaveBeenCalledWith(identity());
});
it("does not stop a newer capture when an old microphone-stop timer expires", async () => {
  const view = mount([item()]);
  await waitFor(() => expect(fake.detect).toBeTruthy());
  act(() =>
    fake.detect!({
      payload: {
        type: "micDetected",
        key: "active",
        apps: [{ id: "meeting", name: "Meeting" }],
      },
    }),
  );
  vi.useFakeTimers();
  act(() =>
    fake.detect!({
      payload: {
        type: "micStopped",
        apps: [{ id: "meeting", name: "Meeting" }],
      },
    }),
  );
  await act(async () => {
    view.cache.setQueryData(["device", "recordings", "profile"], [item("new")]);
    await vi.advanceTimersByTimeAsync(1);
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(view.monitor.stop).not.toHaveBeenCalled();
});

it("abandons automatic stop after a transient network interruption", async () => {
  const view = mount([item()]);
  await waitFor(() => expect(fake.detect).toBeTruthy());
  const apps = [{ id: "meeting", name: "Meeting" }];
  act(() =>
    fake.detect!({ payload: { type: "micDetected", key: "active", apps } }),
  );
  vi.useFakeTimers();
  act(() => fake.detect!({ payload: { type: "micStopped", apps } }));
  act(() => window.dispatchEvent(new Event("offline")));
  act(() => window.dispatchEvent(new Event("online")));
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(view.monitor.stop).not.toHaveBeenCalled();
});
it("stops only the original physical capture on sleep and ignores wake", async () => {
  const view = mount([item()]);
  await waitFor(() => expect(fake.detect).toBeTruthy());
  act(() =>
    fake.detect!({ payload: { type: "sleepStateChanged", value: true } }),
  );
  await waitFor(() =>
    expect(view.monitor.stop).toHaveBeenCalledWith(identity()),
  );
  act(() =>
    fake.detect!({ payload: { type: "sleepStateChanged", value: false } }),
  );
  expect(view.monitor.stop).toHaveBeenCalledOnce();
});

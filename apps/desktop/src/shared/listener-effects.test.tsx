import { act, renderHook } from "@testing-library/react";
import { StrictMode } from "react";
import { beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  navigate: vi.fn(),
  openNew: vi.fn(),
  openNewNote: vi.fn(),
  emit: vi.fn(),
  registrations: [] as {
    handler: (event: any) => void;
    resolve: (fn: () => void) => void;
    reject: (e: Error) => void;
  }[],
}));
vi.mock("@tanstack/react-router", () => ({
  useNavigate: () => mocks.navigate,
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({}),
}));
vi.mock("~/shared/useNewNote", () => ({ useNewNote: () => mocks.openNewNote }));
vi.mock("~/store/zustand/tabs", () => ({
  useTabs: (select: any) => select({ openNew: mocks.openNew }),
  isTabInputSupported: () => true,
}));
vi.mock("@hypr/plugin-windows", () => ({
  getCurrentWebviewWindowLabel: () => "main",
  events: {
    navigate: () => ({ listen: register }),
    openTab: () => ({ listen: register }),
  },
}));
vi.mock("@hypr/plugin-updater2", () => ({
  commands: { maybeEmitUpdated: mocks.emit },
  events: { updatedEvent: { listen: register } },
}));
function register(handler: (event: any) => void) {
  return new Promise<() => void>((resolve, reject) =>
    mocks.registrations.push({ handler, resolve, reject }),
  );
}
import { useNavigationEvents } from "./hooks/useNavigationEvents";

import { useUpdaterEvents } from "~/services/updater-events";

beforeEach(() => {
  vi.clearAllMocks();
  mocks.registrations.length = 0;
});

it.each(["early", "late", "strict"])(
  "cleans updater/navigation/open-tab registrations on %s teardown",
  async (mode) => {
    const hook = renderHook(
      () => {
        useNavigationEvents();
        useUpdaterEvents();
      },
      mode === "strict" ? { wrapper: StrictMode } : {},
    );
    const cleanups = mocks.registrations.map(() => vi.fn());
    if (mode === "early") hook.unmount();
    await act(async () => {
      mocks.registrations.forEach((registration, i) =>
        registration.resolve(cleanups[i]),
      );
    });
    if (mode !== "early") hook.unmount();
    mocks.navigate.mockClear();
    mocks.openNew.mockClear();
    mocks.emit.mockClear();
    mocks.registrations.forEach((r) =>
      r.handler({
        payload: {
          path: "/app/test",
          tab: { type: "sessions", id: "s" },
          previous: "1",
          current: "2",
        },
      }),
    );
    cleanups.forEach((fn) => expect(fn).toHaveBeenCalledOnce());
    expect(mocks.navigate).not.toHaveBeenCalled();
    expect(mocks.openNew).not.toHaveBeenCalled();
    expect(mocks.emit).not.toHaveBeenCalled();
  },
);

it("does not request updater emission when registration resolves after unmount", async () => {
  const hook = renderHook(useUpdaterEvents);
  hook.unmount();
  await act(async () => {
    mocks.registrations[0].resolve(vi.fn());
  });
  expect(mocks.emit).not.toHaveBeenCalled();
});

it("cleans replaced navigation dependencies and handles registration rejection", async () => {
  const hook = renderHook(useNavigationEvents);
  const old = [...mocks.registrations];
  mocks.navigate = vi.fn();
  hook.rerender();
  const cleanup = vi.fn();
  await act(async () => {
    old[0].resolve(cleanup);
    old[1].reject(new Error("registration failure"));
    mocks.registrations.slice(2).forEach((r) => r.resolve(vi.fn()));
  });
  old[0].handler({ payload: { path: "/old" } });
  expect(cleanup).toHaveBeenCalledOnce();
  expect(mocks.navigate).not.toHaveBeenCalled();
  hook.unmount();
});

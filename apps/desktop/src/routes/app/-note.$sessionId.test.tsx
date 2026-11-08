import {
  act,
  cleanup,
  render,
  renderHook,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  resetTabsStore,
  TabsTestProvider,
  tabsTestStore as useTabs,
} from "~/store/zustand/tabs/test-utils";

const mocks = vi.hoisted(() => ({
  label: "note-local",
  remote: vi.fn(),
  attachLiveSession: vi.fn(),
  close: vi.fn(),
  listenerState: {
    attachLiveSession: vi.fn(),
    live: {
      eventUnlistenersBySession: {} as Record<string, (() => void)[]>,
    },
  },
}));

vi.mock("@hypr/plugin-windows", () => ({
  getCurrentWebviewWindowLabel: () => mocks.label,
}));
vi.mock("~/shared/application/remote-note-window", () => ({
  RemoteNoteWindow: () => {
    mocks.remote();
    return <output>original remote claim</output>;
  },
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    close: mocks.close,
  }),
}));

vi.mock("~/stt/contexts", () => ({
  useListener: (selector: (state: typeof mocks.listenerState) => unknown) =>
    selector(mocks.listenerState),
}));
vi.mock("~/main/layout", async () => {
  const { TabsTestProvider } = await import("~/store/zustand/tabs/test-utils");
  return {
    ClassicMainLayout: ({ children }: { children: React.ReactNode }) => (
      <TabsTestProvider>{children}</TabsTestProvider>
    ),
  };
});
vi.mock("~/session", () => ({
  TabContentNote: ({ tab }: { tab: { id: string } }) => (
    <output>{tab.id}</output>
  ),
}));
vi.mock("~/shared/window-shell", () => ({
  StandaloneWindowShell: ({ children }: { children: React.ReactNode }) =>
    children,
}));

import {
  useAttachStandaloneNoteToLiveSession,
  useCloseStandaloneNoteWindowOnEscape,
  useStandaloneNoteTab,
  Route,
} from "./note.$sessionId";

describe("standalone note window route", () => {
  beforeEach(() => {
    mocks.label = "note-local";
    mocks.remote.mockClear();
    mocks.listenerState.attachLiveSession = mocks.attachLiveSession;
    mocks.listenerState.live.eventUnlistenersBySession = {};
    mocks.attachLiveSession.mockClear();
    mocks.close.mockClear();
    resetTabsStore();
  });

  afterEach(() => {
    cleanup();
  });

  it("closes the standalone note window on escape", () => {
    renderHook(() => useCloseStandaloneNoteWindowOnEscape());

    const event = dispatchKeyDown("Escape");

    expect(event.defaultPrevented).toBe(true);
    expect(mocks.close).toHaveBeenCalledTimes(1);
  });

  it("resolves standalone tab hooks inside its explicit local layout provider", async () => {
    const params = vi
      .spyOn(Route, "useParams")
      .mockReturnValue({ sessionId: "standalone-local" });
    const Component = Route.options.component!;
    render(<Component />);
    expect(screen.getByRole("status").textContent).toBe("standalone-local");
    expect(useTabs.getState().currentTab).toMatchObject({
      id: "standalone-local",
    });
    expect(mocks.attachLiveSession).toHaveBeenCalledWith("standalone-local");
    params.mockRestore();
  });

  it("uses the Native remote grant instead of the renderer route's session or Local listener", () => {
    mocks.label = "remote-note-native-grant";
    const params = vi
      .spyOn(Route, "useParams")
      .mockReturnValue({ sessionId: "forged-session" });
    const Component = Route.options.component!;
    render(<Component />);
    expect(screen.getByText("original remote claim")).toBeTruthy();
    expect(mocks.attachLiveSession).not.toHaveBeenCalled();
    params.mockRestore();
  });

  it("ignores other keys", () => {
    renderHook(() => useCloseStandaloneNoteWindowOnEscape());

    const event = dispatchKeyDown("Enter");

    expect(event.defaultPrevented).toBe(false);
    expect(mocks.close).not.toHaveBeenCalled();
  });

  it("returns the subscribed standalone note tab after tab state updates", async () => {
    const { result } = renderHook(() => useStandaloneNoteTab("session-1"), {
      wrapper: TabsTestProvider,
    });

    await waitFor(() => {
      expect(useTabs.getState().tabs).toHaveLength(1);
    });

    const tab = useTabs.getState().tabs[0];
    expect(tab).toMatchObject({
      active: true,
      id: "session-1",
      type: "sessions",
    });

    act(() => {
      useTabs.getState().updateSessionTabState(tab, {
        autoStart: null,
        view: { type: "raw" },
      });
    });

    expect(result.current.state.view).toEqual({ type: "raw" });
  });

  it("attaches the standalone note to live session events", () => {
    renderHook(() => useAttachStandaloneNoteToLiveSession("session-1"));

    expect(mocks.attachLiveSession).toHaveBeenCalledWith("session-1");
  });

  it("reattaches after standalone live session events are removed", () => {
    const { rerender } = renderHook(() =>
      useAttachStandaloneNoteToLiveSession("session-1"),
    );
    expect(mocks.attachLiveSession).toHaveBeenCalledTimes(1);

    mocks.listenerState.live.eventUnlistenersBySession = {
      "session-1": [vi.fn()],
    };
    rerender();
    expect(mocks.attachLiveSession).toHaveBeenCalledTimes(1);

    mocks.listenerState.live.eventUnlistenersBySession = {};
    rerender();
    expect(mocks.attachLiveSession).toHaveBeenCalledTimes(2);
  });
});

function dispatchKeyDown(key: string) {
  const event = new KeyboardEvent("keydown", {
    bubbles: true,
    cancelable: true,
    key,
  });
  window.dispatchEvent(event);
  return event;
}

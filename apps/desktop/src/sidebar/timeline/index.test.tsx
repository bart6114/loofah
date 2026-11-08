import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  anchorNode: null as HTMLDivElement | null,
  openNew: vi.fn(),
  registerAnchor: vi.fn(),
  useAutoScrollToAnchor: vi.fn(),
  invalidateResource: vi.fn(),
  clearSelection: vi.fn(),
  currentTab: { type: "empty" } as
    | { type: "empty" }
    | { type: "sessions"; id: string },
  deleteSession: vi.fn(),
  configValue: undefined as string | undefined,
  configValues: {} as Record<string, unknown>,
  setSettingValue: vi.fn(),
  currentTimeMs: undefined as number | undefined,
  isAnchorVisible: true,
  isScrolledPastAnchor: false,
  liveSessionId: null as string | null,
  liveStatus: "inactive" as "inactive" | "active" | "finalizing",
  selectAll: vi.fn(),
  smartCurrentTimeMs: undefined as number | undefined,
  timelineSelectionAnchorId: null as string | null,
  timelineSelectionSelectedIds: [] as string[],
  timelineSessionsTable: {} as Record<string, Record<string, unknown>>,
  virtualRange: null as null | { startIndex: number; endIndex: number },
  virtualScrollToIndex: vi.fn(),
}));

const lingui = vi.hoisted(() => {
  const t = (
    input:
      | TemplateStringsArray
      | { message?: string; values?: Record<string, unknown> }
      | string,
    ...values: unknown[]
  ) => {
    if (Array.isArray(input)) {
      const message = input.reduce(
        (message, part, index) =>
          `${message}${part}${index < values.length ? String(values[index]) : ""}`,
        "",
      );

      return message === "Now" ? "Localized now" : message;
    }

    if (typeof input === "string") {
      return input;
    }

    if ("message" in input) {
      if (input.message === "Now") {
        return "Localized now";
      }

      return (input.message ?? "").replace(
        /\{(\w+)\}/g,
        (_match: string, key: string) =>
          String(input.values?.[key] ?? `{${key}}`),
      );
    }

    return "";
  };

  return { t };
});

vi.mock("@lingui/react/macro", () => ({
  Trans: ({
    children,
    id,
    message,
  }: {
    children?: ReactNode;
    id?: string;
    message?: string;
  }) => <>{children ?? message ?? id}</>,
  useLingui: () => ({
    _: lingui.t,
    t: lingui.t,
  }),
}));

vi.mock("@lingui/react", () => ({
  Trans: ({
    children,
    id,
    message,
  }: {
    children?: ReactNode;
    id?: string;
    message?: string;
  }) => <>{children ?? message ?? id}</>,
  useLingui: () => ({
    _: lingui.t,
    t: lingui.t,
  }),
}));

vi.mock("@tanstack/react-virtual", async () => {
  const React = await vi.importActual<typeof import("react")>("react");

  return {
    defaultRangeExtractor: ({
      startIndex,
      endIndex,
    }: {
      startIndex: number;
      endIndex: number;
    }) =>
      Array.from(
        { length: Math.max(0, endIndex - startIndex + 1) },
        (_, index) => startIndex + index,
      ),
    useVirtualizer: (options: {
      count: number;
      estimateSize: (index: number) => number;
      getItemKey: (index: number) => string | number;
      getScrollElement: () => HTMLDivElement | null;
      rangeExtractor: (range: {
        startIndex: number;
        endIndex: number;
        overscan: number;
        count: number;
      }) => number[];
      scrollMargin: number;
    }) => {
      const [, rerender] = React.useReducer((count) => count + 1, 0);
      React.useEffect(() => {
        const element = options.getScrollElement();
        if (!element) return;
        element.addEventListener("scroll", rerender);
        return () => element.removeEventListener("scroll", rerender);
      }, [options.getScrollElement]);

      let start = options.scrollMargin;
      const measurements = Array.from({ length: options.count }, (_, index) => {
        const size = options.estimateSize(index);
        const measurement = {
          end: start + size,
          index,
          key: options.getItemKey(index),
          size,
          start,
        };
        start += size;
        return measurement;
      });
      const range = mocks.virtualRange ?? {
        startIndex: 0,
        endIndex: Math.max(0, options.count - 1),
      };
      const indexes = options.rangeExtractor({
        ...range,
        overscan: 0,
        count: options.count,
      });
      const element = options.getScrollElement();
      const scrollOffset = mocks.isScrolledPastAnchor
        ? 10_000
        : (element?.scrollTop ?? 0);
      const viewportHeight = mocks.isAnchorVisible
        ? 100_000
        : (element?.clientHeight ?? 0);

      return {
        getTotalSize: () => Math.max(0, start - options.scrollMargin),
        getVirtualItems: () =>
          indexes.flatMap((index) => measurements[index] ?? []),
        isAtEnd: (threshold = 0) =>
          !element ||
          element.scrollHeight - element.clientHeight - element.scrollTop <=
            threshold,
        options,
        scrollOffset,
        scrollRect: { height: viewportHeight, width: 240 },
        scrollToIndex: mocks.virtualScrollToIndex,
      };
    },
  };
});

vi.mock("~/shared/config", () => ({
  // mocks.configValue keeps serving the timezone key for the older tests;
  // key-aware overrides go through mocks.configValues.
  useConfigValue: (key: string) => {
    if (key in mocks.configValues) {
      return mocks.configValues[key];
    }
    if (key === "sidebar_group_by") {
      return "date";
    }
    if (key === "sidebar_expanded_tags") {
      return [];
    }
    return mocks.configValue;
  },
}));

vi.mock("~/settings/queries", () => ({
  setSettingValue: (key: string, value: string) =>
    Promise.resolve(mocks.setSettingValue(key, value)),
}));

vi.mock("~/ai/hooks/useEnhancingSessions", () => ({
  useEnhancingSessionIds: () => [],
}));

vi.mock("./queries", () => ({
  useTimelineSessionsTable: () => mocks.timelineSessionsTable,
}));

vi.mock("~/session/hooks/useDeleteSession", () => ({
  useDeleteSession: () => mocks.deleteSession,
}));

vi.mock("~/shared/hooks/useNativeContextMenu", () => ({
  useNativeContextMenu: () => vi.fn(),
}));

vi.mock("~/store/zustand/tabs", () => ({
  useTabs: (selector: (state: unknown) => unknown) =>
    selector({
      currentTab: mocks.currentTab,
      invalidateResource: mocks.invalidateResource,
      openNew: mocks.openNew,
    }),
}));

vi.mock("~/store/zustand/timeline-selection", () => ({
  useTimelineSelection: (selector: (state: unknown) => unknown) =>
    selector({
      anchorId: mocks.timelineSelectionAnchorId,
      clear: mocks.clearSelection,
      selectAll: mocks.selectAll,
      selectedIds: mocks.timelineSelectionSelectedIds,
    }),
}));

vi.mock("~/stt/contexts", () => ({
  useListener: (
    selector: (state: {
      live: {
        sessionId: string | null;
        status: "inactive" | "active" | "finalizing";
      };
    }) => unknown,
  ) =>
    selector({
      live: {
        sessionId: mocks.liveSessionId,
        status: mocks.liveStatus,
      },
    }),
}));

vi.mock("./item", () => ({
  TimelineItemComponent: ({
    isUpcoming,
    item,
    upcomingProgress,
  }: {
    isUpcoming?: boolean;
    item: { id: string };
    upcomingProgress?: number;
  }) => (
    <div
      data-testid={`timeline-item-${item.id}`}
      data-upcoming={isUpcoming ? "true" : undefined}
      data-upcoming-progress={upcomingProgress}
    />
  ),
}));

vi.mock("./realtime", async () => {
  const React = await vi.importActual<typeof import("react")>("react");

  return {
    CurrentTimeIndicator: React.forwardRef<HTMLDivElement>(
      function CurrentTimeIndicator(_props, ref) {
        return <div ref={ref} data-testid="current-time-indicator" />;
      },
    ),
    useCurrentTimeMs: () => mocks.currentTimeMs ?? Date.now(),
    useSmartCurrentTime: () => mocks.smartCurrentTimeMs ?? Date.now(),
  };
});

import { TimelineView } from ".";

function sessionRow({
  title,
  started_at,
}: {
  title: string;
  started_at: string;
}) {
  return {
    title,
    created_at: started_at,
  };
}

describe("TimelineView", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.anchorNode = null;
    mocks.configValue = undefined;
    mocks.configValues = {};
    mocks.currentTimeMs = undefined;
    mocks.isAnchorVisible = true;
    mocks.isScrolledPastAnchor = false;
    mocks.liveSessionId = null;
    mocks.liveStatus = "inactive";
    mocks.currentTab = { type: "empty" };
    mocks.selectAll.mockClear();
    mocks.smartCurrentTimeMs = undefined;
    mocks.timelineSelectionAnchorId = null;
    mocks.timelineSelectionSelectedIds = [];
    mocks.timelineSessionsTable = {};
    mocks.virtualRange = null;
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("does not render sidebar action tabs inside the timeline chrome", () => {
    render(<TimelineView topChromeInset />);

    expect(screen.queryByRole("button", { name: "New note" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Search" })).toBeNull();
    expect(getSidebarActionTabsOrNull()).toBeNull();
  });

  it("keeps the native timeline scrollbar available", () => {
    const { container } = render(<TimelineView />);
    const scroller = container.querySelector("[data-sidebar-timeline-scroll]");

    expect(scroller?.className).toContain("overflow-y-auto");
    expect(scroller?.className).not.toContain("scrollbar-hide");
  });

  it("keeps a 3,500-session timeline bounded to the virtual window", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.smartCurrentTimeMs = Date.now();
    mocks.isAnchorVisible = false;
    mocks.virtualRange = { startIndex: 0, endIndex: 20 };
    mocks.timelineSessionsTable = Object.fromEntries(
      Array.from({ length: 3_500 }, (_, index) => [
        `session-${index}`,
        {
          title: `Note ${index}`,
          created_at: new Date(
            Date.UTC(2024, 0, 14, 12, 0) - index * 60_000,
          ).toISOString(),
        },
      ]),
    );

    const { container, rerender } = render(<TimelineView />);
    const renderedRows = () =>
      container.querySelectorAll("[data-sidebar-timeline-virtual-row]");

    expect(renderedRows().length).toBeLessThanOrEqual(23);
    expect(
      container
        .querySelector("[data-sidebar-timeline-virtual-canvas]")
        ?.getAttribute("style"),
    ).toContain("height:");
    expect(screen.queryByTestId("timeline-item-session-3499")).toBeNull();

    mocks.virtualRange = { startIndex: 1_500, endIndex: 1_520 };
    rerender(<TimelineView topChipsOverlapHeader />);

    expect(renderedRows().length).toBeLessThanOrEqual(23);
    expect(container.querySelector("[data-index='1500']")).toBeTruthy();
  });

  it("keeps the first bucket below the sidebar action chrome", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.smartCurrentTimeMs = Date.now();
    mocks.timelineSessionsTable = {
      past: {
        title: "Demo Session Kickoff",
        created_at: "2024-01-01T12:00:00.000Z",
      },
    };

    const { container } = render(<TimelineView topChromeInset />);

    expect(
      container.querySelector("[data-sidebar-timeline-top-spacer]")?.className,
    ).toContain("h-12");
    expect(
      container.querySelector("[data-sidebar-timeline-bucket-header]")
        ?.className,
    ).toContain("top-12");
    expect(queryTopOccluder(container)?.className).toContain("h-12");
  });

  it("pins bucket headers to the sidebar chrome while scrolled", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.smartCurrentTimeMs = Date.now();
    mocks.timelineSessionsTable = {
      tomorrow: {
        title: "Founder sync",
        created_at: "2024-01-16T10:00:00.000Z",
      },
      today: {
        title: "Design sync",
        created_at: "2024-01-15T17:30:00.000Z",
      },
    };

    const { container } = render(<TimelineView topChromeInset />);
    const scroller = container.querySelector("[data-sidebar-timeline-scroll]");
    const header = container.querySelector(
      "[data-sidebar-timeline-bucket-header]",
    );

    expect(scroller).toBeInstanceOf(HTMLDivElement);
    expect(header?.className).toContain("top-12");

    Object.defineProperty(scroller, "clientHeight", {
      configurable: true,
      value: 200,
    });
    Object.defineProperty(scroller, "scrollHeight", {
      configurable: true,
      value: 1200,
    });
    scroller!.scrollTop = 120;
    fireEvent.scroll(scroller!);

    expect(header?.className).toContain("top-12");
    expect(header?.className).toContain("z-20");
    expect(header?.className).toContain("bg-background");
    expect(header?.className).not.toContain("backdrop-blur");
    expect(container.querySelector("[class*='backdrop-blur']")).toBeNull();
    expect(queryTopOccluder(container)?.className).toContain("z-10");
  });

  it("selects all visible notes with Cmd+A after a sidebar note selection", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T09:00:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.currentTab = { type: "sessions", id: "selected-note" };
    mocks.timelineSelectionAnchorId = "session-selected-note";
    mocks.timelineSessionsTable = {
      "selected-note": {
        title: "Selected note",
        created_at: "2024-01-15T12:00:00.000Z",
      },
      "other-note": {
        title: "Other note",
        created_at: "2024-01-15T11:00:00.000Z",
      },
    };

    render(<TimelineView />);

    fireEvent.keyDown(window, { key: "a", metaKey: true });

    expect(mocks.selectAll).toHaveBeenCalledWith([
      "session-selected-note",
      "session-other-note",
    ]);
  });

  it("does not select sidebar notes while the mounted timeline is hidden", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T09:00:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.currentTab = { type: "sessions", id: "selected-note" };
    mocks.timelineSelectionAnchorId = "session-selected-note";
    mocks.timelineSessionsTable = {
      "selected-note": {
        title: "Selected note",
        created_at: "2024-01-15T12:00:00.000Z",
      },
      "other-note": {
        title: "Other note",
        created_at: "2024-01-15T11:00:00.000Z",
      },
    };

    render(
      <div aria-hidden inert>
        <TimelineView />
      </div>,
    );

    fireEvent.keyDown(window, { key: "a", metaKey: true });

    expect(mocks.selectAll).not.toHaveBeenCalled();
  });

  it("does not select sidebar notes when Cmd+A starts in the editor", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T09:00:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.currentTab = { type: "sessions", id: "selected-note" };
    mocks.timelineSelectionAnchorId = "session-selected-note";
    mocks.timelineSessionsTable = {
      "selected-note": {
        title: "Selected note",
        created_at: "2024-01-15T12:00:00.000Z",
      },
      "other-note": {
        title: "Other note",
        created_at: "2024-01-15T11:00:00.000Z",
      },
    };

    render(<TimelineView />);

    const editor = document.createElement("div");
    editor.className = "ProseMirror";
    editor.contentEditable = "true";
    editor.tabIndex = 0;
    document.body.appendChild(editor);
    editor.focus();

    fireEvent.keyDown(editor, { key: "a", metaKey: true });

    expect(mocks.selectAll).not.toHaveBeenCalled();

    editor.remove();
  });

  it("does not show a top chrome fade while scrolled without timeline action tabs", () => {
    const { container } = render(<TimelineView topChromeInset />);
    const scroller = container.querySelector("[data-sidebar-timeline-scroll]");

    expect(scroller).toBeInstanceOf(HTMLDivElement);
    expect(getSidebarActionTabsOrNull()).toBeNull();
    expect(queryTopFade(container)).toBeNull();
    expect(queryTopOccluder(container)?.className).toContain("h-12");

    Object.defineProperty(scroller, "clientHeight", {
      configurable: true,
      value: 200,
    });
    Object.defineProperty(scroller, "scrollHeight", {
      configurable: true,
      value: 1200,
    });
    scroller!.scrollTop = 120;
    fireEvent.scroll(scroller!);

    expect(getSidebarActionTabsOrNull()).toBeNull();
    expect(queryTopFade(container)).toBeNull();
    expect(queryTopOccluder(container)?.className).toContain("h-12");
  });

  it("does not show a top scroll fade when there are no hidden future notes", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.timelineSessionsTable = {
      today: {
        title: "Design sync",
        created_at: "2024-01-15T11:00:00.000Z",
      },
    };

    const { container } = render(<TimelineView topChromeInset />);
    const scroller = container.querySelector(
      "[data-sidebar-timeline-scroll]",
    ) as HTMLDivElement | null;

    expect(scroller).toBeInstanceOf(HTMLDivElement);

    Object.defineProperty(scroller, "clientHeight", {
      configurable: true,
      value: 200,
    });
    Object.defineProperty(scroller, "scrollHeight", {
      configurable: true,
      value: 1200,
    });
    scroller!.scrollTop = 120;
    fireEvent.scroll(scroller!);

    expect(scroller!.style.maskImage).toBe("");
    expect(queryBottomFade(container)).toBeTruthy();
  });

  it("does not show a top scroll fade when future notes are hidden above a sticky header", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.timelineSessionsTable = {
      today: {
        title: "Design sync",
        created_at: "2024-01-15T11:00:00.000Z",
      },
      later: {
        title: "Quarterly planning",
        created_at: "2024-01-17T12:00:00.000Z",
      },
    };

    const { container } = render(<TimelineView topChromeInset />);
    const scroller = container.querySelector(
      "[data-sidebar-timeline-scroll]",
    ) as HTMLDivElement | null;

    expect(scroller).toBeInstanceOf(HTMLDivElement);

    Object.defineProperty(scroller, "clientHeight", {
      configurable: true,
      value: 200,
    });
    Object.defineProperty(scroller, "scrollHeight", {
      configurable: true,
      value: 1200,
    });
    scroller!.scrollTop = 120;
    fireEvent.scroll(scroller!);

    expect(screen.getByText("Today")).toBeTruthy();
    expect(queryTopFade(container)).toBeNull();
    expect(queryTopOccluder(container)?.className).toContain("h-12");
    expect(scroller!.style.maskImage).toBe("");
    expect(queryBottomFade(container)).toBeTruthy();
  });

  it("drops the bottom scroll fade at the bottom edge", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.timelineSessionsTable = {
      today: {
        title: "Design sync",
        created_at: "2024-01-15T11:00:00.000Z",
      },
      later: {
        title: "Quarterly planning",
        created_at: "2024-01-17T12:00:00.000Z",
      },
    };

    const { container } = render(<TimelineView topChromeInset />);
    const scroller = container.querySelector(
      "[data-sidebar-timeline-scroll]",
    ) as HTMLDivElement | null;

    expect(scroller).toBeInstanceOf(HTMLDivElement);

    Object.defineProperty(scroller, "clientHeight", {
      configurable: true,
      value: 200,
    });
    Object.defineProperty(scroller, "scrollHeight", {
      configurable: true,
      value: 1200,
    });
    scroller!.scrollTop = 1000;
    fireEvent.scroll(scroller!);

    expect(scroller!.style.maskImage).toBe("");
    expect(queryBottomFade(container)).toBeNull();
  });

  it("keeps the bottom now chip above the scroll fade", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.isAnchorVisible = false;
    mocks.isScrolledPastAnchor = false;
    mocks.timelineSessionsTable = {
      later: {
        title: "Design sync",
        created_at: "2024-01-16T11:00:00.000Z",
      },
    };

    const { container } = render(<TimelineView />);
    const scroller = container.querySelector(
      "[data-sidebar-timeline-scroll]",
    ) as HTMLDivElement;
    Object.defineProperty(scroller, "clientHeight", {
      configurable: true,
      value: 20,
    });
    Object.defineProperty(scroller, "scrollHeight", {
      configurable: true,
      value: 1200,
    });
    scroller.scrollTop = 0;
    fireEvent.scroll(scroller);
    const nowChip = screen.getByRole("button", { name: "Go back to now" });

    expect(queryBottomFade(container)?.className).toContain("z-30");
    expect(nowChip.className).toContain("z-40");
  });

  it("shows an imminent meeting chip over the sidebar timeline", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.isAnchorVisible = false;
    mocks.isScrolledPastAnchor = true;
    mocks.smartCurrentTimeMs = Date.now();
    mocks.timelineSessionsTable = {
      standup: sessionRow({
        title: "Team standup",
        started_at: "2024-01-15T12:00:51.000Z",
      }),
    };

    const { container } = render(<TimelineView topChromeInset />);
    const scroller = container.querySelector("[data-sidebar-timeline-scroll]");
    const row = screen.getByTestId("timeline-item-standup");
    const chip = container.querySelector(
      "[data-sidebar-upcoming-meeting-status]",
    ) as HTMLElement | null;

    Object.defineProperty(scroller, "clientHeight", {
      configurable: true,
      value: 400,
    });
    scroller!.scrollTop = 0;
    scroller!.scrollTo = vi.fn();
    vi.spyOn(scroller!, "getBoundingClientRect").mockReturnValue({
      bottom: 400,
      height: 400,
      left: 0,
      right: 240,
      toJSON: () => ({}),
      top: 0,
      width: 240,
      x: 0,
      y: 0,
    });
    vi.spyOn(row, "getBoundingClientRect").mockReturnValue({
      bottom: 832,
      height: 32,
      left: 0,
      right: 240,
      toJSON: () => ({}),
      top: 800,
      width: 240,
      x: 0,
      y: 800,
    });

    expect(chip?.textContent).toBe("In 51s");
    expect(chip?.className).toContain("bg-destructive");
    expect(chip?.className).toContain("w-28");
    expect(chip?.querySelector("svg")).toBeTruthy();
    expect(chip?.getAttribute("aria-label")).toBe("Team standup in 51s");
    expect(screen.getByTestId("timeline-item-standup").dataset.upcoming).toBe(
      "true",
    );
    expect(
      screen.getByTestId("timeline-item-standup").dataset.upcomingProgress,
    ).toBe("0.17");
    expect(
      container.querySelector("[data-sidebar-timeline-top-spacer]")?.className,
    ).toContain("h-12");
    expect(
      container.querySelector("[data-sidebar-timeline-top-chip-stack]")
        ?.className,
    ).toContain("top-4");
    expect(screen.queryByText("Now")).toBeNull();

    fireEvent.click(chip!);

    expect(mocks.virtualScrollToIndex).toHaveBeenCalledWith(
      expect.any(Number),
      { align: "center", behavior: "smooth" },
    );
  });

  it("hides the imminent meeting chip when the meeting row is visible", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.smartCurrentTimeMs = Date.now();
    mocks.timelineSessionsTable = {
      standup: sessionRow({
        title: "Team standup",
        started_at: "2024-01-15T12:00:51.000Z",
      }),
    };

    const { container } = render(<TimelineView topChromeInset />);
    const scroller = container.querySelector("[data-sidebar-timeline-scroll]");
    const row = screen.getByTestId("timeline-item-standup");

    vi.spyOn(scroller!, "getBoundingClientRect").mockReturnValue({
      bottom: 400,
      height: 400,
      left: 0,
      right: 240,
      toJSON: () => ({}),
      top: 0,
      width: 240,
      x: 0,
      y: 0,
    });
    vi.spyOn(row, "getBoundingClientRect").mockReturnValue({
      bottom: 120,
      height: 32,
      left: 0,
      right: 240,
      toJSON: () => ({}),
      top: 88,
      width: 240,
      x: 0,
      y: 88,
    });

    fireEvent.scroll(scroller!);

    expect(
      container.querySelector("[data-sidebar-upcoming-meeting-status]"),
    ).toBeNull();
  });

  it("shows upcoming meeting minutes with remaining seconds", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.isAnchorVisible = false;
    mocks.smartCurrentTimeMs = Date.now();
    mocks.timelineSessionsTable = {
      standup: sessionRow({
        title: "Team standup",
        started_at: "2024-01-15T12:01:01.000Z",
      }),
    };

    const { container } = render(<TimelineView topChromeInset />);

    expect(
      container.querySelector("[data-sidebar-upcoming-meeting-status]")
        ?.textContent,
    ).toBe("In 1m 1s");
  });

  it("hides the meeting chip once the meeting starts", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.isAnchorVisible = false;
    mocks.smartCurrentTimeMs = Date.now();
    mocks.timelineSessionsTable = {
      later: sessionRow({
        title: "Roadmap review",
        started_at: "2024-01-15T12:06:00.000Z",
      }),
    };

    const { container, rerender } = render(<TimelineView topChromeInset />);

    expect(
      container.querySelector("[data-sidebar-upcoming-meeting-status]"),
    ).toBeNull();

    vi.setSystemTime(new Date("2024-01-15T12:01:00.000Z"));
    mocks.currentTimeMs = Date.now();
    fireEvent.focus(window);
    rerender(<TimelineView topChromeInset />);

    expect(
      container.querySelector("[data-sidebar-upcoming-meeting-status]")
        ?.textContent,
    ).toBe("In 5m 0s");

    vi.setSystemTime(new Date("2024-01-15T12:06:01.000Z"));
    mocks.currentTimeMs = Date.now();
    fireEvent.focus(window);
    rerender(<TimelineView topChromeInset />);

    expect(
      container.querySelector("[data-sidebar-upcoming-meeting-status]"),
    ).toBeNull();
  });

  it("overlays the top now chip without reserving sidebar space", () => {
    mocks.isAnchorVisible = false;
    mocks.isScrolledPastAnchor = true;
    mocks.timelineSessionsTable = {
      past: {
        title: "Design sync",
        created_at: "2024-01-14T12:00:00.000Z",
      },
    };

    const { container } = render(<TimelineView topChromeInset />);
    const scroller = container.querySelector("[data-sidebar-timeline-scroll]");

    expect(scroller).toBeInstanceOf(HTMLDivElement);

    const nowButton = screen.getByRole("button", { name: "Go back to now" });
    expect(nowButton.className).toContain("bg-card");
    expect(nowButton.className).not.toContain("backdrop-blur");
    expect(
      container.querySelector("[data-sidebar-timeline-top-chip-stack]")
        ?.className,
    ).toContain("top-4");
    expect(
      container.querySelector("[data-sidebar-timeline-top-spacer]")?.className,
    ).toContain("h-12");
    expect(
      container.querySelector("[data-sidebar-timeline-bucket-header]")
        ?.className,
    ).toContain("top-12");

    Object.defineProperty(scroller, "clientHeight", {
      configurable: true,
      value: 200,
    });
    Object.defineProperty(scroller, "scrollHeight", {
      configurable: true,
      value: 1200,
    });
    scroller!.scrollTop = 120;
    fireEvent.scroll(scroller!);

    expect(queryTopFade(container)).toBeNull();
  });

  it("places the fallback now indicator between future and past buckets", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T15:54:00.000Z"));

    mocks.configValue = "Asia/Seoul";
    mocks.timelineSessionsTable = {
      tomorrow: {
        title: "Sprint retro & planning",
        created_at: "2024-01-17T08:30:00.000Z",
      },
      yesterday: {
        title: "Design sync",
        created_at: "2024-01-15T12:00:00.000Z",
      },
      "two-days-ago": {
        title: "Product Discovery Pace",
        created_at: "2024-01-14T12:00:00.000Z",
      },
    };

    render(<TimelineView />);

    const tomorrowHeading = screen.getByText("Tomorrow");
    const yesterdayHeading = screen.getByText("Yesterday");
    const twoDaysAgoHeading = screen.getByText("2 days ago");
    const indicator = screen.getByTestId("current-time-indicator");

    expect(isBefore(tomorrowHeading, indicator)).toBe(true);
    expect(isBefore(indicator, yesterdayHeading)).toBe(true);
    expect(isBefore(indicator, twoDaysAgoHeading)).toBe(true);
    expect(
      indicator.closest("[data-sidebar-current-time-header-gap]")?.className,
    ).toContain("py-3");
  });

  it("does not auto-scroll to the fallback now indicator without a today bucket", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T15:54:00.000Z"));

    mocks.configValue = "UTC";
    mocks.anchorNode = document.createElement("div");
    mocks.timelineSessionsTable = {
      tomorrow: {
        title: "Roadmap review",
        created_at: "2024-01-16T12:00:00.000Z",
      },
      yesterday: {
        title: "Design sync",
        created_at: "2024-01-14T12:00:00.000Z",
      },
    };

    render(<TimelineView topChromeInset />);

    expect(screen.getByTestId("current-time-indicator")).toBeTruthy();
    expect(mocks.virtualScrollToIndex).not.toHaveBeenCalled();
  });

  it("auto-scrolls to the current-time anchor when a today bucket exists", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T15:54:00.000Z"));

    mocks.configValue = "UTC";
    mocks.isAnchorVisible = false;
    const anchorNode = document.createElement("div");
    mocks.anchorNode = anchorNode;
    mocks.timelineSessionsTable = {
      today: {
        title: "Design sync",
        created_at: "2024-01-15T12:00:00.000Z",
      },
    };

    render(<TimelineView topChromeInset />);
    vi.runOnlyPendingTimers();

    expect(screen.getByText("Today")).toBeTruthy();
    expect(mocks.virtualScrollToIndex).toHaveBeenCalledWith(
      expect.any(Number),
      { align: "center", behavior: "auto" },
    );
  });

  it("hides the now indicator while an active meeting is visible", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T11:15:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.smartCurrentTimeMs = Date.now();
    mocks.liveStatus = "active";
    mocks.liveSessionId = "session-live";
    mocks.timelineSessionsTable = {
      "session-live": {
        title: "kate <> john (char)",
        created_at: "2024-01-15T11:00:00.000Z",
      },
    };

    const { container } = render(<TimelineView />);

    expect(screen.getByText("Today")).toBeTruthy();
    expect(screen.getByTestId("timeline-item-session-live")).toBeTruthy();
    expect(screen.queryByTestId("current-time-indicator")).toBeNull();
    const anchor = container.querySelector(
      "[data-sidebar-current-time-anchor]",
    );
    expect(anchor).toBeTruthy();
  });

  it("hides the now indicator while a finalizing meeting is visible", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T11:15:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.smartCurrentTimeMs = Date.now();
    mocks.liveStatus = "finalizing";
    mocks.liveSessionId = "session-finalizing";
    mocks.timelineSessionsTable = {
      "session-finalizing": {
        title: "kate <> john (char)",
        created_at: "2024-01-15T11:00:00.000Z",
      },
    };

    const { container } = render(<TimelineView />);

    expect(screen.getByText("Today")).toBeTruthy();
    expect(screen.getByTestId("timeline-item-session-finalizing")).toBeTruthy();
    expect(screen.queryByTestId("current-time-indicator")).toBeNull();
    const anchor = container.querySelector(
      "[data-sidebar-current-time-anchor]",
    );
    expect(anchor).toBeTruthy();
  });

  it("places the fallback now indicator with fresh time after data refreshes", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T23:58:00.000Z"));
    mocks.configValue = "UTC";
    mocks.currentTimeMs = Date.now();

    const { rerender } = render(<TimelineView />);

    vi.setSystemTime(new Date("2024-01-16T00:01:00.000Z"));
    mocks.timelineSessionsTable = {
      tomorrow: {
        title: "Roadmap review",
        created_at: "2024-01-17T12:00:00.000Z",
      },
      yesterday: {
        title: "Late wrap",
        created_at: "2024-01-15T23:59:00.000Z",
      },
    };
    // Re-render with a changed prop so React.memo doesn't bail out on an
    // identical-props re-render — the mocked data above needs a fresh pass.
    rerender(<TimelineView topChromeInset />);

    const tomorrowHeading = screen.getByText("Tomorrow");
    const yesterdayHeading = screen.getByText("Yesterday");
    const indicator = screen.getByTestId("current-time-indicator");

    expect(isBefore(tomorrowHeading, indicator)).toBe(true);
    expect(isBefore(indicator, yesterdayHeading)).toBe(true);
  });

  it("hides the fallback now indicator once stale future buckets are past", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T23:58:00.000Z"));
    mocks.configValue = "UTC";
    mocks.currentTimeMs = Date.now();
    mocks.smartCurrentTimeMs = Date.now();
    mocks.timelineSessionsTable = {
      soon: {
        title: "Late handoff",
        created_at: "2024-01-16T00:00:30.000Z",
      },
      yesterday: {
        title: "Planning",
        created_at: "2024-01-14T12:00:00.000Z",
      },
    };

    const { container, rerender } = render(<TimelineView />);

    expect(screen.getByTestId("current-time-indicator")).toBeTruthy();

    vi.setSystemTime(new Date("2024-01-16T00:01:00.000Z"));
    mocks.currentTimeMs = Date.now();
    // Re-render with a changed prop so React.memo doesn't bail out on an
    // identical-props re-render — the fresh time above needs a new pass.
    rerender(<TimelineView topChromeInset />);

    const staleTomorrowItem = screen.getByTestId("timeline-item-soon");
    const yesterdayHeading = screen.getByText("Yesterday");
    const anchor = container.querySelector(
      "[data-sidebar-current-time-anchor]",
    );

    expect(screen.queryByTestId("current-time-indicator")).toBeNull();
    expect(anchor).toBeTruthy();
    expect(isBefore(staleTomorrowItem, anchor!)).toBe(true);
    expect(isBefore(anchor!, yesterdayHeading)).toBe(true);
  });

  it("hides the now indicator when nothing in the timeline is upcoming", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.smartCurrentTimeMs = Date.now();
    mocks.timelineSessionsTable = {
      "past-note": {
        title: "Design sync",
        created_at: "2024-01-15T09:00:00.000Z",
      },
    };

    const { container } = render(<TimelineView />);

    expect(screen.getByText("Today")).toBeTruthy();
    expect(screen.queryByTestId("current-time-indicator")).toBeNull();
    const anchor = container.querySelector(
      "[data-sidebar-current-time-anchor]",
    );
    expect(anchor).toBeTruthy();
  });

  it("shows the now indicator between future and past items today", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.currentTimeMs = Date.now();
    mocks.smartCurrentTimeMs = Date.now();
    mocks.timelineSessionsTable = {
      "future-note": {
        title: "Roadmap review",
        created_at: "2024-01-15T15:00:00.000Z",
      },
      "past-note": {
        title: "Design sync",
        created_at: "2024-01-15T09:00:00.000Z",
      },
    };

    render(<TimelineView />);

    const futureItem = screen.getByTestId("timeline-item-future-note");
    const pastItem = screen.getByTestId("timeline-item-past-note");
    const indicator = screen.getByTestId("current-time-indicator");

    expect(isBefore(futureItem, indicator)).toBe(true);
    expect(isBefore(indicator, pastItem)).toBe(true);
  });

  describe("tag grouping", () => {
    beforeEach(() => {
      mocks.configValues = {
        sidebar_group_by: "tag",
        sidebar_expanded_tags: [],
      };
      mocks.timelineSessionsTable = {
        "work-note": {
          title: "Weekly sync",
          created_at: "2024-01-15T09:00:00.000Z",
          tags: ["work"],
        },
        "both-note": {
          title: "Retro",
          created_at: "2024-01-14T09:00:00.000Z",
          tags: ["work", "personal"],
        },
        "loose-note": {
          title: "Scratch",
          created_at: "2024-01-13T09:00:00.000Z",
        },
      };
    });

    it("renders tag headers with counts but no rows while collapsed by default", () => {
      render(<TimelineView />);

      expect(screen.getByText("work")).toBeTruthy();
      expect(screen.getByText("personal")).toBeTruthy();
      expect(screen.getByText("Untagged")).toBeTruthy();
      expect(screen.getByText("(2)")).toBeTruthy();
      expect(screen.queryByTestId("timeline-item-work-note")).toBeNull();
      expect(screen.queryByTestId("timeline-item-both-note")).toBeNull();
      expect(screen.queryByTestId("timeline-item-loose-note")).toBeNull();
    });

    it("shows only an expanded tag's rows and persists a header toggle", () => {
      mocks.configValues.sidebar_expanded_tags = ["personal"];

      render(<TimelineView />);

      expect(screen.getByTestId("timeline-item-both-note")).toBeTruthy();
      expect(screen.queryByTestId("timeline-item-work-note")).toBeNull();
      expect(screen.queryByTestId("timeline-item-loose-note")).toBeNull();

      fireEvent.click(screen.getByText("work").closest("button")!);

      expect(mocks.setSettingValue).toHaveBeenCalledWith(
        "sidebar_expanded_tags",
        JSON.stringify(["personal", "work"]),
      );
    });

    it("expands the Untagged bucket like any tag", () => {
      mocks.configValues.sidebar_expanded_tags = ["Untagged"];

      render(<TimelineView />);

      expect(screen.getByTestId("timeline-item-loose-note")).toBeTruthy();
      expect(screen.queryByTestId("timeline-item-work-note")).toBeNull();
    });

    it("expands all tags from the collapse-all control and collapses them back", () => {
      render(<TimelineView />);

      fireEvent.click(screen.getByLabelText("Expand all"));

      expect(mocks.setSettingValue).toHaveBeenCalledWith(
        "sidebar_expanded_tags",
        JSON.stringify(["personal", "work", "Untagged"]),
      );

      cleanup();
      mocks.configValues.sidebar_expanded_tags = [
        "personal",
        "work",
        "Untagged",
      ];
      render(<TimelineView />);

      fireEvent.click(screen.getByLabelText("Collapse all"));

      expect(mocks.setSettingValue).toHaveBeenLastCalledWith(
        "sidebar_expanded_tags",
        JSON.stringify([]),
      );
    });

    describe("slash-tag nesting", () => {
      beforeEach(() => {
        mocks.timelineSessionsTable = {
          "interview-note": {
            title: "Interview",
            created_at: "2024-01-15T09:00:00.000Z",
            tags: ["dataroots/interviews"],
          },
          "allhands-note": {
            title: "All-hands",
            created_at: "2024-01-14T09:00:00.000Z",
            tags: ["dataroots"],
          },
        };
      });

      it("hides child headers until the parent is expanded", () => {
        render(<TimelineView />);

        expect(screen.getByText("dataroots")).toBeTruthy();
        expect(screen.queryByText("interviews")).toBeNull();
        // Parent count is the deduped subtree total.
        expect(screen.getByText("(2)")).toBeTruthy();
      });

      it("expanding the parent shows its rows plus a collapsed child header", () => {
        mocks.configValues.sidebar_expanded_tags = ["dataroots"];

        render(<TimelineView />);

        expect(screen.getByTestId("timeline-item-allhands-note")).toBeTruthy();
        expect(screen.getByText("interviews")).toBeTruthy();
        expect(screen.queryByTestId("timeline-item-interview-note")).toBeNull();

        fireEvent.click(screen.getByText("interviews").closest("button")!);

        expect(mocks.setSettingValue).toHaveBeenCalledWith(
          "sidebar_expanded_tags",
          JSON.stringify(["dataroots", "dataroots/interviews"]),
        );
      });

      it("shows child rows only when parent and child are both expanded", () => {
        mocks.configValues.sidebar_expanded_tags = [
          "dataroots",
          "dataroots/interviews",
        ];

        render(<TimelineView />);

        expect(screen.getByTestId("timeline-item-interview-note")).toBeTruthy();
      });

      it("renders nothing for a child id expanded without its parent", () => {
        mocks.configValues.sidebar_expanded_tags = ["dataroots/interviews"];

        render(<TimelineView />);

        expect(screen.queryByText("interviews")).toBeNull();
        expect(screen.queryByTestId("timeline-item-interview-note")).toBeNull();
      });

      it("expand-all enumerates full paths including virtual parents", () => {
        mocks.timelineSessionsTable = {
          "roadmap-note": {
            title: "Roadmap",
            created_at: "2024-01-15T09:00:00.000Z",
            tags: ["projects/2024/roadmap"],
          },
          "loose-note": {
            title: "Scratch",
            created_at: "2024-01-13T09:00:00.000Z",
          },
        };

        render(<TimelineView />);

        fireEvent.click(screen.getByLabelText("Expand all"));

        expect(mocks.setSettingValue).toHaveBeenCalledWith(
          "sidebar_expanded_tags",
          JSON.stringify([
            "projects",
            "projects/2024",
            "projects/2024/roadmap",
            "Untagged",
          ]),
        );
      });
    });
  });
});

function getSidebarActionTabsOrNull() {
  return document.querySelector("[data-sidebar-timeline-action-tabs]");
}

function queryTopFade(container: HTMLElement) {
  return container.querySelector("[data-sidebar-timeline-top-fade]");
}

function queryTopOccluder(container: HTMLElement) {
  return container.querySelector("[data-sidebar-timeline-top-occluder]");
}

function queryBottomFade(container: HTMLElement) {
  return container.querySelector("[data-sidebar-timeline-bottom-fade]");
}

function isBefore(first: Element, second: Element) {
  return Boolean(
    first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING,
  );
}

vi.mock("~/main/use-legacy-activity", async () => {
  const { useListener } = await import("~/stt/contexts");
  const { legacyActivityFixture } =
    await import("~/main/legacy-activity.test-support");
  const source = (id = "") =>
    legacyActivityFixture(
      useListener((state) => state),
      id,
    );
  return {
    useLegacySessionActivity: source,
    useLegacyDesktopAccess: () => source(),
  };
});

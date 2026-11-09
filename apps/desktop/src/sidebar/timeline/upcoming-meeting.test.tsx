import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  timelineSessionsTable: {} as Record<string, Record<string, unknown>>,
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
      return input.reduce(
        (message, part, index) =>
          `${message}${part}${index < values.length ? String(values[index]) : ""}`,
        "",
      );
    }

    if (typeof input === "string") {
      return input;
    }

    const descriptor = input as {
      message?: string;
      values?: Record<string, unknown>;
    };

    return (descriptor.message ?? "").replace(
      /\{(\w+)\}/g,
      (_match: string, key: string) =>
        String(descriptor.values?.[key] ?? `{${key}}`),
    );
  };

  return { t };
});

vi.mock("@lingui/react/macro", () => ({
  useLingui: () => ({
    _: lingui.t,
    t: lingui.t,
  }),
}));

vi.mock("@lingui/react", () => ({
  useLingui: () => ({
    _: lingui.t,
    t: lingui.t,
  }),
}));

vi.mock("~/shared/config", () => ({
  useConfigValue: () => undefined,
}));

vi.mock("./queries", async () => {
  const { useCurrentDay } = await import("~/shared/hooks/useCurrentDay");
  const { buildTimelineBuckets, deriveTimelineWindowData } =
    await import("./utils");
  return {
    useNearestTimelineBuckets: () => {
      useCurrentDay();
      return buildTimelineBuckets({
        timelineSessionsTable: deriveTimelineWindowData({
          timelineSessionsTable: mocks.timelineSessionsTable,
        }).timelineSessionsTable,
      });
    },
  };
});

import { useSidebarUpcomingMeetingStatus } from "./upcoming-meeting";

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

describe("useSidebarUpcomingMeetingStatus", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-15T12:00:00.000Z"));
    mocks.timelineSessionsTable = {};
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("clears the meeting status once the meeting starts", () => {
    mocks.timelineSessionsTable = {
      standup: sessionRow({
        title: "Team standup",
        started_at: "2024-01-15T12:04:00.000Z",
      }),
    };

    const upcoming = renderHook(() => useSidebarUpcomingMeetingStatus());

    expect(upcoming.result.current).toMatchObject({
      itemKey: "session-standup",
      title: "Team standup",
    });

    vi.setSystemTime(new Date("2024-01-15T12:04:01.000Z"));
    act(() => {
      vi.advanceTimersByTime(1_000);
    });

    expect(upcoming.result.current).toBeNull();
  });

  it("does not rerender every second while the active status is unchanged", () => {
    mocks.timelineSessionsTable = {
      standup: sessionRow({
        title: "Team standup",
        started_at: "2024-01-15T11:55:00.000Z",
      }),
    };
    let renderCount = 0;

    renderHook(() => {
      renderCount += 1;
      return useSidebarUpcomingMeetingStatus();
    });
    const initialRenderCount = renderCount;

    act(() => {
      vi.advanceTimersByTime(5_000);
    });

    expect(renderCount).toBe(initialRenderCount);
  });

  it("refreshes the status when the window regains focus", () => {
    mocks.timelineSessionsTable = {
      standup: sessionRow({
        title: "Team standup",
        started_at: "2024-01-15T11:55:00.000Z",
      }),
    };

    const active = renderHook(() => useSidebarUpcomingMeetingStatus());
    vi.setSystemTime(new Date("2024-01-15T12:30:01.000Z"));

    act(() => window.dispatchEvent(new Event("focus")));

    expect(active.result.current).toBeNull();
  });

  it("refreshes the status when the document becomes visible", () => {
    mocks.timelineSessionsTable = {
      standup: sessionRow({
        title: "Team standup",
        started_at: "2024-01-15T11:55:00.000Z",
      }),
    };

    const active = renderHook(() => useSidebarUpcomingMeetingStatus());
    vi.setSystemTime(new Date("2024-01-15T12:30:01.000Z"));

    act(() => document.dispatchEvent(new Event("visibilitychange")));

    expect(active.result.current).toBeNull();
  });

  it("rebuilds timeline buckets after a day boundary", () => {
    vi.setSystemTime(new Date("2024-01-15T23:59:00.000Z"));
    mocks.timelineSessionsTable = {
      standup: sessionRow({
        title: "Team standup",
        started_at: "2024-01-17T00:01:00.000Z",
      }),
    };

    const upcoming = renderHook(() => useSidebarUpcomingMeetingStatus());
    expect(upcoming.result.current).toBeNull();

    vi.setSystemTime(new Date("2024-01-16T23:59:00.000Z"));
    act(() => window.dispatchEvent(new Event("focus")));

    expect(upcoming.result.current).toMatchObject({
      itemKey: "session-standup",
      label: "In 2m 0s",
      title: "Team standup",
    });
  });
});

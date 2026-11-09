import { onlineManager } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { TimelineView } from ".";
import { TimelineOwnerProvider, useTimelineOwner } from "./owner";
import { deferred, header, page, timelineFixture } from "./test-fixture";

import type {
  TimelinePage,
  TimelineQuery,
} from "~/shared/application/protocol.gen";
import { VaultHostContext } from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";
import {
  createTimelineSelectionStore,
  TimelineSelectionContext,
} from "~/store/zustand/timeline-selection";

const mocks = vi.hoisted(() => ({
  group: "date",
  expanded: [] as string[],
  tabs: {},
  range: null as { start: number; end: number } | null,
  scroll: vi.fn(),
  settings: vi.fn(),
}));
const lingui = vi.hoisted(() => ({
  t: (
    input:
      | string
      | { message?: string; values?: Record<string, unknown> }
      | TemplateStringsArray,
    ...values: (string | number)[]
  ) =>
    Array.isArray(input)
      ? input.map((part, index) => `${part}${values[index] ?? ""}`).join("")
      : typeof input === "string"
        ? input
        : "message" in input
          ? (input.message ?? "").replace(/\{(\w+)\}/g, (_match, key) =>
              String(input.values?.[key] ?? key),
            )
          : "",
}));
vi.mock("@lingui/react/macro", () => ({
  useLingui: () => ({ t: lingui.t, _: lingui.t }),
  Trans: ({
    children,
    message,
    id,
    values,
  }: {
    children?: React.ReactNode;
    message?: string;
    id?: string;
    values?: Record<string, unknown>;
  }) => children ?? lingui.t({ message: message ?? id, values }),
}));
vi.mock("@lingui/react", () => ({
  useLingui: () => ({ t: lingui.t, _: lingui.t }),
  Trans: ({
    children,
    message,
    id,
    values,
  }: {
    children?: React.ReactNode;
    message?: string;
    id?: string;
    values?: Record<string, unknown>;
  }) => children ?? lingui.t({ message: message ?? id, values }),
}));
vi.mock("~/shared/config", () => ({
  useConfigValue: (key: string) =>
    key === "timezone"
      ? "UTC"
      : key === "sidebar_group_by"
        ? mocks.group
        : key === "sidebar_expanded_tags"
          ? mocks.expanded
          : undefined,
}));
vi.mock("~/settings/queries", () => ({ setSettingValue: mocks.settings }));
vi.mock("~/ai/hooks/useEnhancingSessions", () => ({
  useEnhancingSessionIds: () => [],
}));
vi.mock("~/main/use-legacy-activity", () => ({
  useLegacySessionActivity: () => ({
    liveStatus: "inactive",
    liveSessionId: null,
  }),
}));
vi.mock("~/session/hooks/useDeleteSession", () => ({
  useSessionDeletionAccess: () => ({
    available: true,
    isAvailable: () => true,
  }),
  useDeleteSession: () => vi.fn(),
}));
vi.mock("~/shared/hooks/useNativeContextMenu", () => ({
  useNativeContextMenu: () => vi.fn(),
}));
vi.mock("~/store/zustand/tabs", () => ({
  useTabsStore: () => mocks.tabs,
  useTabs: (select: (value: unknown) => unknown) =>
    select({ currentTab: { type: "sessions", id: "note-0" } }),
}));
vi.mock("./item", async () => {
  const { useTimelineSelectionStore } =
    await import("~/store/zustand/timeline-selection");
  return {
    TimelineItemComponent: ({
      item,
    }: {
      item: { id: string; data: { title: string } };
    }) => {
      const store = useTimelineSelectionStore();
      return (
        <button
          data-testid={`note-${item.id}`}
          onClick={() => store.getState().setAnchor(`session-${item.id}`)}
        >
          {item.data.title}
        </button>
      );
    },
  };
});
vi.mock("./realtime", () => ({
  useCurrentTimeMs: () => Date.now(),
  CurrentTimeIndicator: () => <span />,
}));
vi.mock("@tanstack/react-virtual", () => ({
  defaultRangeExtractor: ({
    startIndex,
    endIndex,
  }: {
    startIndex: number;
    endIndex: number;
  }) =>
    Array.from({ length: endIndex - startIndex + 1 }, (_, i) => i + startIndex),
  useVirtualizer: (options: {
    count: number;
    scrollMargin: number;
    estimateSize: (index: number) => number;
    getItemKey: (index: number) => string;
  }) => {
    let offset = options.scrollMargin;
    const rows = Array.from({ length: options.count }, (_, index) => {
      const start = offset,
        size = options.estimateSize(index);
      offset += size;
      return {
        index,
        key: options.getItemKey(index),
        start,
        size,
        end: offset,
      };
    });
    const visible = mocks.range
      ? rows.slice(mocks.range.start, mocks.range.end + 1)
      : rows;
    return {
      options,
      getVirtualItems: () => visible,
      getTotalSize: () => offset - options.scrollMargin,
      scrollOffset: 0,
      scrollRect: { height: 500 },
      scrollToIndex: mocks.scroll,
    };
  },
}));
const scopes: VaultScope[] = [];
beforeEach(() => {
  mocks.group = "date";
  mocks.expanded = [];
  mocks.range = null;
  vi.clearAllMocks();
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
});
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  onlineManager.setOnline(true);
  vi.unstubAllGlobals();
});
async function mount(f = timelineFixture()) {
  scopes.push(f.scope);
  await f.start();
  const selection = createTimelineSelectionStore();
  let owner!: ReturnType<typeof useTimelineOwner>;
  function Probe() {
    owner = useTimelineOwner();
    const [visible, setVisible] = useState(true);
    return (
      <>
        <button onClick={() => setVisible((value) => !value)}>
          Toggle sidebar
        </button>
        {visible && <TimelineView />}
      </>
    );
  }
  const ui = render(
    <VaultHostContext.Provider
      value={{ scope: f.scope, error: null, retry: () => {} }}
    >
      <TimelineSelectionContext.Provider value={selection}>
        <TimelineOwnerProvider>
          <Probe />
        </TimelineOwnerProvider>
      </TimelineSelectionContext.Provider>
    </VaultHostContext.Provider>,
  );
  return { ...f, ...ui, selection, getOwner: () => owner };
}

it("keeps a 20,000-note vault bounded, advertises counts, pages on intent and selects only loaded rows", async () => {
  const f = timelineFixture();
  f.client.timelineHeaders.mockImplementation(async (input: TimelineQuery) =>
    input.view.kind === "date"
      ? page(
          Array.from({ length: 200 }, (_, i) =>
            header(`note-${Number(input.cursor ?? 0) + i}`, Date.now() - i - 1),
          ),
          { total_count: 20000, next_cursor: input.cursor ? null : "200" },
        )
      : page(),
  );
  const view = await mount(f);
  await screen.findByText("200 of 20000 loaded");
  expect(screen.queryByText("0 of 0 loaded")).toBeNull();
  expect(f.client.timelineHeaders).toHaveBeenCalledTimes(2);
  expect(screen.getAllByTestId(/^note-note-/)).toHaveLength(200);
  fireEvent.click(screen.getByTestId("note-note-0"));
  fireEvent.keyDown(window, { key: "a", metaKey: true });
  expect(view.selection.getState().selectedIds).toHaveLength(200);
  expect(screen.getByText("200 loaded notes selected")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Load earlier" }));
  await screen.findByText("400 of 20000 loaded");
  expect(view.selection.getState().selectedIds).toHaveLength(200);
  expect(f.client.timelineHeaders).toHaveBeenCalledTimes(3);
  expect(f.client.timelineHeaders.mock.calls[2][0]).toMatchObject({
    cursor: "200",
    expected_revision: "r1",
  });
});
it("shows later and handwritten dates explicitly without reading hidden pages", async () => {
  const f = timelineFixture();
  f.client.timelineHeaders.mockImplementation(async (input) =>
    input.view.kind === "date"
      ? page([header("past", Date.now() - 1)], {
          future_count: 450,
          undated_count: 1,
        })
      : input.view.kind === "upcoming"
        ? page(
            [
              header(
                input.view.before_ms === null ? "later" : "near",
                Date.now() + 1000,
              ),
            ],
            {
              total_count: input.view.before_ms === null ? 450 : 1,
              undated_count: 1,
            },
          )
        : page([header("handwritten", null)]),
  );
  await mount(f);
  await screen.findByRole("button", { name: "Show later dates (449)" });
  expect(f.client.timelineHeaders).toHaveBeenCalledTimes(2);
  expect(screen.queryByTestId("note-handwritten")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /Undated/ }));
  await screen.findByTestId("note-handwritten");
  expect(f.client.timelineHeaders).toHaveBeenCalledTimes(3);
  fireEvent.click(
    screen.getByRole("button", { name: "Show later dates (449)" }),
  );
  await screen.findByTestId("note-later");
  expect(f.client.timelineHeaders.mock.calls[3][0].view).toMatchObject({
    kind: "upcoming",
    before_ms: null,
  });
});
it("loads only visible expanded direct tag buckets, with complete counts and no synthetic/real Untagged collision", async () => {
  mocks.group = "tag";
  mocks.expanded = ["root", "root/child", "Untagged"];
  const f = timelineFixture();
  f.client.timelineFacets.mockResolvedValue({
    revision: "r1",
    nodes: [
      {
        path: "root",
        parent_path: null,
        label: "root",
        direct_count: 0,
        subtree_count: 1200,
      },
      {
        path: "root/child",
        parent_path: "root",
        label: "child",
        direct_count: 1200,
        subtree_count: 1200,
      },
      {
        path: "Untagged",
        parent_path: null,
        label: "Untagged",
        direct_count: 2,
        subtree_count: 2,
      },
    ],
    untagged_count: 400,
  });
  f.client.timelineHeaders.mockImplementation(async (input) =>
    page(
      [header(input.view.kind === "tag" ? input.view.path : "truly-untagged")],
      {
        total_count:
          input.view.kind === "tag" && input.view.path === "root/child"
            ? 1200
            : 400,
      },
    ),
  );
  await mount(f);
  await screen.findByTestId("note-root/child");
  expect(screen.getAllByText("(1200)")).toHaveLength(2);
  expect(
    f.client.timelineHeaders.mock.calls.map(([input]) => input.view),
  ).not.toContainEqual({ kind: "tag", path: "root" });
  expect(f.client.timelineHeaders).toHaveBeenCalledTimes(3);
  expect(screen.getByTestId("note-Untagged")).toBeTruthy();
  expect(screen.getByTestId("note-truly-untagged")).toBeTruthy();
});
it("keeps explicit retries on disconnect and refreshes cursors only after revision changes", async () => {
  const f = timelineFixture();
  let failed = true;
  f.client.timelineHeaders.mockImplementation(async (input) => {
    if (input.view.kind === "date" && failed)
      throw new Error("Network interrupted");
    return page([header("note-0", Date.now() - 1)]);
  });
  await mount(f);
  await screen.findByRole("alert");
  expect(screen.getByText("Network interrupted")).toBeTruthy();
  failed = false;
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await screen.findByTestId("note-note-0");
  const before = f.client.timelineHeaders.mock.calls.length;
  act(() =>
    f.emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "session_headers",
        ids: ["note-0"],
      },
    }),
  );
  await waitFor(() =>
    expect(f.client.timelineHeaders.mock.calls.length).toBe(before + 1),
  );
  expect(
    screen.queryByText(
      "The timeline changed. Its loaded window was refreshed.",
    ),
  ).toBeNull();
});
it("restores its loaded per-owner viewport on sidebar remount and reports a missing anchor after a revision reset", async () => {
  const f = timelineFixture();
  f.client.timelineHeaders.mockImplementation(async (input) =>
    input.view.kind === "date"
      ? page([header("note-0", Date.now() - 1)])
      : page(),
  );
  const view = await mount(f);
  await screen.findByTestId("note-note-0");
  const owner = view.getOwner();
  act(() =>
    owner.saveViewport("date", {
      sessionId: "note-0",
      bucketId: "Today",
      offset: 18,
      revision: "r1",
    }),
  );
  const reads = f.client.timelineHeaders.mock.calls.length;
  fireEvent.click(screen.getByRole("button", { name: "Toggle sidebar" }));
  fireEvent.click(screen.getByRole("button", { name: "Toggle sidebar" }));
  await screen.findByTestId("note-note-0");
  expect(mocks.scroll).toHaveBeenCalledWith(expect.any(Number), {
    align: "start",
    behavior: "auto",
  });
  expect(f.client.timelineHeaders).toHaveBeenCalledTimes(reads);
  act(() =>
    owner.saveViewport("date", {
      sessionId: "missing",
      bucketId: "Today",
      offset: 18,
      revision: "r1",
    }),
  );
  f.client.timelineHeaders.mockResolvedValue(page([], { revision: "r2" }));
  await act(() => owner.probe());
  await screen.findByText(
    "The previous note is outside the refreshed window. Scroll position was reset.",
  );
});
it("does not automatically retry a lost pending page into a replacement scope", async () => {
  const f = timelineFixture();
  const pending = deferred<TimelinePage>();
  f.client.timelineHeaders.mockReturnValue(pending.promise);
  const view = await mount(f);
  await waitFor(() =>
    expect(f.client.timelineHeaders).toHaveBeenCalledTimes(2),
  );
  view.unmount();
  pending.resolve(page([header("private-old")]));
  await act(async () => {});
  expect(f.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
});

it("fails closed on unsupported servers, and permits reader timeline reads without writes", async () => {
  const unsupported = timelineFixture();
  unsupported.client.features = [];
  const view = await mount(unsupported);
  await screen.findByText("This server does not support the timeline.");
  expect(unsupported.client.timelineHeaders).not.toHaveBeenCalled();
  expect(unsupported.client.timelineFacets).not.toHaveBeenCalled();
  view.unmount();
  const reader = timelineFixture("online");
  reader.client.canEdit = false;
  reader.client.timelineHeaders.mockResolvedValue(
    page([header("read-only", Date.now() - 1)]),
  );
  await mount(reader);
  await screen.findByTestId("note-read-only");
  expect(reader.client.timelineHeaders).toHaveBeenCalledTimes(2);
});

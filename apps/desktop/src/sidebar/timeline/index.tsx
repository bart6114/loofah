import { Trans, useLingui } from "@lingui/react/macro";
import {
  defaultRangeExtractor,
  type Range,
  useVirtualizer,
} from "@tanstack/react-virtual";
import {
  ArrowDownIcon,
  ArrowUpIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  ChevronsDownUpIcon,
  ChevronsUpDownIcon,
  SunIcon,
} from "lucide-react";
import {
  type ReactNode,
  memo,
  type WheelEvent as ReactWheelEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useLayoutEffect,
} from "react";

import { Button } from "@hypr/ui/components/ui/button";
import { cn } from "@hypr/utils";

import { UNDATED_ID, UNTAGGED_ID } from "./controller";
import { TimelineItemComponent } from "./item";
import { usePagedTimeline } from "./queries";
import { CurrentTimeIndicator, useCurrentTimeMs } from "./realtime";
import {
  buildTimelineRows,
  getTimelineRowHeight,
  type TimelineRow,
} from "./rows";
import { TimelineStreamControl } from "./stream-control";
import {
  useUpcomingMeetingStatus,
  useUpcomingMeetingLabelFormatter,
} from "./upcoming-meeting";
import {
  getItemTimestamp,
  hasFutureTimelineItems,
  type TimelineBucket,
} from "./utils";

import { useEnhancingSessionIds } from "~/ai/hooks/useEnhancingSessions";
import { useLegacySessionActivity } from "~/main/use-legacy-activity";
import { useDeleteSession } from "~/session/hooks/useDeleteSession";
import { setSettingValue } from "~/settings/queries";
import { useConfigValue } from "~/shared/config";
import { scrollElementByWheel } from "~/shared/dom/scroll-wheel";
import { useMountEffect } from "~/shared/hooks/useMountEffect";
import { useNativeContextMenu } from "~/shared/hooks/useNativeContextMenu";
import { useTabs } from "~/store/zustand/tabs";
import { useTimelineSelection } from "~/store/zustand/timeline-selection";

const EMPTY_BUCKETS: TimelineBucket[] = [];

export const TimelineView = memo(function TimelineView({
  topChipsOverlapHeader = false,
  topChromeInset = false,
}: {
  topChipsOverlapHeader?: boolean;
  topChromeInset?: boolean;
} = {}) {
  const { t } = useLingui();
  const timezone = useConfigValue("timezone") || undefined;
  const groupBy: "date" | "tag" =
    useConfigValue("sidebar_group_by") === "tag" ? "tag" : "date";
  const timeline = usePagedTimeline();
  const {
    owner,
    state,
    available,
    supported,
    buckets,
    table: timelineSessionsTable,
    laterCount,
  } = timeline;
  const expandedTagSet = useMemo(
    () =>
      new Set([...state.expanded, ...(state.undatedOpen ? [UNDATED_ID] : [])]),
    [state.expanded, state.undatedOpen],
  );
  const hasMoreFutureItems = laterCount > 0;
  const bucketItemCounts = useMemo(
    () =>
      new Map(
        buckets.map((bucket) => [
          bucket.id,
          bucket.totalCount ?? bucket.items.length,
        ]),
      ),
    [buckets],
  );
  const persistExpanded = useCallback(
    (next: string[]) => {
      owner.setExpanded(next);
      const paths = next.flatMap((id) => {
        if (id === UNTAGGED_ID) return ["Untagged"];
        try {
          const value: unknown = JSON.parse(id);
          return Array.isArray(value) &&
            value[0] === "tag" &&
            typeof value[1] === "string"
            ? [value[1]]
            : [];
        } catch {
          return [];
        }
      });
      void setSettingValue(
        "sidebar_expanded_tags",
        JSON.stringify([...new Set(paths)]),
      );
    },
    [owner],
  );
  const toggleTagExpanded = useCallback(
    (id: string) => {
      if (id === UNDATED_ID) {
        owner.toggleUndated();
        return;
      }
      persistExpanded(
        state.expanded.includes(id)
          ? state.expanded.filter((value) => value !== id)
          : [...state.expanded, id],
      );
    },
    [owner, state.expanded, persistExpanded],
  );
  const anyTagExpanded =
    groupBy === "tag" &&
    buckets.some((bucket) => expandedTagSet.has(bucket.id));
  const toggleAllTagsExpanded = useCallback(() => {
    const all = [
      ...(state.facets?.nodes.map((node) =>
        JSON.stringify(["tag", node.path]),
      ) ?? []),
      UNTAGGED_ID,
    ];
    persistExpanded(anyTagExpanded ? [] : all);
  }, [state.facets, anyTagExpanded, persistExpanded]);
  const hasToday = useMemo(
    () =>
      groupBy === "date" && buckets.some((bucket) => bucket.label === "Today"),
    [buckets, groupBy],
  );
  const indicatorTimeMs = useCurrentTimeMs();
  const formatUpcomingMeetingLabel = useUpcomingMeetingLabelFormatter();
  const upcomingMeetingStatus = useUpcomingMeetingStatus(
    groupBy === "date" ? buckets : EMPTY_BUCKETS,
    formatUpcomingMeetingLabel,
  );
  const legacy = useLegacySessionActivity("");
  const activeSessionId =
    legacy.liveStatus === "active" || legacy.liveStatus === "finalizing"
      ? legacy.liveSessionId
      : null;
  const enhancingSessionIds = useEnhancingSessionIds();
  const enhancingSessionIdSet = useMemo(
    () => new Set(enhancingSessionIds),
    [enhancingSessionIds],
  );
  const hasActiveVisibleSession = useMemo(
    () =>
      !!activeSessionId &&
      buckets.some((bucket) =>
        bucket.items.some(
          (item) => item.type === "session" && item.id === activeSessionId,
        ),
      ),
    [activeSessionId, buckets],
  );

  const currentTab = useTabs((state) => state.currentTab);

  const selectedSessionId = useMemo(() => {
    return currentTab?.type === "sessions" ? currentTab.id : undefined;
  }, [currentTab]);

  const selectedIds = useTimelineSelection((s) => s.selectedIds);
  const selectedIdSet = useMemo(() => new Set(selectedIds), [selectedIds]);
  const anchorId = useTimelineSelection((s) => s.anchorId);
  const selectAll = useTimelineSelection((s) => s.selectAll);
  const clearSelection = useTimelineSelection((s) => s.clear);
  const reconcileSelection = useTimelineSelection((s) => s.reconcile);
  const deleteSession = useDeleteSession();

  const flatItemKeys = useMemo(() => {
    // Deduped: in tag mode a multi-tag session appears in several buckets but
    // must count once for selection.
    const keys = new Set<string>();
    for (const bucket of buckets) {
      for (const item of bucket.items) {
        keys.add(`${item.type}-${item.id}`);
      }
    }
    return [...keys];
  }, [buckets]);
  useEffect(
    () => reconcileSelection(flatItemKeys),
    [reconcileSelection, flatItemKeys],
  );
  const flatItemKeysRef = useRef(flatItemKeys);
  flatItemKeysRef.current = flatItemKeys;
  const getFlatItemKeys = useCallback(() => flatItemKeysRef.current, []);
  const flatSessionItemKeys = useMemo(
    () => flatItemKeys.filter(isSessionItemKey),
    [flatItemKeys],
  );
  const selectAllShortcutStateRef = useRef({
    anchorId,
    flatSessionItemKeys,
    selectedIds,
    selectedSessionId,
    selectAll,
  });
  selectAllShortcutStateRef.current = {
    anchorId,
    flatSessionItemKeys,
    selectedIds,
    selectedSessionId,
    selectAll,
  };

  const indicatorIndex = useMemo(() => {
    if (groupBy === "tag" || hasToday) {
      return -1;
    }
    return getFallbackIndicatorIndex(buckets, Date.now());
  }, [buckets, groupBy, hasToday, indicatorTimeMs]);
  const hasFutureItems = useMemo(
    () => hasFutureTimelineItems(buckets, Date.now()),
    [buckets, indicatorTimeMs],
  );
  const suppressCurrentTimeIndicator =
    groupBy === "tag" || hasActiveVisibleSession || !hasFutureItems;
  const rows = useMemo(() => {
    const base = buildTimelineRows({
      buckets,
      currentTimeMs: indicatorTimeMs,
      fallbackIndicatorIndex: indicatorIndex,
      groupBy,
      hasToday,
      suppressCurrentTimeIndicator,
    });
    const result: TimelineRow[] = [];
    const bucketsById = new Map(buckets.map((bucket) => [bucket.id, bucket]));
    if (groupBy === "date")
      result.push({ kind: "page", key: "page:future", streamKey: "future" });
    for (let index = 0; index < base.length; index++) {
      const row = base[index]!;
      result.push(row);
      const bucketId =
        row.kind === "bucket-header"
          ? row.bucket.id
          : row.kind === "session"
            ? row.bucketId
            : null;
      const next = base[index + 1];
      if (bucketId && (!next || next.kind === "bucket-header")) {
        const bucket = bucketsById.get(bucketId);
        if (
          bucket?.streamKey &&
          expandedTagSet.has(bucketId) &&
          (bucket.directCount ?? 0) > 0
        )
          result.push({
            kind: "page",
            key: `page:${bucketId}`,
            streamKey: bucket.streamKey,
          });
      }
    }
    if (groupBy === "date")
      result.push({ kind: "page", key: "page:past", streamKey: "past" });
    return result;
  }, [
    buckets,
    expandedTagSet,
    groupBy,
    hasToday,
    indicatorIndex,
    indicatorTimeMs,
    suppressCurrentTimeIndicator,
  ]);
  const bucketHeaderIndexes = useMemo(
    () =>
      rows.flatMap((row, index) =>
        row.kind === "bucket-header" ? [index] : [],
      ),
    [rows],
  );
  const currentTimeRowIndex = rows.findIndex(
    (row) => row.kind === "current-time",
  );
  const selectedSessionRowIndex = rows.findIndex(
    (row) => row.kind === "session" && row.item.id === selectedSessionId,
  );
  const upcomingMeetingRowIndex = rows.findIndex(
    (row) =>
      row.kind === "session" &&
      `session-${row.item.id}` === upcomingMeetingStatus?.itemKey,
  );
  const activeStickyIndexRef = useRef<number | null>(null);
  const extractTimelineRange = useCallback(
    (range: Range) => {
      activeStickyIndexRef.current = null;
      for (let index = bucketHeaderIndexes.length - 1; index >= 0; index--) {
        const headerIndex = bucketHeaderIndexes[index];
        if (headerIndex !== undefined && headerIndex <= range.startIndex) {
          activeStickyIndexRef.current = headerIndex;
          break;
        }
      }
      return [
        ...new Set([
          ...(activeStickyIndexRef.current === null
            ? []
            : [activeStickyIndexRef.current]),
          ...(currentTimeRowIndex < 0 ? [] : [currentTimeRowIndex]),
          ...(upcomingMeetingRowIndex < 0 ? [] : [upcomingMeetingRowIndex]),
          ...defaultRangeExtractor(range),
        ]),
      ].sort((left, right) => left - right);
    },
    [bucketHeaderIndexes, currentTimeRowIndex, upcomingMeetingRowIndex],
  );
  const topSpacerClassName = topChromeInset
    ? "h-12"
    : topChipsOverlapHeader
      ? "h-9"
      : "h-8";
  const topSpacerHeight = topChromeInset ? 48 : topChipsOverlapHeader ? 36 : 32;
  const showTopSpacer = topChromeInset || hasMoreFutureItems;
  const preludeRef = useRef<HTMLDivElement>(null);
  const [preludeHeight, setPreludeHeight] = useState(27);
  useLayoutEffect(() => {
    const element = preludeRef.current;
    if (!element) return;
    const measure = () =>
      setPreludeHeight(element.getBoundingClientRect().height || 27);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const timelinePreludeHeight =
    (showTopSpacer ? topSpacerHeight : 0) + preludeHeight;
  const topChipStackTopClassName = topChromeInset
    ? "top-4"
    : topChipsOverlapHeader
      ? "top-1"
      : "top-2";
  const containerRef = useRef<HTMLDivElement>(null);
  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    estimateSize: (index) => getTimelineRowHeight(rows[index]!),
    getItemKey: (index) => rows[index]?.key ?? index,
    getScrollElement: () => containerRef.current,
    overscan: 8,
    rangeExtractor: extractTimelineRange,
    scrollMargin: timelinePreludeHeight,
  });
  const virtualRows = rowVirtualizer.getVirtualItems();
  const visibleBucketIds = virtualRows
    .flatMap((virtualRow) => {
      const row = rows[virtualRow.index];
      return row?.kind === "bucket-header"
        ? [row.bucket.id]
        : row?.kind === "session"
          ? [row.bucketId]
          : row?.kind === "page"
            ? [row.streamKey]
            : [];
    })
    .join("\n");
  useEffect(() => {
    if (!available) return;
    const visible = new Set(visibleBucketIds.split("\n"));
    owner.ensureVisible(
      buckets.flatMap((bucket) =>
        visible.has(bucket.id) &&
        expandedTagSet.has(bucket.id) &&
        (bucket.directCount ?? 0) > 0 &&
        bucket.streamKey &&
        bucket.view
          ? [{ key: bucket.streamKey, view: bucket.view }]
          : [],
      ),
    );
  }, [owner, available, visibleBucketIds, buckets, expandedTagSet]);

  const isTodayVisible = isVirtualRowVisible(
    rowVirtualizer,
    virtualRows,
    currentTimeRowIndex,
  );
  const currentTimeVirtualRow = virtualRows.find(
    (row) => row.index === currentTimeRowIndex,
  );
  const isScrolledPastToday =
    currentTimeVirtualRow !== undefined &&
    currentTimeVirtualRow.end <= (rowVirtualizer.scrollOffset ?? 0);
  const isUpcomingMeetingVisible = isVirtualRowVisible(
    rowVirtualizer,
    virtualRows,
    upcomingMeetingRowIndex,
  );
  const scrollElement = containerRef.current;
  const isScrolledToBottom =
    scrollElement === null ||
    scrollElement.scrollHeight -
      scrollElement.clientHeight -
      scrollElement.scrollTop <=
      12;
  const showUpcomingMeetingChip =
    Boolean(upcomingMeetingStatus) && !isUpcomingMeetingVisible;
  const showTopNowChip =
    !showUpcomingMeetingChip && !isTodayVisible && isScrolledPastToday;
  const scrollToToday = useCallback(() => {
    if (currentTimeRowIndex >= 0) {
      rowVirtualizer.scrollToIndex(currentTimeRowIndex, {
        align: "center",
        behavior: "smooth",
      });
    }
  }, [currentTimeRowIndex, rowVirtualizer]);
  const scrollToUpcomingMeeting = useCallback(() => {
    if (upcomingMeetingRowIndex >= 0) {
      rowVirtualizer.scrollToIndex(upcomingMeetingRowIndex, {
        align: "center",
        behavior: "smooth",
      });
    }
  }, [rowVirtualizer, upcomingMeetingRowIndex]);
  const todayBucketLength = useMemo(() => {
    const b = buckets.find((bucket) => bucket.label === "Today");
    return b?.items.length ?? 0;
  }, [buckets]);
  const initialNowScrollDoneRef = useRef(false);
  const previousTodayBucketLengthRef = useRef(todayBucketLength);
  useEffect(() => {
    if (!hasToday || currentTimeRowIndex < 0) {
      previousTodayBucketLengthRef.current = todayBucketLength;
      return;
    }

    const todayLengthChanged =
      previousTodayBucketLengthRef.current !== todayBucketLength;
    previousTodayBucketLengthRef.current = todayBucketLength;
    if (initialNowScrollDoneRef.current || owner.readViewport(groupBy)) {
      return;
    }
    initialNowScrollDoneRef.current = true;

    const frame = requestAnimationFrame(() => {
      if (!isTodayVisible) {
        rowVirtualizer.scrollToIndex(currentTimeRowIndex, {
          align: "center",
          behavior: todayLengthChanged ? "smooth" : "auto",
        });
      }
    });
    return () => cancelAnimationFrame(frame);
  }, [
    currentTimeRowIndex,
    hasToday,
    isTodayVisible,
    rowVirtualizer,
    todayBucketLength,
    owner,
    groupBy,
  ]);
  const initialWindowReady =
    groupBy === "date"
      ? Boolean(
          state.streams.get("past")?.pages.length &&
          state.streams.get("future")?.pages.length,
        )
      : Boolean(state.facets && !state.facetsPending);
  const lastSelectedScroll = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (lastSelectedScroll.current === selectedSessionId) return;
    if (currentTab?.type !== "sessions") {
      lastSelectedScroll.current = undefined;
      return;
    }
    if (selectedSessionRowIndex < 0) {
      if (initialWindowReady) lastSelectedScroll.current = selectedSessionId;
      return;
    }
    lastSelectedScroll.current = selectedSessionId;
    if (
      owner.readViewport(groupBy) ||
      isVirtualRowVisible(
        rowVirtualizer,
        rowVirtualizer.getVirtualItems(),
        selectedSessionRowIndex,
      )
    )
      return;
    const frame = requestAnimationFrame(() => {
      rowVirtualizer.scrollToIndex(selectedSessionRowIndex, {
        align: "center",
        behavior: "smooth",
      });
    });
    return () => cancelAnimationFrame(frame);
  }, [
    currentTab?.type,
    selectedSessionId,
    rowVirtualizer,
    selectedSessionRowIndex,
    initialWindowReady,
    owner,
    groupBy,
  ]);

  const viewportKey = groupBy;
  const restoreAttempt = useRef<string | null>(null);
  const cohort = `${state.epoch}:${groupBy}`;
  useEffect(() => {
    if (restoreAttempt.current === cohort) return;
    const saved = owner.readViewport(viewportKey);
    if (!saved) {
      restoreAttempt.current = cohort;
      return;
    }
    if (
      !state.revision ||
      state !== owner.getSnapshot() ||
      [...state.streams.values()].some((stream) => stream.pending) ||
      (groupBy === "date"
        ? !state.streams.get("past")?.pages.length ||
          !state.streams.get("future")?.pages.length
        : !state.facets)
    )
      return;
    restoreAttempt.current = cohort;
    initialNowScrollDoneRef.current = true;
    const index = rows.findIndex(
      (row) =>
        row.kind === "session" &&
        row.item.id === saved.sessionId &&
        row.bucketId === saved.bucketId,
    );
    if (index >= 0) {
      rowVirtualizer.scrollToIndex(index, { align: "start", behavior: "auto" });
      const element = containerRef.current;
      if (element) element.scrollTop += saved.offset;
    } else {
      if (containerRef.current) containerRef.current.scrollTop = 0;
      owner.viewportUnavailable();
    }
  }, [cohort, owner, viewportKey, rows, rowVirtualizer, state, groupBy]);
  const rememberViewport = useCallback(() => {
    const scrollTop = containerRef.current?.scrollTop ?? 0;
    const first = rowVirtualizer
      .getVirtualItems()
      .find(
        (value) =>
          value.end > scrollTop && rows[value.index]?.kind === "session",
      );
    const row = first ? rows[first.index] : undefined;
    if (first && row?.kind === "session")
      owner.saveViewport(viewportKey, {
        sessionId: row.item.id,
        bucketId: row.bucketId,
        offset: scrollTop - first.start,
        revision: state.revision,
      });
  }, [owner, viewportKey, rows, rowVirtualizer, state.revision]);

  useMountEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      const container = containerRef.current;

      if (
        !container ||
        container.closest("[inert], [aria-hidden='true']") ||
        event.defaultPrevented ||
        !isSelectAllShortcut(event) ||
        isTextEditingShortcutTarget(event.target) ||
        isTextEditingShortcutTarget(document.activeElement)
      ) {
        return;
      }

      const {
        anchorId,
        flatSessionItemKeys,
        selectedIds,
        selectedSessionId,
        selectAll,
      } = selectAllShortcutStateRef.current;

      if (
        !selectedSessionId ||
        flatSessionItemKeys.length === 0 ||
        !hasSidebarNoteSelectionContext({
          anchorId,
          selectedIds,
          selectedSessionId,
        })
      ) {
        return;
      }

      event.preventDefault();
      selectAll(flatSessionItemKeys);
    };

    window.addEventListener("keydown", handleKeyDown, { capture: true });
    return () => {
      window.removeEventListener("keydown", handleKeyDown, { capture: true });
    };
  });

  const handleDeleteSelected = useCallback(() => {
    const sessionIds = selectedIds
      .filter((key) => key.startsWith("session-"))
      .map((key) => key.replace("session-", ""));

    const batchId = sessionIds.length > 1 ? crypto.randomUUID() : undefined;

    for (const sessionId of sessionIds) {
      deleteSession(sessionId, {
        batchId,
        title: timelineSessionsTable?.[sessionId]?.title ?? undefined,
      });
    }

    clearSelection();
  }, [selectedIds, deleteSession, clearSelection, timelineSessionsTable]);

  const sessionCount = useMemo(
    () => selectedIds.filter((key) => key.startsWith("session-")).length,
    [selectedIds],
  );

  const contextMenuItems = useMemo(
    () =>
      selectedIds.length > 0
        ? [
            {
              id: "delete-selected",
              text: t`Delete Selected (${sessionCount})`,
              action: handleDeleteSelected,
              disabled: sessionCount === 0,
            },
          ]
        : [],
    [selectedIds, sessionCount, handleDeleteSelected, t],
  );

  const showContextMenu = useNativeContextMenu(contextMenuItems);
  const handleWheelCapture = useCallback(
    (event: ReactWheelEvent<HTMLDivElement>) => {
      const container = containerRef.current;
      const target = event.target;

      if (
        !container ||
        event.defaultPrevented ||
        (target instanceof Node && container.contains(target))
      ) {
        return;
      }

      scrollElementByWheel(container, event);
    },
    [containerRef],
  );

  return (
    <div
      data-sidebar-timeline-root
      className="relative h-full"
      onWheelCapture={handleWheelCapture}
    >
      <div
        ref={containerRef}
        data-sidebar-timeline-scroll
        onContextMenu={showContextMenu}
        onScroll={rememberViewport}
        className={cn(["flex h-full flex-col overflow-y-auto", "rounded-xl"])}
      >
        {(topChromeInset || hasMoreFutureItems) && (
          <div
            aria-hidden
            data-sidebar-timeline-top-spacer
            className={cn([topSpacerClassName, "shrink-0"])}
          />
        )}
        <div ref={preludeRef}>
          <div
            data-sidebar-timeline-group-toggle
            className="flex shrink-0 items-center gap-2.5 pt-2 pb-1 pl-3"
          >
            {(["date", "tag"] as const).map((mode) => (
              <button
                key={mode}
                type="button"
                aria-pressed={mode === groupBy}
                onClick={() => void setSettingValue("sidebar_group_by", mode)}
                className={cn([
                  "rounded-sm text-[11px] font-medium transition-none",
                  "focus-visible:ring-ring focus-visible:ring-2 focus-visible:outline-hidden",
                  mode === groupBy
                    ? "text-foreground"
                    : "text-muted-foreground hover:text-foreground",
                ])}
              >
                {mode === "date" ? t`By date` : t`By tag`}
              </button>
            ))}
            {groupBy === "tag" && (
              <button
                type="button"
                aria-label={anyTagExpanded ? t`Collapse all` : t`Expand all`}
                title={anyTagExpanded ? t`Collapse all` : t`Expand all`}
                onClick={toggleAllTagsExpanded}
                className={cn([
                  "mr-3 ml-auto",
                  "text-muted-foreground hover:text-foreground transition-none",
                ])}
              >
                {anyTagExpanded ? (
                  <ChevronsDownUpIcon size={13} />
                ) : (
                  <ChevronsUpDownIcon size={13} />
                )}
              </button>
            )}
          </div>
          <div
            className="text-muted-foreground px-3 text-xs"
            aria-live="polite"
          >
            {!supported ? (
              <p>
                <Trans>This server does not support the timeline.</Trans>
              </p>
            ) : !available ? (
              <p>
                <Trans>Vault unavailable</Trans>
              </p>
            ) : null}
            {timeline.connection.state === "reconnecting" && (
              <p>
                <Trans>Reconnecting… Loaded notes remain available.</Trans>
              </p>
            )}
            {state.notice && (
              <button type="button" onClick={() => owner.clearNotice()}>
                {state.notice === "anchor_missing" ? (
                  <Trans>
                    The previous note is outside the refreshed window. Scroll
                    position was reset.
                  </Trans>
                ) : (
                  <Trans>
                    The timeline changed. Its loaded window was refreshed.
                  </Trans>
                )}
              </button>
            )}
            {state.facetsError && groupBy === "tag" && (
              <p role="alert">
                {state.facetsError.message}{" "}
                <button onClick={() => owner.retryFacets()}>
                  <Trans>Retry</Trans>
                </button>
              </p>
            )}
            {state.facetsPending && groupBy === "tag" && (
              <p role="status">
                <Trans>Loading tags…</Trans>
              </p>
            )}
            {sessionCount > 0 && (
              <p>
                <Trans>{sessionCount} loaded notes selected</Trans>
              </p>
            )}
            {selectedSessionId &&
              !flatSessionItemKeys.includes(`session-${selectedSessionId}`) &&
              state.revision && (
                <p>
                  <Trans>
                    The open note is outside this loaded timeline window.
                  </Trans>
                </p>
              )}
            {hasMoreFutureItems && !state.allFuture && (
              <button onClick={() => owner.showFuture()}>
                <Trans>Show later dates ({laterCount})</Trans>
              </button>
            )}
          </div>
        </div>
        <div
          data-sidebar-timeline-virtual-canvas
          className="relative w-full shrink-0"
          style={{ height: rowVirtualizer.getTotalSize() }}
        >
          {virtualRows.map((virtualRow) => {
            const row = rows[virtualRow.index];
            if (!row) {
              return null;
            }
            const activeStickyHeader =
              row.kind === "bucket-header" &&
              activeStickyIndexRef.current === virtualRow.index;

            return (
              <div
                key={virtualRow.key}
                data-index={virtualRow.index}
                data-sidebar-timeline-virtual-row={row.kind}
                className={cn([
                  "left-0 w-full",
                  activeStickyHeader ? "z-20" : "absolute top-0",
                ])}
                style={
                  activeStickyHeader
                    ? {
                        height: virtualRow.size,
                        position: "sticky",
                        top: topChromeInset ? 48 : 0,
                      }
                    : {
                        height: virtualRow.size,
                        transform: `translateY(${virtualRow.start - rowVirtualizer.options.scrollMargin}px)`,
                      }
                }
              >
                <TimelineVirtualRow
                  row={row}
                  bucketItemCounts={bucketItemCounts}
                  enhancingSessionIds={enhancingSessionIdSet}
                  expandedTagSet={expandedTagSet}
                  getFlatItemKeys={getFlatItemKeys}
                  selectedIdSet={selectedIdSet}
                  selectedSessionId={selectedSessionId}
                  topChromeInset={topChromeInset}
                  timezone={timezone}
                  toggleTagExpanded={toggleTagExpanded}
                  upcomingMeetingStatus={upcomingMeetingStatus}
                />
              </div>
            );
          })}
        </div>
      </div>

      {!isScrolledToBottom && (
        <div
          aria-hidden
          data-sidebar-timeline-bottom-fade
          className="from-background/0 to-background pointer-events-none absolute inset-x-0 bottom-0 z-30 h-7 bg-linear-to-b"
        />
      )}

      {topChromeInset && (
        <div
          aria-hidden
          data-sidebar-timeline-top-occluder
          className="bg-background pointer-events-none absolute inset-x-0 top-0 z-10 h-12"
        />
      )}

      {available &&
        supported &&
        (showUpcomingMeetingChip || showTopNowChip) && (
          <div
            data-sidebar-timeline-top-chip-stack
            className={cn([
              "absolute left-1/2 z-20 flex -translate-x-1/2 transform flex-col items-center gap-2",
              topChipStackTopClassName,
            ])}
          >
            {upcomingMeetingStatus && showUpcomingMeetingChip && (
              <SidebarUpcomingMeetingStatus
                label={upcomingMeetingStatus.label}
                onClick={scrollToUpcomingMeeting}
                title={upcomingMeetingStatus.title}
              />
            )}
            {showTopNowChip && (
              <TimelineNowChip direction="up" onClick={scrollToToday} />
            )}
          </div>
        )}

      {available &&
        supported &&
        !showUpcomingMeetingChip &&
        !isTodayVisible &&
        !isScrolledPastToday && (
          <TimelineNowChip
            onClick={scrollToToday}
            direction="down"
            className={cn([
              "absolute bottom-2 left-1/2 -translate-x-1/2 transform",
              "z-40",
            ])}
          />
        )}
    </div>
  );
});

function TimelineVirtualRow({
  row,
  bucketItemCounts,
  enhancingSessionIds,
  expandedTagSet,
  getFlatItemKeys,
  selectedIdSet,
  selectedSessionId,
  topChromeInset,
  timezone,
  toggleTagExpanded,
  upcomingMeetingStatus,
}: {
  row: TimelineRow;
  bucketItemCounts: Map<string, number>;
  enhancingSessionIds: Set<string>;
  expandedTagSet: Set<string>;
  getFlatItemKeys: () => string[];
  selectedIdSet: Set<string>;
  selectedSessionId: string | undefined;
  topChromeInset: boolean;
  timezone?: string;
  toggleTagExpanded: (bucketId: string) => void;
  upcomingMeetingStatus: ReturnType<typeof useUpcomingMeetingStatus>;
}) {
  if (row.kind === "page")
    return <TimelineStreamControl streamKey={row.streamKey} />;

  if (row.kind === "bucket-header") {
    const { bucket } = row;
    return (
      <div
        data-sidebar-timeline-bucket-header
        className={cn([
          "bg-background z-20 h-[27px] pr-1 pb-1 pl-3",
          topChromeInset ? "top-12" : "top-0",
        ])}
      >
        {bucket.kind === "tag" ? (
          <button
            type="button"
            aria-expanded={expandedTagSet.has(bucket.id)}
            onClick={() => toggleTagExpanded(bucket.id)}
            style={
              bucket.depth ? { paddingLeft: bucket.depth * 12 } : undefined
            }
            className={cn([
              "text-muted-foreground hover:text-foreground flex w-full items-center gap-1",
              "pt-2 text-[11px] font-semibold transition-none",
            ])}
          >
            {expandedTagSet.has(bucket.id) ? (
              <ChevronDownIcon size={12} className="shrink-0" />
            ) : (
              <ChevronRightIcon size={12} className="shrink-0" />
            )}
            <span className="truncate">
              {bucket.id === UNDATED_ID ? <Trans>Undated</Trans> : bucket.label}
            </span>
            <span className="tracking-normal">
              ({bucketItemCounts.get(bucket.id) ?? 0})
            </span>
          </button>
        ) : (
          <div className="text-muted-foreground pt-2 text-[11px] font-semibold">
            {bucket.label}
          </div>
        )}
      </div>
    );
  }

  if (row.kind === "current-time") {
    const indicator = row.suppressed ? (
      <CurrentTimeAnchor />
    ) : (
      <CurrentTimeIndicator timezone={timezone} />
    );
    if (row.gap === "top-bucket") {
      return (
        <div className="pt-3">
          <div data-sidebar-current-time-header-gap className="py-3">
            {indicator}
          </div>
        </div>
      );
    }
    return row.gap === "bucket" ? (
      <div data-sidebar-current-time-header-gap className="py-3">
        {indicator}
      </div>
    ) : (
      indicator
    );
  }

  if (row.kind === "empty-today") {
    return (
      <div className="text-muted-foreground px-3 py-4 text-center text-sm">
        <Trans>No items today</Trans>
      </div>
    );
  }

  const itemKey = `${row.item.type}-${row.item.id}`;
  const isUpcoming = itemKey === upcomingMeetingStatus?.itemKey;
  return (
    <TimelineItemComponent
      item={row.item}
      precision={row.precision}
      selected={row.item.id === selectedSessionId}
      timezone={timezone}
      multiSelected={selectedIdSet.has(itemKey)}
      getFlatItemKeys={getFlatItemKeys}
      isEnhancing={enhancingSessionIds.has(row.item.id)}
      isUpcoming={isUpcoming}
      upcomingProgress={
        isUpcoming ? upcomingMeetingStatus?.progress : undefined
      }
    />
  );
}

function SidebarUpcomingMeetingStatus({
  label,
  onClick,
  title,
}: {
  label: string;
  onClick: () => void;
  title: string;
}) {
  const { t } = useLingui();
  return (
    <TimelineTopChip
      aria-live="polite"
      ariaLabel={`${title || t`Meeting`} ${label.toLowerCase()}`}
      data-sidebar-upcoming-meeting-status
      className="border-destructive bg-destructive text-destructive-foreground w-28 justify-center shadow-md"
      icon={<ArrowUpIcon aria-hidden className="size-3" strokeWidth={2.4} />}
      onClick={onClick}
    >
      {label}
    </TimelineTopChip>
  );
}

function TimelineTopChip({
  ariaLabel,
  children,
  icon,
  onClick,
  ...props
}: {
  ariaLabel?: string;
  children: ReactNode;
  icon: ReactNode;
  className?: string;
  role?: string;
  "aria-live"?: "off" | "polite" | "assertive";
  "data-sidebar-upcoming-meeting-status"?: true;
  onClick?: () => void;
}) {
  const className = cn([
    "border-border bg-card text-muted-foreground flex h-6 items-center gap-1 rounded-full border px-2.5 text-xs font-medium shadow-xs",
    onClick && "hover:bg-accent hover:text-foreground transition-none",
    "focus-visible:ring-ring focus-visible:ring-2 focus-visible:outline-hidden",
    props.className,
  ]);

  if (onClick) {
    return (
      <Button
        {...props}
        aria-label={ariaLabel}
        className={className}
        onClick={onClick}
        size="sm"
        variant="outline"
      >
        <span className="flex size-3 shrink-0 items-center justify-center">
          {icon}
        </span>
        <span className="truncate">{children}</span>
      </Button>
    );
  }

  return (
    <div {...props} aria-label={ariaLabel} className={className}>
      <span className="flex size-3 shrink-0 items-center justify-center">
        {icon}
      </span>
      <span className="truncate">{children}</span>
    </div>
  );
}

function getFallbackIndicatorIndex(buckets: TimelineBucket[], nowMs: number) {
  let staleFutureBoundary: number | null = null;

  for (let index = 0; index < buckets.length; index++) {
    const bucket = buckets[index];
    const firstItem = bucket?.items[0];
    if (!bucket || !firstItem) {
      continue;
    }

    const itemDate = getItemTimestamp(firstItem);
    if (!itemDate || itemDate.getTime() >= nowMs) {
      continue;
    }

    if (isFutureBucketLabel(bucket.label)) {
      staleFutureBoundary = index + 1;
      continue;
    }

    return staleFutureBoundary ?? index;
  }

  return staleFutureBoundary ?? -1;
}

function isFutureBucketLabel(label: string) {
  return (
    label === "Tomorrow" ||
    label === "next week" ||
    label === "next month" ||
    label.startsWith("in ")
  );
}

function isSelectAllShortcut(event: KeyboardEvent) {
  return (
    event.key.toLowerCase() === "a" &&
    (event.metaKey || event.ctrlKey) &&
    !event.altKey &&
    !event.shiftKey
  );
}

function isSessionItemKey(key: string) {
  return key.startsWith("session-");
}

function hasSidebarNoteSelectionContext({
  anchorId,
  selectedIds,
  selectedSessionId,
}: {
  anchorId: string | null;
  selectedIds: string[];
  selectedSessionId: string;
}) {
  const currentSessionKey = `session-${selectedSessionId}`;

  return anchorId === currentSessionKey || selectedIds.some(isSessionItemKey);
}

function isTextEditingShortcutTarget(target: EventTarget | null) {
  const element =
    target instanceof Element
      ? target
      : target instanceof Node
        ? target.parentElement
        : null;

  return (
    element !== null &&
    Boolean(
      element.closest(
        [
          "input",
          "textarea",
          "select",
          "[contenteditable='true']",
          "[role='textbox']",
          ".ProseMirror",
        ].join(","),
      ),
    )
  );
}

function TimelineNowChip({
  className,
  direction,
  onClick,
}: {
  className?: string;
  direction: "up" | "down";
  onClick: () => void;
}) {
  const DirectionIcon = direction === "up" ? ArrowUpIcon : ArrowDownIcon;
  const { t } = useLingui();

  return (
    <button
      type="button"
      aria-label={t`Go back to now`}
      className={cn([
        "border-border bg-card text-foreground flex h-6 items-center gap-1 rounded-full border px-2.5 text-xs font-medium shadow-md",
        "hover:border-border hover:bg-accent hover:text-foreground transition-none",
        "focus-visible:ring-ring focus-visible:ring-2 focus-visible:outline-hidden",
        className,
      ])}
      onClick={onClick}
    >
      {direction === "up" ? (
        <DirectionIcon size={12} strokeWidth={1.75} />
      ) : null}
      <SunIcon size={13} className="text-brand shrink-0" />
      <span>
        <Trans>Now</Trans>
      </span>
      {direction === "down" ? (
        <DirectionIcon size={12} strokeWidth={1.75} />
      ) : null}
    </button>
  );
}

function CurrentTimeAnchor() {
  return (
    <div
      aria-hidden
      data-sidebar-current-time-anchor
      className={cn(["pointer-events-none opacity-0", "relative z-20 h-px"])}
    />
  );
}

function isVirtualRowVisible(
  virtualizer: {
    scrollOffset: number | null;
    scrollRect: { height: number } | null;
  },
  virtualRows: Array<{ end: number; index: number; start: number }>,
  targetIndex: number,
) {
  if (targetIndex < 0) {
    return false;
  }
  const row = virtualRows.find((item) => item.index === targetIndex);
  if (!row) {
    return false;
  }
  const scrollOffset = virtualizer.scrollOffset ?? 0;
  const viewportHeight = virtualizer.scrollRect?.height ?? 0;
  const margin = 8;
  return (
    row.end > scrollOffset + margin &&
    row.start < scrollOffset + viewportHeight - margin
  );
}

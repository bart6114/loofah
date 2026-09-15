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
} from "react";

import { Button } from "@hypr/ui/components/ui/button";
import { cn } from "@hypr/utils";

import { TimelineItemComponent } from "./item";
import { useTimelineSessionsTable } from "./queries";
import {
  CurrentTimeIndicator,
  useCurrentTimeMs,
  useSmartCurrentTime,
} from "./realtime";
import {
  buildTimelineRows,
  getTimelineRowHeight,
  type TimelineRow,
} from "./rows";
import {
  useUpcomingMeetingStatus,
  useUpcomingMeetingLabelFormatter,
} from "./upcoming-meeting";
import {
  buildTagTimelineBuckets,
  buildTimelineBuckets,
  deriveTimelineWindowData,
  getItemTimestamp,
  hasFutureTimelineItems,
  type TimelineBucket,
  type TimelineSessionsTable,
} from "./utils";

import { useEnhancingSessionIds } from "~/ai/hooks/useEnhancingSessions";
import { useDeleteSession } from "~/session/hooks/useDeleteSession";
import { setSettingValue } from "~/settings/queries";
import { useConfigValue } from "~/shared/config";
import { scrollElementByWheel } from "~/shared/dom/scroll-wheel";
import { useMountEffect } from "~/shared/hooks/useMountEffect";
import { useNativeContextMenu } from "~/shared/hooks/useNativeContextMenu";
import { useTabs } from "~/store/zustand/tabs";
import { useTimelineSelection } from "~/store/zustand/timeline-selection";
import { useListener } from "~/stt/contexts";

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
  const expandedTags = useConfigValue("sidebar_expanded_tags");
  const timelineSessionsTable = useTimelineSessionsTable();

  const { buckets: allBuckets, hasMoreFutureItems } = useTimelineData({
    timelineSessionsTable,
    timezone,
    groupBy,
  });
  const expandedTagSet = useMemo(() => new Set(expandedTags), [expandedTags]);
  // Tag sections default to collapsed: headers keep the full count but drop
  // their items -- also from the flat selection keys below, so keyboard/shift
  // selection skips hidden rows. Nested tag buckets (slash tags) only render
  // when every ancestor is expanded; walk `parentId` rather than splitting ids
  // so synthetic buckets like `Untagged` are never parsed as paths.
  const buckets = useMemo(() => {
    if (groupBy !== "tag") {
      return allBuckets;
    }
    const bucketsById = new Map(
      allBuckets.map((bucket) => [bucket.id, bucket]),
    );
    return allBuckets
      .filter((bucket) => {
        for (let p = bucket.parentId; p; p = bucketsById.get(p)?.parentId) {
          if (!expandedTagSet.has(p)) {
            return false;
          }
        }
        return true;
      })
      .map((bucket) =>
        expandedTagSet.has(bucket.id) ? bucket : { ...bucket, items: [] },
      );
  }, [allBuckets, expandedTagSet, groupBy]);
  const bucketItemCounts = useMemo(
    () =>
      new Map(
        allBuckets.map((bucket) => [
          bucket.id,
          bucket.totalCount ?? bucket.items.length,
        ]),
      ),
    [allBuckets],
  );
  const toggleTagExpanded = useCallback(
    (label: string) => {
      const next = expandedTags.includes(label)
        ? expandedTags.filter((tag) => tag !== label)
        : [...expandedTags, label];
      void setSettingValue("sidebar_expanded_tags", JSON.stringify(next));
    },
    [expandedTags],
  );
  const anyTagExpanded = useMemo(
    () =>
      groupBy === "tag" &&
      allBuckets.some((bucket) => expandedTagSet.has(bucket.id)),
    [allBuckets, expandedTagSet, groupBy],
  );
  const toggleAllTagsExpanded = useCallback(() => {
    const next = anyTagExpanded ? [] : allBuckets.map((bucket) => bucket.id);
    void setSettingValue("sidebar_expanded_tags", JSON.stringify(next));
  }, [allBuckets, anyTagExpanded]);
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
  const activeSessionId = useListener((state) =>
    state.live.status === "active" || state.live.status === "finalizing"
      ? state.live.sessionId
      : null,
  );
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
  const rows = useMemo(
    () =>
      buildTimelineRows({
        buckets,
        currentTimeMs: indicatorTimeMs,
        fallbackIndicatorIndex: indicatorIndex,
        groupBy,
        hasToday,
        suppressCurrentTimeIndicator,
      }),
    [
      buckets,
      groupBy,
      hasToday,
      indicatorIndex,
      indicatorTimeMs,
      suppressCurrentTimeIndicator,
    ],
  );
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
  const timelinePreludeHeight = (showTopSpacer ? topSpacerHeight : 0) + 27;
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
    if (initialNowScrollDoneRef.current && !todayLengthChanged) {
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
  ]);
  useEffect(() => {
    if (
      currentTab?.type !== "sessions" ||
      selectedSessionRowIndex < 0 ||
      isVirtualRowVisible(
        rowVirtualizer,
        rowVirtualizer.getVirtualItems(),
        selectedSessionRowIndex,
      )
    ) {
      return;
    }

    const frame = requestAnimationFrame(() => {
      rowVirtualizer.scrollToIndex(selectedSessionRowIndex, {
        align: "center",
        behavior: "smooth",
      });
    });
    return () => cancelAnimationFrame(frame);
  }, [currentTab?.type, rowVirtualizer, selectedSessionRowIndex]);

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
        className={cn(["flex h-full flex-col overflow-y-auto", "rounded-xl"])}
      >
        {(topChromeInset || hasMoreFutureItems) && (
          <div
            aria-hidden
            data-sidebar-timeline-top-spacer
            className={cn([topSpacerClassName, "shrink-0"])}
          />
        )}
        <div
          data-sidebar-timeline-group-toggle
          className="flex shrink-0 items-center gap-2.5 pt-2 pb-1 pl-3"
        >
          {(["date", "tag"] as const).map((mode) => (
            <button
              key={mode}
              type="button"
              onClick={() => void setSettingValue("sidebar_group_by", mode)}
              className={cn([
                "text-[10px] font-semibold tracking-[0.09em] uppercase transition-none",
                mode === groupBy
                  ? "text-foreground"
                  : "text-muted-foreground/50 hover:text-muted-foreground",
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
                "text-muted-foreground/50 hover:text-muted-foreground transition-none",
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

      {(showUpcomingMeetingChip || showTopNowChip) && (
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

      {!showUpcomingMeetingChip && !isTodayVisible && !isScrolledPastToday && (
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
              "text-muted-foreground/70 hover:text-foreground flex w-full items-center gap-1",
              "pt-2 text-[10px] font-semibold tracking-[0.09em] uppercase transition-none",
            ])}
          >
            {expandedTagSet.has(bucket.id) ? (
              <ChevronDownIcon size={12} className="shrink-0" />
            ) : (
              <ChevronRightIcon size={12} className="shrink-0" />
            )}
            <span className="truncate">{bucket.label}</span>
            <span className="tracking-normal">
              ({bucketItemCounts.get(bucket.id) ?? 0})
            </span>
          </button>
        ) : (
          <div className="text-muted-foreground/70 pt-2 text-[10px] font-semibold tracking-[0.09em] uppercase">
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

function useTimelineData({
  timelineSessionsTable,
  timezone,
  groupBy,
}: {
  timelineSessionsTable: TimelineSessionsTable;
  timezone?: string;
  groupBy: "date" | "tag";
}): {
  buckets: TimelineBucket[];
  hasMoreFutureItems: boolean;
} {
  // Tag mode has no date window: every session shows under its tags, so the
  // future-items filtering (and its "more" affordance) doesn't apply.
  const windowData = useMemo(
    () =>
      groupBy === "tag"
        ? { timelineSessionsTable, hasMoreFutureItems: false }
        : deriveTimelineWindowData({ timelineSessionsTable, timezone }),
    [groupBy, timelineSessionsTable, timezone],
  );
  const currentTimeMs = useSmartCurrentTime(windowData.timelineSessionsTable);
  // Tag buckets are time-independent; pinning the dep keeps the minute tick
  // from rebuilding every bucket/item identity in tag mode.
  const bucketTimeMs = groupBy === "tag" ? 0 : currentTimeMs;

  return useMemo(() => {
    const buckets =
      groupBy === "tag"
        ? buildTagTimelineBuckets({
            timelineSessionsTable: windowData.timelineSessionsTable,
          })
        : buildTimelineBuckets({
            timelineSessionsTable: windowData.timelineSessionsTable,
            timezone,
          });

    return {
      buckets,
      hasMoreFutureItems: windowData.hasMoreFutureItems,
    };
  }, [groupBy, windowData, bucketTimeMs, timezone]);
}

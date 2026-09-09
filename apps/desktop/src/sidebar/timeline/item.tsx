import { useLingui } from "@lingui/react/macro";
import { platform } from "@tauri-apps/plugin-os";
import { BotIcon, SquareIcon } from "lucide-react";
import { memo, type DragEvent, useCallback, useMemo } from "react";

import { commands as fsSyncCommands } from "@hypr/plugin-fs-sync";
import { commands as openerCommands } from "@hypr/plugin-opener2";
import { DancingSticks } from "@hypr/ui/components/ui/dancing-sticks";
import { Spinner } from "@hypr/ui/components/ui/spinner";
import { sonnerToast } from "@hypr/ui/components/ui/toast";
import { cn, format, getYear, safeParseDate, TZDate } from "@hypr/utils";

import {
  isTimelineItemInFuture,
  type TimelineItem,
  TimelinePrecision,
} from "./utils";

import { useDeleteSession } from "~/session/hooks/useDeleteSession";
import { openStandaloneNoteWindow } from "~/session/window";
import type { MenuItemDef } from "~/shared/hooks/useNativeContextMenu";
import { writeSessionContextDragData } from "~/shared/session-drag";
import { InteractiveButton } from "~/shared/ui/interactive-button";
import { useSessionTitle } from "~/store/zustand/live-title";
import { useTabs } from "~/store/zustand/tabs";
import { useTimelineSelection } from "~/store/zustand/timeline-selection";
import { useListener } from "~/stt/contexts";
import { commands } from "~/types/tauri.gen";

const EMPTY_TIMELINE_ITEM_KEYS: string[] = [];

type ItemBaseProps = {
  title: string;
  displayTime: string;
  author?: string | null;
  isLive?: boolean;
  amplitude?: number;
  showSpinner?: boolean;
  selected: boolean;
  muted?: boolean;
  multiSelected: boolean;
  onClick: () => void;
  onDoubleClick?: () => void;
  onCmdClick: () => void;
  onShiftClick: () => void;
  onStop?: () => void;
  onDragStart?: (event: DragEvent<HTMLElement>) => void;
  contextMenu: MenuItemDef[];
  draggable?: boolean;
  timelineSessionId?: string;
  isUpcoming?: boolean;
  upcomingProgress?: number;
};

export const TimelineItemComponent = memo(
  ({
    item,
    precision,
    selected,
    timezone,
    multiSelected,
    flatItemKeys,
    getFlatItemKeys,
    isUpcoming,
    upcomingProgress,
    isEnhancing,
  }: {
    item: TimelineItem;
    precision: TimelinePrecision;
    selected: boolean;
    timezone?: string;
    multiSelected: boolean;
    flatItemKeys?: string[];
    getFlatItemKeys?: () => string[];
    isUpcoming?: boolean;
    upcomingProgress?: number;
    isEnhancing?: boolean;
  }) => {
    const readFlatItemKeys =
      getFlatItemKeys ?? (() => flatItemKeys ?? EMPTY_TIMELINE_ITEM_KEYS);

    return (
      <SessionItem
        item={item}
        precision={precision}
        selected={selected}
        timezone={timezone}
        multiSelected={multiSelected}
        getFlatItemKeys={readFlatItemKeys}
        isUpcoming={isUpcoming}
        upcomingProgress={upcomingProgress}
        isEnhancing={isEnhancing}
      />
    );
  },
);

function ItemBase({
  title,
  displayTime,
  author,
  isLive,
  amplitude,
  showSpinner,
  selected,
  muted,
  multiSelected,
  onClick,
  onDoubleClick,
  onCmdClick,
  onShiftClick,
  onStop,
  onDragStart,
  contextMenu,
  draggable,
  timelineSessionId,
  isUpcoming,
  upcomingProgress,
}: ItemBaseProps) {
  const { t } = useLingui();
  const hasSelection = useTimelineSelection((s) => s.selectedIds.length > 0);
  const showLiveStop = isLive && onStop;
  const showUpcomingGauge =
    typeof upcomingProgress === "number" &&
    Boolean(isUpcoming) &&
    !isLive &&
    !showSpinner;
  const upcomingGaugePercent =
    typeof upcomingProgress === "number"
      ? Math.round(Math.max(0, Math.min(upcomingProgress, 1)) * 100)
      : 0;
  const showTrailingStatus = showLiveStop || showSpinner;

  return (
    <div
      data-sidebar-timeline-session-id={timelineSessionId}
      className="group/sidebar-live-item relative"
    >
      <InteractiveButton
        onClick={onClick}
        onDoubleClick={onDoubleClick}
        onCmdClick={onCmdClick}
        onShiftClick={onShiftClick}
        onDragStart={onDragStart}
        contextMenu={hasSelection ? undefined : contextMenu}
        className={cn([
          "w-full rounded-lg px-3 py-1.5 text-left",
          showUpcomingGauge && "pl-4",
          showTrailingStatus && "pr-10",
          "cursor-pointer",
          multiSelected && "bg-accent",
          !multiSelected && selected && "bg-accent",
          !multiSelected && !selected && "hover:bg-accent/50",
          isUpcoming &&
            !isLive && [
              "bg-destructive/8 text-foreground",
              "focus-visible:ring-destructive/25",
            ],
          isLive && [
            "bg-destructive text-destructive-foreground hover:bg-destructive/90",
            "focus-visible:ring-destructive/40 focus-visible:ring-2 focus-visible:outline-hidden",
          ],
          muted && !isLive && !isUpcoming && "opacity-65",
        ])}
        draggable={draggable}
      >
        <div className="flex min-w-0 items-center gap-2">
          <div className="pointer-events-none min-w-0 flex-1 truncate text-sm font-normal">
            {title || t`Untitled`}
          </div>
          {author && (
            <span
              title={t`Written by ${author}`}
              className={cn([
                "flex shrink-0 items-center",
                isLive
                  ? "text-destructive-foreground/65"
                  : "text-muted-foreground/70",
              ])}
            >
              <BotIcon
                aria-label={t`Written by ${author}`}
                className="size-3"
              />
            </span>
          )}
          {displayTime && (
            <div
              className={cn([
                "timecode shrink-0",
                isLive
                  ? "text-destructive-foreground/65"
                  : "text-muted-foreground/70",
              ])}
            >
              {displayTime}
            </div>
          )}
        </div>
      </InteractiveButton>
      {showUpcomingGauge ? (
        <div
          aria-hidden
          data-sidebar-timeline-upcoming-gauge
          className="bg-destructive/20 pointer-events-none absolute top-2 bottom-2 left-1.5 w-0.5 overflow-hidden rounded-full"
        >
          <div
            data-sidebar-timeline-upcoming-gauge-fill
            className="bg-destructive absolute bottom-0 left-0 w-full rounded-full transition-[height] duration-300 ease-linear"
            style={{ height: `${upcomingGaugePercent}%` }}
          />
        </div>
      ) : null}
      {showSpinner ? (
        <div
          aria-hidden
          className="text-muted-foreground pointer-events-none absolute top-1/2 right-3 flex size-5 -translate-y-1/2 items-center justify-center"
        >
          <Spinner size={14} />
        </div>
      ) : null}
      {showLiveStop ? (
        <button
          type="button"
          aria-label={t`Stop listening`}
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            onStop();
          }}
          className={cn([
            "absolute top-1/2 right-3 flex size-5 -translate-y-1/2 items-center justify-center rounded-sm",
            "text-destructive-foreground/80 hover:bg-destructive-foreground/15 hover:text-destructive-foreground transition-colors",
            "focus-visible:ring-destructive-foreground/70 focus-visible:ring-2 focus-visible:outline-hidden",
          ])}
        >
          <span
            aria-hidden
            className="flex items-center justify-center group-hover/sidebar-live-item:hidden"
          >
            <DancingSticks
              amplitude={amplitude ?? 0.25}
              color="currentColor"
              height={14}
              width={13}
              stickWidth={2}
              gap={2}
            />
          </span>
          <span
            aria-hidden
            className="hidden items-center justify-center group-hover/sidebar-live-item:flex"
          >
            <SquareIcon size={10} className="fill-current" />
          </span>
        </button>
      ) : null}
    </div>
  );
}

const SessionItem = memo(
  ({
    item,
    precision,
    selected,
    timezone,
    multiSelected,
    getFlatItemKeys,
    isUpcoming,
    upcomingProgress,
    isEnhancing = false,
  }: {
    item: TimelineItem;
    precision: TimelinePrecision;
    selected: boolean;
    timezone?: string;
    multiSelected: boolean;
    getFlatItemKeys: () => string[];
    isUpcoming?: boolean;
    upcomingProgress?: number;
    isEnhancing?: boolean;
  }) => {
    const { t } = useLingui();
    const openCurrent = useTabs((state) => state.openCurrent);
    const deleteSession = useDeleteSession();

    const sessionId = item.id;
    const title = useSessionTitle(sessionId, item.data.title ?? undefined);

    const { sessionMode, stop, amplitude } = useListener((state) => {
      const sessionMode = state.getSessionMode(sessionId);
      return {
        sessionMode,
        stop: state.stop,
        amplitude: sessionMode === "active" ? state.live.amplitude : null,
      };
    });
    const isLive = sessionMode === "active";
    const isFinalizing = sessionMode === "finalizing";
    const isBatching = sessionMode === "running_batch";
    const showSpinner =
      !selected && !isLive && (isFinalizing || isEnhancing || isBatching);

    const displayTime = useMemo(
      () => formatDisplayTime(item.data.created_at, precision, timezone),
      [item.data.created_at, precision, timezone],
    );
    const muted = isTimelineItemInFuture(item);

    const itemKey = `session-${item.id}`;

    const handleClick = useCallback(() => {
      useTimelineSelection.getState().setAnchor(itemKey);
      openCurrent({ id: sessionId, type: "sessions" });
    }, [sessionId, openCurrent, itemKey]);

    const handleCmdClick = useCallback(() => {
      useTimelineSelection.getState().toggleSelect(itemKey);
    }, [itemKey]);

    const handleShiftClick = useCallback(() => {
      useTimelineSelection.getState().selectRange(getFlatItemKeys(), itemKey);
    }, [getFlatItemKeys, itemKey]);

    const handleOpenStandaloneWindow = useCallback(() => {
      void openStandaloneNoteWindow(sessionId);
    }, [sessionId]);

    const handleDragStart = useCallback(
      (event: DragEvent<HTMLElement>) => {
        writeSessionContextDragData(
          event.dataTransfer,
          sessionId,
          title || t`Untitled`,
        );
      },
      [sessionId, title, t],
    );

    const handleDelete = useCallback(() => {
      deleteSession(sessionId, { title });
    }, [deleteSession, sessionId, title]);

    const handleShowInFinder = useCallback(async () => {
      const result = await fsSyncCommands.sessionDir(sessionId);
      if (result.status === "ok") {
        await openerCommands.openPath(result.data, null);
      }
    }, [sessionId]);

    const handleRenameFolder = useCallback(async () => {
      const result = await commands.sessionRenameDirToTitle(sessionId);
      if (result.status === "ok") {
        sonnerToast.success(t`Folder renamed`, { description: result.data });
      } else {
        sonnerToast.error(t`Could not rename the folder`, {
          description: result.error,
        });
      }
    }, [sessionId, t]);

    const recordingHoldsFolder = isLive || isFinalizing;

    const contextMenu = useMemo(
      () => [
        {
          id: "open-new-window",
          text: t`Open in New Window`,
          action: handleOpenStandaloneWindow,
        },
        {
          id: "show",
          text:
            platform() === "windows"
              ? t`Show in File Explorer`
              : t`Show in Finder`,
          action: handleShowInFinder,
        },
        {
          id: "rename-folder",
          text: t`Rename Folder to Match Title`,
          action: handleRenameFolder,
          disabled: recordingHoldsFolder,
        },
        { separator: true as const },
        {
          id: "delete",
          text: t`Delete Note`,
          action: handleDelete,
        },
      ],
      [
        handleOpenStandaloneWindow,
        handleShowInFinder,
        handleRenameFolder,
        recordingHoldsFolder,
        handleDelete,
        t,
      ],
    );

    return (
      <ItemBase
        title={title}
        displayTime={displayTime}
        author={item.data.author ?? null}
        isLive={isLive}
        amplitude={Math.max(
          0.25,
          Math.min(Math.hypot(amplitude?.mic ?? 0, amplitude?.speaker ?? 0), 1),
        )}
        showSpinner={showSpinner}
        selected={selected}
        muted={muted}
        multiSelected={multiSelected}
        onClick={handleClick}
        onDoubleClick={handleOpenStandaloneWindow}
        onCmdClick={handleCmdClick}
        onShiftClick={handleShiftClick}
        onStop={stop}
        onDragStart={handleDragStart}
        contextMenu={contextMenu}
        timelineSessionId={sessionId}
        isUpcoming={isUpcoming}
        upcomingProgress={upcomingProgress}
        draggable
      />
    );
  },
);

function formatDisplayTime(
  timestamp: string | null | undefined,
  precision: TimelinePrecision,
  timezone?: string,
): string {
  const parsed = safeParseDate(timestamp);
  if (!parsed) {
    return "";
  }

  const date = timezone ? new TZDate(parsed, timezone) : parsed;

  if (precision === "time") {
    return "";
  }

  const now = timezone ? new TZDate(new Date(), timezone) : new Date();
  const sameYear = getYear(date) === getYear(now);
  return sameYear ? format(date, "MMM d") : format(date, "MMM d, yyyy");
}

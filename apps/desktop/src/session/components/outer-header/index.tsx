import { useLingui } from "@lingui/react/macro";
import { SquareIcon } from "lucide-react";
import { useCallback } from "react";

import { Spinner } from "@hypr/ui/components/ui/spinner";
import { cn } from "@hypr/utils";

import { RecordingIcon, useHasTranscript } from "../shared";
import { OverflowButton } from "./overflow";

import { useAudioPlayer } from "~/audio-player";
import { useShell } from "~/contexts/shell";
import type { EditorView } from "~/store/zustand/tabs/schema";
import { useListener } from "~/stt/contexts";
import { useStartListening } from "~/stt/useStartListening";
import {
  isMainWebviewWindow,
  requestMainListenerControl,
} from "~/stt/window-control";

export function OuterHeader({
  sessionId,
  currentView,
  standaloneWindow = false,
  title,
  centerTitle = false,
}: {
  sessionId: string;
  currentView: EditorView;
  standaloneWindow?: boolean;
  title?: React.ReactNode;
  centerTitle?: boolean;
}) {
  const { leftsidebar } = useShell();
  const sessionMode = useListener((state) => state.getSessionMode(sessionId));
  const showSidebarTimelineHeaderGutter =
    !standaloneWindow && !leftsidebar.expanded;
  const showExpandedSidebarTimelineHeader = leftsidebar.expanded;

  return (
    <div
      data-tauri-drag-region
      className={cn([
        "relative flex w-full items-center",
        showSidebarTimelineHeaderGutter
          ? "h-[calc(var(--sidebar-chrome-center-y)*2)]"
          : "h-12",
        showSidebarTimelineHeaderGutter &&
          "pl-[calc(var(--traffic-lights-inset)_+_80px)]",
      ])}
    >
      {title ? (
        <div
          data-tauri-drag-region
          className={cn([
            "pointer-events-none absolute inset-y-0 flex items-center",
            "@container",
            centerTitle && "justify-center",
            "right-[140px]",
            standaloneWindow
              ? "left-[var(--traffic-lights-inset)]"
              : showSidebarTimelineHeaderGutter
                ? "left-[calc(var(--traffic-lights-inset)_+_28px)]"
                : showExpandedSidebarTimelineHeader
                  ? "left-0"
                  : "left-[calc(var(--traffic-lights-inset)_+_38px)]",
          ])}
        >
          <div
            data-tauri-drag-region
            className="pointer-events-auto max-w-full min-w-0"
          >
            {title}
          </div>
        </div>
      ) : null}
      <div
        data-tauri-drag-region
        className="relative z-10 ml-auto flex shrink-0 items-center gap-0 pr-1"
      >
        <HeaderMeetingControl sessionId={sessionId} sessionMode={sessionMode} />
        <OverflowButton
          standaloneWindow={standaloneWindow}
          sessionId={sessionId}
          currentView={currentView}
        />
      </div>
    </div>
  );
}

function HeaderMeetingControl({
  sessionId,
  sessionMode,
}: {
  sessionId: string;
  sessionMode: string;
}) {
  const startListening = useStartListening(sessionId);
  const { stop, stopTranscription } = useListener((state) => ({
    stop: state.stop,
    stopTranscription: state.stopTranscription,
  }));
  const hasTranscript = useHasTranscript(sessionId);
  const { audioExists } = useAudioPlayer();
  const canResume = audioExists || hasTranscript;
  const { t } = useLingui();
  const start = useCallback(() => {
    if (!isMainWebviewWindow()) {
      void requestMainListenerControl("start", sessionId);
      return;
    }

    void startListening();
  }, [sessionId, startListening]);
  const stopListening = useCallback(() => {
    if (!isMainWebviewWindow()) {
      void requestMainListenerControl("stop", sessionId);
      return;
    }

    stop();
  }, [sessionId, stop]);
  const action = (() => {
    if (sessionMode === "active") {
      return {
        label: t`Stop`,
        title: t`Stop listening`,
        icon: <SquareIcon className="text-recording size-3 fill-current" />,
        onClick: stopListening,
      };
    }

    if (sessionMode === "running_batch") {
      return {
        label: t`Transcribing`,
        title: t`Stop transcription`,
        icon: <Spinner size={12} />,
        onClick: () => {
          void stopTranscription(sessionId);
        },
      };
    }

    if (sessionMode === "finalizing") {
      return {
        label: t`Finalizing`,
        title: t`Finalizing transcript`,
        icon: <Spinner size={12} />,
        onClick: undefined,
      };
    }

    return {
      label: canResume ? t`Resume` : t`Record`,
      title: canResume ? t`Resume listening` : t`Record`,
      icon: <RecordingIcon />,
      onClick: start,
    };
  })();
  const disabled = sessionMode === "finalizing";

  return (
    <div className="mr-1 flex min-w-0 shrink-0 items-center gap-2">
      <button
        type="button"
        data-tauri-drag-region="false"
        aria-label={action.label}
        title={action.title}
        disabled={disabled}
        onClick={action.onClick}
        className={cn([
          "border-border bg-card text-foreground flex h-7 max-w-56 min-w-0 shrink-0 items-center gap-1.5 rounded-md border px-2.5 py-0",
          "text-sm font-medium",
          "hover:bg-accent transition-none",
          disabled && "hover:bg-card cursor-default opacity-60",
        ])}
      >
        {action.icon}
        <span className="truncate">{action.label}</span>
      </button>
    </div>
  );
}

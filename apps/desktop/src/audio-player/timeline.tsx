import { Pause, Play } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { cn } from "@hypr/utils";

import { useAudioPlayer, useAudioTime } from "./provider";
import { TimelineMeta, TimelineShell } from "./timeline-shell";

import { useNativeContextMenu } from "~/shared/hooks/useNativeContextMenu";

const PLAYBACK_RATES = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2];

export function CompactPlayButton() {
  const { state, pause, resume, start } = useAudioPlayer();

  const handleClick = () => {
    if (state === "playing") {
      pause();
    } else if (state === "paused") {
      resume();
    } else if (state === "stopped") {
      start();
    }
  };

  return (
    <button
      onClick={handleClick}
      aria-label={state === "playing" ? "Pause recording" : "Play recording"}
      className={cn([
        "flex items-center justify-center",
        "h-6 w-6 rounded-full",
        "bg-foreground",
        "transition-none hover:opacity-85",
        "shrink-0 select-none",
      ])}
    >
      {state === "playing" ? (
        <Pause className="text-background h-3 w-3" fill="currentColor" />
      ) : (
        <Play className="text-background ml-px h-3 w-3" fill="currentColor" />
      )}
    </button>
  );
}

export function Timeline({
  contentClassName,
}: {
  contentClassName?: string;
} = {}) {
  const isPro = true;
  const {
    registerContainer,
    state,
    pause,
    resume,
    start,
    stop,
    playbackRate,
    setPlaybackRate,
    deleteRecording,
    isDeletingRecording,
  } = useAudioPlayer();
  const time = useAudioTime();
  const [showRateMenu, setShowRateMenu] = useState(false);
  const rateMenuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (
        rateMenuRef.current &&
        !rateMenuRef.current.contains(e.target as Node)
      ) {
        setShowRateMenu(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  const handleClick = () => {
    if (state === "playing") {
      pause();
    } else if (state === "paused") {
      resume();
    } else if (state === "stopped") {
      start();
    }
  };

  const handleDeleteRecording = useCallback(async () => {
    setShowRateMenu(false);
    await deleteRecording();
  }, [deleteRecording]);

  const contextMenu = useMemo(
    () => [
      ...(state === "paused"
        ? [{ id: "resume", text: "Resume", action: resume }]
        : []),
      ...(state === "stopped"
        ? [{ id: "play", text: "Play", action: start }]
        : []),
      ...(state === "playing"
        ? [{ id: "pause", text: "Pause", action: pause }]
        : []),
      ...(state !== "stopped"
        ? [{ id: "stop", text: "Stop", action: stop }]
        : []),
      { separator: true as const },
      {
        id: "delete-recording",
        text: "Delete recording",
        action: () => void handleDeleteRecording(),
        disabled: isDeletingRecording,
      },
    ],
    [
      state,
      resume,
      start,
      pause,
      stop,
      isDeletingRecording,
      handleDeleteRecording,
    ],
  );
  const showContextMenu = useNativeContextMenu(contextMenu);

  return (
    <TimelineShell
      contentClassName={contentClassName}
      onContextMenu={showContextMenu}
      leading={
        <button
          onClick={handleClick}
          className={cn([
            "flex items-center justify-center",
            "h-5 w-5 rounded-full",
            "bg-foreground",
            "transition-none hover:opacity-85",
            "shrink-0 select-none",
          ])}
        >
          {state === "playing" ? (
            <Pause
              className="text-background h-2.5 w-2.5"
              fill="currentColor"
            />
          ) : (
            <Play
              className="text-background ml-px h-2.5 w-2.5"
              fill="currentColor"
            />
          )}
        </button>
      }
      meta={
        <>
          <TimelineMeta>
            <span>{formatTime(time.current)}</span>/
            <span>{formatTime(time.total)}</span>
          </TimelineMeta>

          {isPro ? (
            <div className="relative shrink-0" ref={rateMenuRef}>
              <button
                onClick={() => setShowRateMenu((prev) => !prev)}
                className={cn([
                  "flex items-center justify-center",
                  "h-5 rounded px-1",
                  "hover:bg-accent transition-none",
                  "text-muted-foreground/80 font-mono text-[10px] select-none",
                ])}
              >
                {playbackRate}x
              </button>
              {showRateMenu && (
                <div
                  className={cn([
                    "absolute right-0 bottom-full mb-1",
                    "border-border bg-card rounded-lg border shadow-md",
                    "z-50 py-1",
                  ])}
                >
                  {PLAYBACK_RATES.map((rate) => (
                    <button
                      key={rate}
                      onClick={() => {
                        setPlaybackRate(rate);
                        setShowRateMenu(false);
                      }}
                      className={cn([
                        "block w-full px-3 py-1 text-left font-mono text-xs select-none",
                        "hover:bg-accent transition-none",
                        rate === playbackRate
                          ? "text-foreground font-semibold"
                          : "text-muted-foreground",
                      ])}
                    >
                      {rate}x
                    </button>
                  ))}
                </div>
              )}
            </div>
          ) : null}
        </>
      }
      main={
        <div
          ref={registerContainer}
          className="h-5 min-w-0 flex-1"
          style={{ width: "100%" }}
        />
      }
    />
  );
}

function formatTime(seconds: number): string {
  const mins = Math.floor(seconds / 60);
  const secs = Math.floor(seconds % 60);
  return `${mins.toString().padStart(2, "0")}:${secs.toString().padStart(2, "0")}`;
}

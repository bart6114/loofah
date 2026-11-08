import {
  DEFAULT_FLOATING_OVERLAY_SETTINGS,
  getFloatingOverlaySettings,
  getLiveCaptionDisplayText,
  type FloatingOverlaySettings,
  type LiveCaptionPosition,
} from "./overlay-settings";
export { getLiveCaptionDisplayText } from "./overlay-settings";
const FLOATING_TRANSCRIPT_OVERLAP_THRESHOLD_MS = 300;
import { useRef } from "react";

import {
  commands as windowsCommands,
  events as windowsEvents,
} from "@hypr/plugin-windows";

import {
  createMeetingFloatLabelContext,
  loadMeetingFloatData,
  type MeetingFloatData,
  subscribeMeetingFloatData,
} from "./hooks";
import { getSettingsValuesFromNativeChange } from "./settings-change";

import {
  getStoredSettingValues,
  setSettingValue,
  useSetSettingValues,
} from "~/settings/queries";
import { useConfigValue, useConfigValues } from "~/shared/config";
import { useLatestRef } from "~/shared/hooks/useLatestRef";
import { useMountEffect } from "~/shared/hooks/useMountEffect";
import { listenerStore } from "~/store/zustand/listener/instance";
import { SegmentKeyUtils, type RenderLabelContext } from "~/stt/live-segment";

type ListenerState = ReturnType<typeof listenerStore.getState>;
type FloatingBarStatus = "recording" | "error";
type FloatingBarColorScheme = "light" | "dark";
type FloatingTranscriptBubble = {
  id: string;
  speakerLabel: string;
  text: string;
  isSelf: boolean;
  isFinal: boolean;
  startMs: number;
  endMs: number;
  overlapsPrevious: boolean;
  overlapsNext: boolean;
};
type FloatingRouteState = {
  sessionId: string;
  title: string;
  amplitude: number;
  status: FloatingBarStatus;
  colorScheme: FloatingBarColorScheme;
  opacity: number;
  liveCaptionOpacity: number;
  liveCaptionWidth: number;
  liveCaptionLineCount: number;
  liveCaptionPosition: LiveCaptionPosition;
  liveCaptionMinimized: boolean;
  liveCaptionToggleVisible: boolean;
  transcriptBubbles: FloatingTranscriptBubble[];
};
type LiveCaptionRouteState = {
  sessionId: string;
  text: string;
  opacity: number;
  width: number;
  lineCount: number;
  position: LiveCaptionPosition;
  minimized: boolean;
};

const FLOATING_OVERLAY_SETTING_KEYS = [
  "floating_bar_opacity",
  "live_caption_opacity",
  "live_caption_width",
  "live_caption_line_count",
  "live_caption_position",
  "live_caption_minimized",
] as const;

export function FloatingMeetingWindowHost() {
  const floatingBarEnabled = useConfigValue("floating_bar_enabled");
  const storedSettings = useConfigValues(FLOATING_OVERLAY_SETTING_KEYS);
  const overlaySettings = getFloatingOverlaySettings(storedSettings);

  return (
    <>
      <FloatingOverlaySettingsEventSync />
      <LiveCaptionDefaultVisibilitySync />
      {floatingBarEnabled ? (
        <FloatingMeetingWindowSync settings={overlaySettings} />
      ) : (
        <FloatingMeetingWindowDisabled />
      )}
      <LiveCaptionWindowDisabled />
    </>
  );
}

function FloatingOverlaySettingsEventSync() {
  const setSettingValues = useSetSettingValues();

  useMountEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    windowsEvents.floatingBarSettingsChange
      .listen((event) => {
        if (cancelled) {
          return;
        }

        const values = getSettingsValuesFromNativeChange(event.payload);
        if (Object.keys(values).length === 0) {
          return;
        }

        setSettingValues(values);
      })
      .then((nextUnlisten) => {
        if (cancelled) {
          nextUnlisten();
          return;
        }

        unlisten = nextUnlisten;
      });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  });

  return null;
}

function LiveCaptionDefaultVisibilitySync() {
  useMountEffect(() => {
    let appliedSessionId: string | null = null;

    const applyDefaultVisibility = (state: ListenerState) => {
      if (state.live.status !== "active" || !state.live.sessionId) {
        appliedSessionId = null;
        return;
      }

      if (appliedSessionId === state.live.sessionId) {
        return;
      }

      appliedSessionId = state.live.sessionId;
      void setSettingValue("live_caption_minimized", true);
    };

    applyDefaultVisibility(listenerStore.getState());

    const unsubscribe = listenerStore.subscribe((state) => {
      applyDefaultVisibility(state);
    });

    return () => {
      unsubscribe();
    };
  });

  return null;
}
function FloatingMeetingWindowDisabled() {
  useMountEffect(() => {
    void hideFloatingMeetingPanel();
  });

  return null;
}

function LiveCaptionWindowDisabled() {
  useMountEffect(() => {
    void hideLiveCaptionPanel();
  });

  return null;
}

function FloatingMeetingWindowSync({
  settings,
}: {
  settings: FloatingOverlaySettings;
}) {
  const settingsRef = useLatestRef(settings);
  const refreshSettingsRef = useRef<() => void>(() => {});

  useMountEffect(() => {
    let meetingData: MeetingFloatData = { sessions: {} };
    let routeState = getCurrentFloatingRouteState(
      listenerStore.getState(),
      undefined,
      settingsRef.current,
      getFloatingLiveCaptionToggleVisible(listenerStore.getState()),
      meetingData,
    );
    let syncQueued = false;
    let cancelled = false;
    let shownSessionId: string | null = null;
    let nativeCommandsUnavailable = false;
    let unsubscribeMeetingData: (() => Promise<void>) | null = null;
    const unlisteners: Array<() => void> = [];

    const shouldContinue = () => !cancelled;
    const updateRouteState = (nextRouteState: FloatingRouteState | null) => {
      if (isSameFloatingRouteState(nextRouteState, routeState)) {
        return;
      }

      routeState = nextRouteState;
      scheduleSync();
    };
    const refreshCurrentRouteState = () => {
      updateRouteState(
        getCurrentFloatingRouteState(
          listenerStore.getState(),
          undefined,
          settingsRef.current,
          getFloatingLiveCaptionToggleVisible(listenerStore.getState()),
          meetingData,
        ),
      );
    };
    refreshSettingsRef.current = refreshCurrentRouteState;

    const sync = async () => {
      if (!shouldContinue()) {
        return;
      }

      if (nativeCommandsUnavailable && routeState) {
        return;
      }

      const nextShownSessionId = await syncFloatingMeetingWindow(
        routeState,
        shownSessionId,
        shouldContinue,
      );
      if (!shouldContinue()) {
        await hideFloatingMeetingPanel();
        return;
      }

      if (nextShownSessionId === "unavailable") {
        nativeCommandsUnavailable = true;
        return;
      }

      shownSessionId = nextShownSessionId;
    };

    const scheduleSync = () => {
      if (syncQueued) {
        return;
      }

      syncQueued = true;
      queueMicrotask(() => {
        syncQueued = false;
        if (cancelled) {
          return;
        }

        void sync();
      });
    };

    windowsEvents.floatingBarStop
      .listen(({ payload }) => {
        if (payload.capture_key) return;
        void hideFloatingMeetingPanel();
        listenerStore.getState().stop();
      })
      .then((unlisten) => {
        if (cancelled) {
          unlisten();
          return;
        }

        unlisteners.push(unlisten);
      });

    windowsEvents.floatingBarOpenMain
      .listen(async () => {
        await windowsCommands.windowShow({ type: "main" });
      })
      .then((unlisten) => {
        if (cancelled) {
          unlisten();
          return;
        }

        unlisteners.push(unlisten);
      });

    scheduleSync();

    const unsubscribe = listenerStore.subscribe((state, previousState) => {
      const colorScheme = getCurrentFloatingBarColorScheme();
      const nextRouteState = getFloatingRouteState(state, {
        colorScheme,
        settings: settingsRef.current,
        liveCaptionToggleVisible: getFloatingLiveCaptionToggleVisible(state),
        sessionTitle: getFloatingSessionTitle(state, meetingData),
        speakerLabelContext: getFloatingSpeakerLabelContext(state, meetingData),
      });
      const previousRouteState = getFloatingRouteState(previousState, {
        colorScheme,
        settings: settingsRef.current,
        liveCaptionToggleVisible:
          getFloatingLiveCaptionToggleVisible(previousState),
        sessionTitle: getFloatingSessionTitle(previousState, meetingData),
        speakerLabelContext: getFloatingSpeakerLabelContext(
          previousState,
          meetingData,
        ),
      });

      if (!isSameFloatingRouteState(nextRouteState, previousRouteState)) {
        updateRouteState(nextRouteState);
      }
    });

    void subscribeMeetingFloatData(
      (nextData) => {
        meetingData = nextData;
        refreshCurrentRouteState();
      },
      (error) => {
        console.error("Failed to read floating meeting data:", error);
      },
    )
      .then((unsubscribe) => {
        if (cancelled) {
          void unsubscribe();
        } else {
          unsubscribeMeetingData = unsubscribe;
        }
      })
      .catch((error) => {
        console.error("Failed to subscribe to floating meeting data:", error);
      });

    const unsubscribeAppliedTheme = subscribeToAppliedTheme(() => {
      refreshCurrentRouteState();
    });

    return () => {
      cancelled = true;
      refreshSettingsRef.current = () => {};
      unsubscribe();
      unsubscribeAppliedTheme();
      void unsubscribeMeetingData?.();
      unlisteners.forEach((unlisten) => unlisten());
      void hideFloatingMeetingPanel();
    };
  });

  return (
    <FloatingMeetingWindowSettingsSync
      key={JSON.stringify(settings)}
      onSettingsChange={() => refreshSettingsRef.current()}
    />
  );
}

function FloatingMeetingWindowSettingsSync({
  onSettingsChange,
}: {
  onSettingsChange: () => void;
}) {
  useMountEffect(onSettingsChange);
  return null;
}

export function getFloatingRouteState(
  state: ListenerState,
  {
    sessionId,
    colorScheme = "dark",
    settings = DEFAULT_FLOATING_OVERLAY_SETTINGS,
    liveCaptionToggleVisible = false,
    sessionTitle,
    speakerLabelContext,
  }: {
    sessionId?: string;
    colorScheme?: FloatingBarColorScheme;
    settings?: FloatingOverlaySettings;
    liveCaptionToggleVisible?: boolean;
    sessionTitle?: string | null;
    speakerLabelContext?: RenderLabelContext;
  } = {},
): FloatingRouteState | null {
  if (state.live.status !== "active") {
    return null;
  }

  if (!state.live.sessionId) {
    return null;
  }

  if (sessionId && state.live.sessionId !== sessionId) {
    return null;
  }

  return {
    sessionId: state.live.sessionId,
    title: getFloatingTitle(sessionTitle),
    amplitude: Math.min(
      Math.hypot(state.live.amplitude.mic, state.live.amplitude.speaker),
      1,
    ),
    status: state.live.degraded || state.live.lastError ? "error" : "recording",
    colorScheme,
    opacity: settings.floatingBarOpacity,
    liveCaptionOpacity: settings.liveCaptionOpacity,
    liveCaptionWidth: settings.liveCaptionWidth,
    liveCaptionLineCount: settings.liveCaptionLineCount,
    liveCaptionPosition: settings.liveCaptionPosition,
    liveCaptionMinimized: settings.liveCaptionMinimized,
    liveCaptionToggleVisible,
    transcriptBubbles: getFloatingTranscriptBubbles(
      state.liveSegments,
      speakerLabelContext,
    ),
  };
}

function getCurrentFloatingRouteState(
  state: ListenerState,
  sessionId?: string,
  settings: FloatingOverlaySettings = DEFAULT_FLOATING_OVERLAY_SETTINGS,
  liveCaptionToggleVisible = false,
  meetingData?: MeetingFloatData,
): FloatingRouteState | null {
  return getFloatingRouteState(state, {
    sessionId,
    colorScheme: getCurrentFloatingBarColorScheme(),
    settings,
    liveCaptionToggleVisible,
    sessionTitle: getFloatingSessionTitle(state, meetingData),
    speakerLabelContext: getFloatingSpeakerLabelContext(state, meetingData),
  });
}

function getFloatingSessionTitle(
  state: ListenerState,
  meetingData: MeetingFloatData | undefined,
) {
  const sessionId = state.live.sessionId;
  if (!sessionId) {
    return null;
  }

  return meetingData?.sessions[sessionId]?.title ?? null;
}

function getFloatingTitle(title: string | null | undefined) {
  const normalized = title?.trim();
  return normalized || "Live transcript";
}

export function getFloatingTranscriptBubbles(
  segments: ListenerState["liveSegments"],
  speakerLabelContext?: RenderLabelContext,
): FloatingTranscriptBubble[] {
  const bubbles = segments
    .slice()
    .sort(
      (a, b) =>
        a.start_ms - b.start_ms ||
        a.end_ms - b.end_ms ||
        a.id.localeCompare(b.id),
    )
    .map((segment) => {
      const text = getFloatingSegmentText(segment);
      if (!text) {
        return null;
      }

      return {
        id: segment.id,
        speakerLabel: getFloatingSpeakerLabel(segment.key, speakerLabelContext),
        text,
        isSelf: isFloatingSelfSpeaker(segment.key),
        isFinal: segment.words.every((word) => word.is_final),
        startMs: segment.start_ms,
        endMs: segment.end_ms,
        overlapsPrevious: false,
        overlapsNext: false,
      };
    })
    .filter((bubble): bubble is FloatingTranscriptBubble => bubble !== null);

  return bubbles.map((bubble, index) => ({
    ...bubble,
    overlapsPrevious: bubbles
      .slice(0, index)
      .some((previous) => doFloatingTranscriptBubblesOverlap(previous, bubble)),
    overlapsNext: bubbles
      .slice(index + 1)
      .some((next) => doFloatingTranscriptBubblesOverlap(bubble, next)),
  }));
}

function getFloatingSegmentText(
  segment: ListenerState["liveSegments"][number],
) {
  const wordText = segment.words
    .map((word) => word.text.trim())
    .filter(Boolean)
    .join(" ")
    .replace(/\s+([,.?!;:])/g, "$1");

  return (wordText || segment.text).trim().replace(/\s+/g, " ");
}

function getFloatingSpeakerLabel(
  key: ListenerState["liveSegments"][number]["key"],
  ctx?: RenderLabelContext,
) {
  if (isFloatingSelfSpeaker(key)) {
    return "You";
  }

  if (ctx) {
    return SegmentKeyUtils.renderLabel(key, ctx);
  }

  if (key.speaker_index != null) {
    return `Speaker ${key.speaker_index + 1}`;
  }

  if (key.channel === "RemoteParty") {
    return "Speaker";
  }

  return "Audio";
}

function getFloatingSpeakerLabelContext(
  state: ListenerState,
  meetingData: MeetingFloatData | undefined,
): RenderLabelContext | undefined {
  if (!meetingData || !state.live.sessionId) {
    return undefined;
  }

  return createMeetingFloatLabelContext(meetingData, state.live.sessionId);
}

function isFloatingSelfSpeaker(
  key: ListenerState["liveSegments"][number]["key"],
) {
  return key.channel === "DirectMic";
}

function doFloatingTranscriptBubblesOverlap(
  left: FloatingTranscriptBubble,
  right: FloatingTranscriptBubble,
) {
  if (
    left.isSelf === right.isSelf &&
    left.speakerLabel === right.speakerLabel
  ) {
    return false;
  }

  const overlapMs =
    Math.min(left.endMs, right.endMs) - Math.max(left.startMs, right.startMs);
  return overlapMs >= FLOATING_TRANSCRIPT_OVERLAP_THRESHOLD_MS;
}

export function shouldShowFloatingLiveCaptionToggle({
  liveTranscriptionActive,
}: {
  provider?: string | null;
  model?: string | null;
  liveTranscriptionActive: boolean;
}) {
  return liveTranscriptionActive;
}

function getFloatingLiveCaptionToggleVisible(state: ListenerState) {
  return shouldShowFloatingLiveCaptionToggle({
    liveTranscriptionActive: state.live.liveTranscriptionActive === true,
  });
}

export function getLiveCaptionRouteState(
  state: ListenerState,
  settings: FloatingOverlaySettings = DEFAULT_FLOATING_OVERLAY_SETTINGS,
): LiveCaptionRouteState | null {
  if (state.live.status !== "active") {
    return null;
  }

  if (!state.live.sessionId) {
    return null;
  }

  if (state.live.liveTranscriptionActive !== true) {
    return null;
  }

  if (settings.liveCaptionMinimized) {
    return null;
  }

  const text = getLiveCaptionDisplayText(state.liveCaptionText, settings);

  return {
    sessionId: state.live.sessionId,
    text,
    opacity: settings.liveCaptionOpacity,
    width: settings.liveCaptionWidth,
    lineCount: settings.liveCaptionLineCount,
    position: settings.liveCaptionPosition,
    minimized: settings.liveCaptionMinimized,
  };
}

function subscribeToAppliedTheme(onStoreChange: () => void) {
  if (
    typeof document === "undefined" ||
    typeof MutationObserver === "undefined"
  ) {
    return () => {};
  }

  const observer = new MutationObserver(onStoreChange);
  observer.observe(document.documentElement, {
    attributeFilter: ["class"],
    attributes: true,
  });
  return () => observer.disconnect();
}

export function getCurrentFloatingBarColorScheme(): FloatingBarColorScheme {
  if (typeof document === "undefined") {
    return "dark";
  }

  return document.documentElement.classList.contains("dark") ? "dark" : "light";
}

function isSameFloatingRouteState(
  left: FloatingRouteState | null,
  right: FloatingRouteState | null,
) {
  return (
    left?.sessionId === right?.sessionId &&
    left?.amplitude === right?.amplitude &&
    left?.status === right?.status &&
    left?.colorScheme === right?.colorScheme &&
    left?.opacity === right?.opacity &&
    left?.liveCaptionOpacity === right?.liveCaptionOpacity &&
    left?.liveCaptionWidth === right?.liveCaptionWidth &&
    left?.liveCaptionLineCount === right?.liveCaptionLineCount &&
    left?.liveCaptionPosition === right?.liveCaptionPosition &&
    left?.liveCaptionMinimized === right?.liveCaptionMinimized &&
    left?.liveCaptionToggleVisible === right?.liveCaptionToggleVisible &&
    left?.title === right?.title &&
    isSameFloatingTranscriptBubbles(
      left?.transcriptBubbles,
      right?.transcriptBubbles,
    )
  );
}

function isSameFloatingTranscriptBubbles(
  left: FloatingTranscriptBubble[] | undefined,
  right: FloatingTranscriptBubble[] | undefined,
) {
  if (left === right) {
    return true;
  }

  if (!left || !right || left.length !== right.length) {
    return false;
  }

  return left.every((bubble, index) => {
    const other = right[index];
    return (
      other &&
      bubble.id === other.id &&
      bubble.speakerLabel === other.speakerLabel &&
      bubble.text === other.text &&
      bubble.isSelf === other.isSelf &&
      bubble.isFinal === other.isFinal &&
      bubble.startMs === other.startMs &&
      bubble.endMs === other.endMs &&
      bubble.overlapsPrevious === other.overlapsPrevious &&
      bubble.overlapsNext === other.overlapsNext
    );
  });
}

async function syncFloatingMeetingWindow(
  routeState: FloatingRouteState | null,
  shownSessionId: string | null,
  shouldContinue: () => boolean,
): Promise<string | null | "unavailable"> {
  if (!shouldContinue()) {
    return null;
  }

  if (!routeState) {
    await hideFloatingMeetingPanel();
    return null;
  }

  const ready = await showFloatingMeetingWindow(
    routeState,
    shownSessionId !== routeState.sessionId,
    shouldContinue,
  );
  if (!shouldContinue()) {
    await hideFloatingMeetingPanel();
    return null;
  }

  return ready ? routeState.sessionId : "unavailable";
}

async function showFloatingMeetingWindow(
  routeState: FloatingRouteState,
  shouldShow: boolean,
  shouldContinue: () => boolean = () => true,
): Promise<boolean> {
  if (!shouldContinue()) {
    return false;
  }

  if (shouldShow) {
    const showResult = await windowsCommands.floatingBarShow();
    if (!shouldContinue()) {
      await hideFloatingMeetingPanel();
      return false;
    }

    if (showResult.status === "error") {
      console.error("Failed to show floating meeting panel:", showResult.error);
      return false;
    }
  }

  const updateResult = await windowsCommands.floatingBarUpdate({
    amplitude: routeState.amplitude,
    title: routeState.title,
    status: routeState.status,
    colorScheme: routeState.colorScheme,
    opacity: routeState.opacity,
    liveCaptionOpacity: routeState.liveCaptionOpacity,
    liveCaptionWidth: routeState.liveCaptionWidth,
    liveCaptionLineCount: routeState.liveCaptionLineCount,
    liveCaptionPosition: routeState.liveCaptionPosition,
    liveCaptionMinimized: routeState.liveCaptionMinimized,
    liveCaptionToggleVisible: routeState.liveCaptionToggleVisible,
    transcriptBubbles: routeState.transcriptBubbles,
  });
  if (!shouldContinue()) {
    await hideFloatingMeetingPanel();
    return false;
  }

  if (updateResult.status === "error") {
    console.error(
      "Failed to update floating meeting panel:",
      updateResult.error,
    );
    return false;
  }

  return true;
}

export async function openFloatingMeetingPanel({
  sessionId,
  enabled,
}: {
  sessionId?: string;
  enabled: boolean;
}) {
  if (!enabled) {
    await hideFloatingMeetingPanel();
    return;
  }

  const state = listenerStore.getState();
  const [{ values }, meetingData] = await Promise.all([
    getStoredSettingValues(),
    loadMeetingFloatData(),
  ]);
  const routeState = getCurrentFloatingRouteState(
    state,
    sessionId,
    getFloatingOverlaySettings(values),
    getFloatingLiveCaptionToggleVisible(state),
    meetingData,
  );

  if (!routeState) {
    return;
  }

  await showFloatingMeetingWindow(routeState, true);
}

export async function hideFloatingMeetingPanel() {
  const result = await windowsCommands.floatingBarHide();
  if (result.status === "error") {
    console.error("Failed to hide floating meeting panel:", result.error);
  }
}

export async function hideLiveCaptionPanel() {
  const result = await windowsCommands.liveCaptionHide();
  if (result.status === "error") {
    console.error("Failed to hide live caption panel:", result.error);
  }
}

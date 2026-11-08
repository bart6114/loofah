import type { ListenerStore } from "~/store/zustand/listener";

export function legacyActivityFixture(
  state: Partial<ReturnType<ListenerStore["getState"]>> = {},
  id = "",
) {
  return {
    mode: state.getSessionMode?.(id) ?? "inactive",
    starting: state.live?.sessionId === id && !!state.live?.loading,
    batchError: state.batch?.[id]?.error,
    hasLiveSegments:
      state.live?.sessionId === id && !!state.liveSegments?.length,
    amplitude: state.live?.amplitude,
    degraded: state.live?.degraded,
    liveStatus: state.live?.status ?? "inactive",
    liveSessionId: state.live?.sessionId ?? null,
    available: true,
    isAvailable: () => true,
    listener: { getState: () => state },
    activity: { captureState: () => "idle" },
  };
}

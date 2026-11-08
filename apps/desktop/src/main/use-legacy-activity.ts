import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useSyncExternalStore,
} from "react";
import { shallow } from "zustand/shallow";

import { useMainHost } from "./host-context";

import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import type { ListenerStore } from "~/store/zustand/listener";

type LegacyFeature =
  | "legacy_desktop_transcript_controls"
  | "legacy_desktop_enhance"
  | "legacy_desktop_session_controls";

export function useLegacyDesktopAccess() {
  const { scope, current: isCurrentScope } = useVaultReadScope();
  const host = useMainHost();
  const listener = host?.listenerStore;
  const activity = host?.activity;
  const lifetime = useMemo(
    () => ({ active: true, scope, listener, activity }),
    [scope, listener, activity],
  );
  useEffect(() => {
    lifetime.active = true;
    return () => {
      lifetime.active = false;
    };
  }, [lifetime]);
  const current = useCallback(
    (feature: LegacyFeature = "legacy_desktop_transcript_controls") =>
      lifetime.active &&
      isCurrentScope() &&
      host?.scope === lifetime.scope &&
      activity?.scope === lifetime.scope &&
      !!listener &&
      lifetime.scope.getSnapshot().state === "connected" &&
      lifetime.scope.client.canEdit &&
      lifetime.scope.client.features.includes(feature),
    [lifetime, isCurrentScope, host?.scope, activity, listener],
  );
  return {
    scope,
    listener,
    activity,
    isAvailable: current,
    available: current(),
  };
}

function sessionActivity(
  state: ReturnType<ListenerStore["getState"]> | null,
  id: string,
  includeAmplitude = false,
) {
  return {
    mode: state?.getSessionMode(id) ?? "inactive",
    starting: !!state && state.live.sessionId === id && state.live.loading,
    batchError: state?.batch[id]?.error,
    hasLiveSegments:
      !!state && state.live.sessionId === id && state.liveSegments.length > 0,
    amplitude:
      includeAmplitude && state?.live.sessionId === id
        ? state.live.amplitude
        : undefined,
    degraded: state?.live.sessionId === id ? state.live.degraded : null,
    liveSessionId: state?.live.sessionId ?? null,
    liveStatus: state?.live.status ?? "inactive",
  };
}

export function useLegacySessionActivity(
  sessionId: string,
  { includeAmplitude = false }: { includeAmplitude?: boolean } = {},
) {
  const access = useLegacyDesktopAccess();
  const { isAvailable: hasAccess, listener } = access;
  const identity = useRef(sessionId);
  identity.current = sessionId;
  const snapshot = useMemo(() => {
    let previous = sessionActivity(null, sessionId, includeAmplitude);
    return () => {
      const next = sessionActivity(
        hasAccess() ? listener!.getState() : null,
        sessionId,
        includeAmplitude,
      );
      if (!shallow(previous, next)) previous = next;
      return previous;
    };
  }, [hasAccess, listener, sessionId, includeAmplitude]);
  const subscribe = useCallback(
    (changed: () => void) => listener?.subscribe(changed) ?? (() => {}),
    [listener],
  );
  const state = useSyncExternalStore(subscribe, snapshot);
  const current = useCallback(
    (feature?: LegacyFeature) =>
      identity.current === sessionId && hasAccess(feature),
    [hasAccess, sessionId],
  );
  return { ...state, ...access, isAvailable: current };
}

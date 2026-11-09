import { emitTo, listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";

import { getCurrentWebviewWindowLabel } from "@hypr/plugin-windows";

import { useStartListening } from "./useStartListening";

import {
  useLegacyDesktopAccess,
  useLegacySessionActivity,
} from "~/main/use-legacy-activity";

const LISTENER_CONTROL_EVENT = "hypr:listener-control";

type ListenerControlAction = "start" | "stop";

type ListenerControlRequest = {
  action: ListenerControlAction;
  requestId: string;
  sessionId: string;
};

export function isMainWebviewWindow() {
  return getCurrentWebviewWindowLabel() === "main";
}

export async function requestMainListenerControl(
  action: ListenerControlAction,
  sessionId: string,
) {
  await emitTo("main", LISTENER_CONTROL_EVENT, {
    action,
    requestId: crypto.randomUUID(),
    sessionId,
  } satisfies ListenerControlRequest);
}

export function MainListenerControlBridge() {
  const legacy = useLegacyDesktopAccess();
  const { isAvailable, available } = legacy;
  const [requests, setRequests] = useState<ListenerControlRequest[]>([]);

  useEffect(() => {
    if (!available) return;
    let active = true;
    let unlisten: (() => void) | undefined;

    void listen<ListenerControlRequest>(LISTENER_CONTROL_EVENT, (event) => {
      if (!active || !isAvailable()) {
        return;
      }

      setRequests((current) => [...current, event.payload]);
    }).then((fn) => {
      if (active) {
        unlisten = fn;
      } else {
        fn();
      }
    });

    return () => {
      active = false;
      unlisten?.();
    };
  }, [isAvailable, available]);

  const handleRequestHandled = useCallback((requestId: string) => {
    setRequests((current) => {
      if (current[0]?.requestId === requestId) {
        return current.slice(1);
      }

      return current.filter((request) => request.requestId !== requestId);
    });
  }, []);

  const request = requests[0];
  if (!request || !isAvailable()) {
    return null;
  }

  return (
    <MainListenerControlRequestRunner
      request={request}
      onHandled={handleRequestHandled}
    />
  );
}

function MainListenerControlRequestRunner({
  onHandled,
  request,
}: {
  onHandled: (requestId: string) => void;
  request: ListenerControlRequest;
}) {
  const legacy = useLegacySessionActivity(request.sessionId);
  const { isAvailable } = legacy;
  const startListening = useStartListening(request.sessionId, isAvailable);
  const stop = () => {
    if (isAvailable()) legacy.listener!.getState().stop();
  };
  const startListeningRef = useRef(startListening);
  startListeningRef.current = startListening;
  const stopRef = useRef(stop);
  stopRef.current = stop;

  // Capture progress must not re-run the request while its start is in flight.
  useEffect(() => {
    let cancelled = false;

    const run = async () => {
      if (!isAvailable()) {
        if (!cancelled) onHandled(request.requestId);
        return;
      }
      const live = legacy.listener!.getState().live;
      const isStartingOrActive = live.loading || live.status === "active";

      if (request.action === "start") {
        // Non-main windows cannot see this window's state, so they can send
        // duplicate start requests for a session that is already starting.
        if (live.sessionId !== request.sessionId || !isStartingOrActive) {
          await startListeningRef.current();
        }
      } else if (live.sessionId === request.sessionId) {
        stopRef.current();
      }

      if (!cancelled) {
        onHandled(request.requestId);
      }
    };

    void run();

    return () => {
      cancelled = true;
    };
  }, [
    onHandled,
    request.action,
    request.requestId,
    request.sessionId,
    isAvailable,
    legacy.listener,
  ]);

  return null;
}

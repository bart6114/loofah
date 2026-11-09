import { useCallback } from "react";

import { useLegacyDeleteSession } from "./useLegacyDeleteSession";
import { useScopedDeleteSession } from "./useScopedDeleteSession";

import { useLegacyDesktopAccess } from "~/main/use-legacy-activity";
import { sessionDeletionAvailable } from "~/shared/application/session-lifecycle";
import { useTabsStore } from "~/store/zustand/tabs";

export { useLegacySessionDeletionUndoListener as useRemoteSessionDeletionUndoListener } from "./useLegacyDeleteSession";

export function useSessionDeletionAccess() {
  const { scope, isAvailable: legacyAvailable } = useLegacyDesktopAccess();
  const store = useTabsStore();
  const shared = scope.client.features?.includes("session_lifecycle") ?? false;
  const isAvailable = useCallback(
    () =>
      shared
        ? sessionDeletionAvailable(scope, store)
        : legacyAvailable("legacy_desktop_session_controls"),
    [shared, scope, legacyAvailable, store],
  );
  return { scope, shared, isAvailable, available: isAvailable() };
}
export function useDeleteSession() {
  const legacy = useLegacyDeleteSession();
  const access = useLegacyDesktopAccess();
  const shared = useScopedDeleteSession(access.scope);
  return access.scope.client.features?.includes("session_lifecycle")
    ? shared
    : legacy;
}

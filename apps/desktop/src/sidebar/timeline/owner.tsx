import {
  createContext,
  useContext,
  useEffect,
  useState,
  useSyncExternalStore,
} from "react";

import { TimelineController } from "./controller";

import { useVaultHost } from "~/shared/application/vault-provider";
import { useTabsStore } from "~/store/zustand/tabs";

const TimelineOwnerContext = createContext<TimelineController | null>(null);

export function TimelineOwnerProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const { scope } = useVaultHost();
  const tabs = useTabsStore();
  const [owner, setOwner] = useState<{
    scope: typeof scope;
    tabs: typeof tabs;
    controller: TimelineController;
  } | null>(null);
  useEffect(() => {
    if (!scope) return;
    const controller = new TimelineController(scope, crypto.randomUUID());
    controller.start();
    setOwner({ scope, tabs, controller });
    return () => controller.dispose();
  }, [scope, tabs]);
  if (!scope || owner?.scope !== scope || owner.tabs !== tabs) return null;
  return (
    <TimelineOwnerContext.Provider value={owner.controller}>
      {children}
    </TimelineOwnerContext.Provider>
  );
}

export function useTimelineOwner() {
  const owner = useContext(TimelineOwnerContext);
  if (!owner) throw new Error("Timeline requires an immutable main host");
  return owner;
}

export function useTimelineSnapshot() {
  const owner = useTimelineOwner();
  const state = useSyncExternalStore(
    owner.subscribe,
    owner.getSnapshot,
    owner.getSnapshot,
  );
  const connection = useSyncExternalStore(
    owner.scope.subscribe,
    owner.scope.getSnapshot,
    owner.scope.getSnapshot,
  );
  const available = ![
    "closed",
    "login_required",
    "access_denied",
    "incompatible",
  ].includes(connection.state);
  return { owner, state, available, connection };
}

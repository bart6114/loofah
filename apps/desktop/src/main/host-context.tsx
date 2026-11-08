import { createContext, useContext } from "react";

import type { VaultScope } from "~/shared/application/vault-scope";
import type { ListenerStore } from "~/store/zustand/listener";

export type MainCaptureActivity = {
  scope: VaultScope;
  captureState: (sessionId: string) => string | null | undefined;
  subscribeCapture: (sessionId: string, changed: () => void) => () => void;
};

export const MainHostContext = createContext<{
  scope: VaultScope;
  listenerStore: ListenerStore;
  activity?: MainCaptureActivity;
  takeInitialSession?: () => string | null;
} | null>(null);

export function useMainHost() {
  return useContext(MainHostContext);
}

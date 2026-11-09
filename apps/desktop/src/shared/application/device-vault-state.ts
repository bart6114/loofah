import { applicationResult } from "./errors";

import { commands, events } from "~/types/tauri.gen";

export const deviceVaultState = {
  async recentSessions(scope: string) {
    return applicationResult(await commands.deviceRecentSessions(scope));
  },
  async rememberSession(scope: string, id: string) {
    return applicationResult(await commands.deviceRememberSession(scope, id));
  },
  async pinnedTabs(scope: string) {
    return applicationResult(await commands.devicePinnedTabs(scope));
  },
  async setPinnedTabs(scope: string, value: string) {
    applicationResult(await commands.deviceSetPinnedTabs(scope, value));
  },
  async subscribe(scope: string, onChanged: () => void) {
    return events.deviceVaultStateChanged.listen(({ payload }) => {
      if (payload.scope === scope) onChanged();
    });
  },
};

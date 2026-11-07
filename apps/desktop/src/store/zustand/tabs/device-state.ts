export interface TabsDeviceState {
  recentSessions(scope: string): Promise<string[]>;
  rememberSession(scope: string, id: string): Promise<string[]>;
  pinnedTabs(scope: string): Promise<string | null>;
  setPinnedTabs(scope: string, value: string): Promise<void>;
  subscribe(scope: string, onChanged: () => void): Promise<() => void>;
}

import type { StoreApi } from "zustand";

const MAX_RECENT_SESSIONS = 10;

export type RecentlyOpenedState = {
  recentlyOpenedSessionIds: string[];
};

export type RecentlyOpenedActions = {
  addRecentlyOpened: (sessionId: string) => void;
};

export const createRecentlyOpenedSlice = <T extends RecentlyOpenedState>(
  set: StoreApi<T>["setState"],
  get: StoreApi<T>["getState"],
): RecentlyOpenedState & RecentlyOpenedActions => ({
  recentlyOpenedSessionIds: [],
  addRecentlyOpened: (sessionId: string) => {
    const { recentlyOpenedSessionIds } = get();
    const filtered = recentlyOpenedSessionIds.filter((id) => id !== sessionId);
    const updated = [sessionId, ...filtered].slice(0, MAX_RECENT_SESSIONS);
    set({ recentlyOpenedSessionIds: updated } as Partial<T>);
  },
});

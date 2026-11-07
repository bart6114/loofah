import { type Tab, type TabInput, uniqueIdfromTab } from "./schema";

export type PinnedTab = TabInput & { pinned: true };

export const serializePinnedTabs = (tabs: Tab[]): string => {
  const pinnedTabs = tabs
    .filter((t) => t.pinned)
    .map((tab): PinnedTab => {
      const { active, slotId, pinned, returnToSlotId, returnToTabId, ...rest } =
        tab as Tab & {
          active: boolean;
          slotId: string;
          pinned: boolean;
          returnToSlotId?: string;
          returnToTabId?: string;
        };
      return { ...rest, pinned: true } as PinnedTab;
    });
  return JSON.stringify(pinnedTabs);
};

export const deserializePinnedTabs = (data: string): PinnedTab[] => {
  try {
    const parsed = JSON.parse(data) as unknown[];
    if (!Array.isArray(parsed)) {
      return [];
    }

    return parsed.flatMap((tab) => {
      if (!tab || typeof tab !== "object") {
        return [];
      }

      if ((tab as { type?: string }).type === "ai") {
        return {
          ...tab,
          type: "settings",
          state: { tab: (tab as any).state?.tab ?? "transcription" },
        } as PinnedTab;
      }

      const tabType = (tab as { type: string }).type;
      switch (tabType) {
        case "sessions":
        case "changelog":
        case "settings":
        case "onboarding":
          return [tab as PinnedTab];
        case "empty":
        case "daily":
        case "edit":
        case "extension":
        case "extensions":
        default:
          return [];
      }
    });
  } catch {
    return [];
  }
};

export const getPinnedTabIds = (tabs: Tab[]): string[] => {
  return tabs
    .filter((t) => t.pinned)
    .map(uniqueIdfromTab)
    .sort();
};

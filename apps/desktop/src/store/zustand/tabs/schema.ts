import type {
  ChangelogState,
  EditorView,
  SessionsState,
  TabInput as WindowsTabInput,
} from "@hypr/plugin-windows";

export type { ChangelogState, EditorView, SessionsState };

export type SupportedWindowTabInput = Exclude<
  WindowsTabInput,
  | { type: "templates" }
  | { type: "extension" }
  | { type: "extensions" }
  | { type: "folders" }
  | { type: "contacts" }
  | { type: "humans" }
  | { type: "organizations" }
>;

export type TabInput = SupportedWindowTabInput;

export const isTabInputSupported = (
  tab: WindowsTabInput,
): tab is SupportedWindowTabInput => {
  return (
    tab.type !== "templates" &&
    tab.type !== "extension" &&
    tab.type !== "extensions" &&
    tab.type !== "folders" &&
    tab.type !== "contacts" &&
    tab.type !== "humans" &&
    tab.type !== "organizations"
  );
};

export type SettingsTab =
  | "app"
  | "storage"
  | "notifications"
  | "developers"
  | "permissions"
  | "dictionary"
  | "transcription"
  | "intelligence"
  | "summary-prompt"
  | "todo";

export const normalizeSettingsTab = (
  tab: string | null | undefined,
): SettingsTab => {
  switch (tab) {
    case "app":
    case "storage":
    case "notifications":
    case "developers":
    case "permissions":
    case "dictionary":
    case "transcription":
    case "intelligence":
    case "summary-prompt":
    case "todo":
      return tab;
    case "personalization":
      return "dictionary";
    default:
      return "app";
  }
};

export type SettingsState = {
  tab: SettingsTab | null;
};

export type DailySummaryState = {
  activeTab: "timeline" | "raw" | null;
};

export const isEnhancedView = (
  view: EditorView,
): view is { type: "enhanced"; id: string } => view.type === "enhanced";
export const isRawView = (view: EditorView): view is { type: "raw" } =>
  view.type === "raw";

type BaseTab = {
  active: boolean;
  slotId: string;
  pinned: boolean;
  returnToSlotId?: string;
  returnToTabId?: string;
};

export type Tab =
  | (BaseTab & {
      type: "sessions";
      id: string;
      state: SessionsState;
    })
  | (BaseTab & { type: "empty" })
  | (BaseTab & {
      type: "changelog";
      state: ChangelogState;
    })
  | (BaseTab & { type: "settings"; state: SettingsState })
  | (BaseTab & { type: "onboarding" })
  | (BaseTab & {
      type: "task";
      id: string;
      resources: TaskResource[];
    })
  | (BaseTab & {
      type: "daily_summary";
      id: string;
      state: DailySummaryState;
    });

export type TaskResource =
  | { type: "github_issue"; owner: string; repo: string; number: number }
  | { type: "github_pr"; owner: string; repo: string; number: number };

export const getDefaultState = (tab: TabInput): Tab => {
  const base = { active: false, slotId: "", pinned: false };

  switch (tab.type) {
    case "sessions":
      return {
        ...base,
        type: "sessions",
        id: tab.id,
        state: tab.state ?? { view: null, autoStart: null },
      };
    case "empty":
      return { ...base, type: "empty" };
    case "changelog":
      return {
        ...base,
        type: "changelog",
        state: tab.state,
      };
    case "settings": {
      const subtab = tab.state?.tab as string | null | undefined;
      return {
        ...base,
        type: "settings",
        state: {
          tab: normalizeSettingsTab(subtab),
        },
      };
    }
    case "onboarding":
      return { ...base, type: "onboarding" };
    default:
      const _exhaustive: never = tab;
      return _exhaustive;
  }
};

export const uniqueIdfromTab = (tab: Tab): string => {
  switch (tab.type) {
    case "sessions":
      return `sessions-${tab.id}`;
    case "empty":
      return `empty-${tab.slotId}`;
    case "changelog":
      return "changelog";
    case "settings":
      return `settings`;
    case "onboarding":
      return `onboarding`;
    case "task":
      return `task-${tab.id}`;
    case "daily_summary":
      return `daily_summary-${tab.id}`;
  }
};

export const isSameTab = (a: Tab, b: Tab) => {
  return uniqueIdfromTab(a) === uniqueIdfromTab(b);
};

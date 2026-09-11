import { Trans, useLingui } from "@lingui/react/macro";
import { platform } from "@tauri-apps/plugin-os";
import {
  AudioLinesIcon,
  BellIcon,
  Code2Icon,
  CogIcon,
  LockIcon,
  SparklesIcon,
  type LucideIcon,
} from "lucide-react";
import { useCallback } from "react";

import { cn } from "@hypr/utils";

import { CustomSidebarHeader } from "./custom-sidebar-header";

import { type SettingsTab, useTabs } from "~/store/zustand/tabs";

type SettingsNavItem = { id: SettingsTab; label: string; icon: LucideIcon };

type SettingsNavGroup = { label: string; items: SettingsNavItem[] };

export function SettingsNav() {
  const { t } = useLingui();
  const currentTab = useTabs((state) => state.currentTab);
  const updateSettingsTabState = useTabs(
    (state) => state.updateSettingsTabState,
  );

  const activeTab =
    currentTab?.type === "settings"
      ? currentTab.state.tab === "summary-prompt"
        ? "intelligence"
        : (currentTab.state.tab ?? "app")
      : "app";

  const setActiveTab = useCallback(
    (tab: SettingsTab) => {
      if (currentTab?.type === "settings") {
        updateSettingsTabState(currentTab, { tab });
      }
    },
    [currentTab, updateSettingsTabState],
  );

  const groups: SettingsNavGroup[] = [
    {
      label: t`General`,
      items: [
        { id: "app", label: t`App`, icon: CogIcon },
        { id: "notifications", label: t`Notifications`, icon: BellIcon },
        { id: "developers", label: t`Agents`, icon: Code2Icon },
      ],
    },
    {
      label: "AI",
      items: [
        { id: "transcription", label: t`Transcription`, icon: AudioLinesIcon },
        { id: "intelligence", label: t`Intelligence`, icon: SparklesIcon },
      ],
    },
  ];
  const isMacos = platform() === "macos";
  if (isMacos) {
    groups[0].items.push({
      id: "permissions" as const,
      label: t`Permissions`,
      icon: LockIcon,
    });
  }

  return (
    <div className="flex h-full w-full flex-col overflow-hidden">
      <CustomSidebarHeader title={<Trans>Settings</Trans>} />
      <div className="scrollbar-hide flex-1 overflow-y-auto">
        <div className="flex flex-col gap-4 pb-2">
          {groups.map((group) => (
            <div key={group.label} className="flex flex-col gap-0.5">
              <span className="text-muted-foreground px-3 pb-1 text-[11px] font-medium tracking-wider uppercase">
                {group.label}
              </span>
              {group.items.map((item) => {
                return (
                  <button
                    key={item.id}
                    onClick={() => {
                      setActiveTab(item.id as SettingsTab);
                    }}
                    className={cn([
                      "flex w-full items-center gap-2 rounded-full px-3 py-2 text-left text-sm",
                      "transition-colors",
                      activeTab === item.id
                        ? "bg-sidebar-accent text-foreground font-medium"
                        : "text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground",
                    ])}
                  >
                    <item.icon size={15} className="shrink-0" />
                    <span className="min-w-0 flex-1 truncate">
                      {item.label}
                    </span>
                  </button>
                );
              })}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

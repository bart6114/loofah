import { type ReactNode } from "react";

import { cn } from "@hypr/utils";

import { SettingsNav } from "./settings";
import { TimelineView } from "./timeline";

import { useTabs } from "~/store/zustand/tabs";

export function LeftSidebar({
  timelineHeader,
}: {
  timelineHeader?: ReactNode;
} = {}) {
  const currentTab = useTabs((state) => state.currentTab);

  const isSettingsMode = currentTab?.type === "settings";
  const isSpecialMode = isSettingsMode;
  const isTimelineSidebarLayout = !isSpecialMode;

  return (
    <div
      className={cn([
        "flex h-full w-full shrink-0 flex-col gap-1 overflow-hidden",
        isTimelineSidebarLayout ? "pt-0" : "pt-11",
        !isTimelineSidebarLayout && "pr-1",
      ])}
    >
      <div className="flex flex-1 flex-col gap-1 overflow-hidden">
        {isTimelineSidebarLayout ? timelineHeader : null}
        <div className="relative min-h-0 flex-1 overflow-hidden">
          {isSettingsMode ? (
            <SettingsNav />
          ) : (
            <div className="flex h-full min-h-0 flex-col">
              <div className="relative min-h-0 flex-1">
                <TimelineView
                  topChromeInset={isTimelineSidebarLayout && !timelineHeader}
                  topChipsOverlapHeader={
                    isTimelineSidebarLayout && !!timelineHeader
                  }
                />
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

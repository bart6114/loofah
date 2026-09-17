import { cn } from "@hypr/utils";

import {
  SettingsApp,
  SettingsNotifications,
  SettingsPermissions,
  SettingsStorage,
} from "./general";
import { SettingsSync } from "./sync";

import { SettingsHydrationBoundary } from "~/settings/hydration-boundary";
import {
  deferredView,
  DeferredView,
  usePreloadViews,
} from "~/shared/deferred-view";
import { StandardContentWrapper } from "~/shared/main";
import { type Tab } from "~/store/zustand/tabs";

const llm = deferredView(() =>
  import("./ai/llm").then((m) => ({ default: m.LLM })),
);
const prompt = deferredView(() =>
  import("./ai/llm/summary-prompt").then((m) => ({
    default: m.SummaryPromptSettings,
  })),
);
const stt = deferredView(() =>
  import("./ai/stt").then((m) => ({ default: m.STT })),
);
const developers = deferredView(() =>
  import("./developers").then((m) => ({ default: m.SettingsDevelopers })),
);
const preloaders = [stt.preload, llm.preload, developers.preload];

export function TabContentSettings({
  tab,
}: {
  tab: Extract<Tab, { type: "settings" }>;
}) {
  return (
    <StandardContentWrapper>
      <SettingsHydrationBoundary>
        <SettingsView tab={tab} />
      </SettingsHydrationBoundary>
    </StandardContentWrapper>
  );
}

function SettingsView({ tab }: { tab: Extract<Tab, { type: "settings" }> }) {
  usePreloadViews(preloaders);
  const requestedTab = tab.state.tab as string | undefined;
  const activeTab =
    requestedTab === "data" ? "storage" : (tab.state.tab ?? "app");

  const renderContent = () => {
    switch (activeTab) {
      case "app":
        return <SettingsApp />;
      case "sync":
        return <SettingsSync />;
      case "storage":
        return <SettingsStorage />;
      case "notifications":
        return <SettingsNotifications />;
      case "permissions":
        return <SettingsPermissions />;
      case "developers":
        return <developers.View />;
      case "transcription":
        return <stt.View />;
      case "summary-prompt":
        return <prompt.View initiallyOpen />;
      case "intelligence":
        return <llm.View />;
      default:
        return <SettingsApp />;
    }
  };

  return (
    <div
      data-settings-content
      className="bg-card dark:bg-accent flex w-full flex-1 flex-col overflow-hidden"
    >
      <div className="relative w-full flex-1 overflow-hidden">
        <div
          className={cn([
            "scroll-fade-y scrollbar-hide h-full w-full flex-1 overflow-y-auto p-6",
          ])}
        >
          <DeferredView viewKey={activeTab}>{renderContent()}</DeferredView>
        </div>
      </div>
    </div>
  );
}

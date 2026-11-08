import { Trans } from "@lingui/react/macro";

import { cn } from "@hypr/utils";

import {
  SettingsApp,
  SettingsNotifications,
  SettingsPermissions,
  SettingsStorage,
} from "./general";

import { useLegacyDesktopAccess } from "~/main/use-legacy-activity";
import { SettingsHydrationBoundary } from "~/settings/hydration-boundary";
import { VaultProcessingSettings } from "~/shared/application/vault-processing-settings";
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
const EMPTY_PRELOADERS: (() => Promise<unknown>)[] = [];
const preloaders = [stt.preload, llm.preload, developers.preload];

export function TabContentSettings({
  tab,
}: {
  tab: Extract<Tab, { type: "settings" }>;
}) {
  const access = useLegacyDesktopAccess();
  const requested = tab.state.tab as string | undefined;
  const active = requested === "data" ? "storage" : (requested ?? "app");
  if (active === "permissions")
    return (
      <StandardContentWrapper>
        <SettingsView tab={tab} preloadLegacy={false} />
      </StandardContentWrapper>
    );
  const feature =
    active === "intelligence" || active === "summary-prompt"
      ? "legacy_desktop_enhance"
      : active === "notifications" ||
          active === "permissions" ||
          active === "transcription"
        ? "legacy_desktop_transcript_controls"
        : "legacy_desktop_session_controls";
  if (active !== "app" && !access.isAvailable(feature))
    return (
      <StandardContentWrapper>
        <div
          className="flex flex-col items-start gap-4 p-6"
          data-settings-content
        >
          <p role="status">
            <Trans>This operation is unavailable for this connection.</Trans>
          </p>
          <VaultProcessingSettings scope={access.scope} />
        </div>
      </StandardContentWrapper>
    );
  return (
    <StandardContentWrapper>
      <SettingsHydrationBoundary>
        <SettingsView
          tab={tab}
          preloadLegacy={
            access.isAvailable("legacy_desktop_enhance") && access.isAvailable()
          }
        />
      </SettingsHydrationBoundary>
    </StandardContentWrapper>
  );
}

function SettingsView({
  tab,
  preloadLegacy,
}: {
  tab: Extract<Tab, { type: "settings" }>;
  preloadLegacy: boolean;
}) {
  usePreloadViews(preloadLegacy ? preloaders : EMPTY_PRELOADERS);
  const requestedTab = tab.state.tab as string | undefined;
  const activeTab =
    requestedTab === "data" ? "storage" : (tab.state.tab ?? "app");

  const renderContent = () => {
    switch (activeTab) {
      case "app":
        return <SettingsApp />;
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
      className="bg-card flex w-full flex-1 flex-col overflow-hidden"
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

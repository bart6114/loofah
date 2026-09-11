import { TabContentChangelog } from "~/changelog";
import { TabContentOnboarding } from "~/onboarding";
import { TabContentNote } from "~/session";
import { TabContentSettings } from "~/settings";
import { type Tab } from "~/store/zustand/tabs";
import { TabContentTask } from "~/task";

export function MainTabContent({ tab }: { tab: Tab }) {
  if (tab.type === "sessions") {
    return <TabContentNote tab={tab} />;
  }
  if (tab.type === "changelog") {
    return <TabContentChangelog tab={tab} />;
  }
  if (tab.type === "settings") {
    return <TabContentSettings tab={tab} />;
  }
  if (tab.type === "onboarding") {
    return <TabContentOnboarding tab={tab} />;
  }
  if (tab.type === "task") {
    return <TabContentTask tab={tab} />;
  }
  return null;
}

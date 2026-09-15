import { TabContentNote } from "~/session";
import {
  deferredView,
  DeferredView,
  usePreloadViews,
} from "~/shared/deferred-view";
import { type Tab } from "~/store/zustand/tabs";

const changelog = deferredView(() =>
  import("~/changelog").then((m) => ({ default: m.TabContentChangelog })),
);
const onboarding = deferredView(() =>
  import("~/onboarding").then((m) => ({ default: m.TabContentOnboarding })),
);
const settings = deferredView(() =>
  import("~/settings").then((m) => ({ default: m.TabContentSettings })),
);
const task = deferredView(() =>
  import("~/task").then((m) => ({ default: m.TabContentTask })),
);
const preloaders = [settings.preload, task.preload, changelog.preload];

export function MainTabContent({ tab }: { tab: Tab }) {
  usePreloadViews(preloaders);
  return (
    <DeferredView viewKey={tab.type}>
      <Content tab={tab} />
    </DeferredView>
  );
}

function Content({ tab }: { tab: Tab }) {
  if (tab.type === "sessions") {
    return <TabContentNote tab={tab} />;
  }
  if (tab.type === "changelog") {
    return <changelog.View tab={tab} />;
  }
  if (tab.type === "settings") {
    return <settings.View tab={tab} />;
  }
  if (tab.type === "onboarding") {
    return <onboarding.View tab={tab} />;
  }
  if (tab.type === "task") {
    return <task.View tab={tab} />;
  }
  return null;
}

import { memo } from "react";

import { AboutDialog } from "./about-dialog";
import { AudioImportDialog } from "./audio-import-dialog";
import { ClassicMainBody } from "./body";
import { resolveMainSurfaceChrome } from "./main-surface-chrome";

import { useShell } from "~/contexts/shell";
import { MainShellBodyFrame, MainShellScaffold } from "~/shared/main";
import { ToastNotifications } from "~/sidebar/toast";
import {
  hasCustomSidebarTab,
  hasLeftSurfaceCustomSidebarTab,
} from "~/sidebar/use-custom-sidebar";
import { useTabs } from "~/store/zustand/tabs";
import { AudioImportWorker } from "~/stt/audio-import-worker";

export function ClassicMainShellFrame() {
  const { leftsidebar } = useShell();
  const currentTab = useTabs((state) => state.currentTab);

  const isOnboarding = currentTab?.type === "onboarding";
  const isChangelog = currentTab?.type === "changelog";
  const hasCustomSidebar = hasCustomSidebarTab(currentTab);
  const hasLeftSurfaceCustomSidebar =
    hasLeftSurfaceCustomSidebarTab(currentTab);
  const showSidebarTimelineChrome = !hasCustomSidebar && !isOnboarding;
  const showSidebarTimeline = showSidebarTimelineChrome && leftsidebar.expanded;
  const mainSurfaceChrome = resolveMainSurfaceChrome({
    hasLeftSurfaceCustomSidebar,
    isChangelog,
    leftSidebarExpanded: leftsidebar.expanded,
    showSidebarTimeline,
    showSidebarTimelineChrome,
  });

  return (
    <MainShellScaffold
      edgeToEdge={isOnboarding}
      mainSurfaceChrome={isOnboarding ? undefined : mainSurfaceChrome}
    >
      <ClassicMainBodyHost />
      <ToastNotifications />
      <AboutDialog />
      <AudioImportDialog />
      <AudioImportWorker />
    </MainShellScaffold>
  );
}

const ClassicMainBodyHost = memo(function ClassicMainBodyHost() {
  return (
    <MainShellBodyFrame>
      <ClassicMainBody />
    </MainShellBodyFrame>
  );
});

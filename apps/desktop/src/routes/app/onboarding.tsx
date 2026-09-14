import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useCallback } from "react";

import { resolveShellEntryPath } from "./-resolve-entry-path";

import { deferredView, DeferredView } from "~/shared/deferred-view";
const onboarding = deferredView(() =>
  import("~/onboarding").then((module) => ({
    default: module.StandaloneOnboardingScreen,
  })),
);
import { useTabs } from "~/store/zustand/tabs";

export const Route = createFileRoute("/app/onboarding")({
  component: Component,
});

function Component() {
  const navigate = useNavigate();
  const openCurrent = useTabs((state) => state.openCurrent);

  const handleFinish = useCallback(
    (sessionId: string) => {
      openCurrent({ type: "sessions", id: sessionId });
      void (async () => {
        await navigate({ to: await resolveShellEntryPath() });
      })();
    },
    [navigate, openCurrent],
  );

  return (
    <DeferredView viewKey="onboarding">
      <onboarding.View onFinish={handleFinish} />
    </DeferredView>
  );
}

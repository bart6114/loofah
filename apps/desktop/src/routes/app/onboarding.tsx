import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useCallback } from "react";

import { resolveShellEntryPath } from "./-resolve-entry-path";

import { deferredView, DeferredView } from "~/shared/deferred-view";
const onboarding = deferredView(() =>
  import("~/onboarding").then((module) => ({
    default: module.StandaloneOnboardingScreen,
  })),
);
import { setPendingWelcomeSession } from "~/onboarding/welcome-note";

export const Route = createFileRoute("/app/onboarding")({
  component: Component,
});

function Component() {
  const navigate = useNavigate();

  const handleFinish = useCallback(
    (sessionId: string) => {
      setPendingWelcomeSession(sessionId);
      void (async () => {
        await navigate({ to: await resolveShellEntryPath() });
      })();
    },
    [navigate],
  );

  return (
    <DeferredView viewKey="onboarding">
      <onboarding.View onFinish={handleFinish} />
    </DeferredView>
  );
}

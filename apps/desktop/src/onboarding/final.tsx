import { Icon } from "@iconify-icon/react";
import { useLingui } from "@lingui/react";
import { Trans } from "@lingui/react/macro";
import { Loader2Icon } from "lucide-react";
import { useRef, useState } from "react";

import { commands as openerCommands } from "@hypr/plugin-opener2";

import { OnboardingButton } from "./shared";
import {
  getOrCreateWelcomeSession,
  setPendingWelcomeSession,
} from "./welcome-note";

import { createSession } from "~/session/queries";
import { flushAutomaticRelaunch } from "~/shared/relaunch";
import { commands } from "~/types/tauri.gen";

const SOCIALS = [
  {
    label: "GitHub",
    icon: "simple-icons:github",
    url: "https://github.com/bart6114/loofah",
  },
] as const;

const SOCIAL_ICON_SIZE = 18;

export function FinalDescription() {
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
      <span>
        <Trans>Join our community and stay updated:</Trans>
      </span>
      <div className="flex items-center gap-2">
        {SOCIALS.map((social) => (
          <button
            key={social.label}
            onClick={() => void openerCommands.openUrl(social.url, null)}
            className="text-muted-foreground hover:text-muted-foreground inline-flex size-5 items-center justify-center rounded-md transition-none duration-150"
            aria-label={social.label}
          >
            <Icon
              icon={social.icon}
              width={SOCIAL_ICON_SIZE}
              height={SOCIAL_ICON_SIZE}
            />
          </button>
        ))}
      </div>
    </div>
  );
}

export function FinalSection({
  onContinue,
}: {
  onContinue: (sessionId: string) => void;
}) {
  const { i18n } = useLingui();
  const translate = i18n._.bind(i18n);
  const [status, setStatus] = useState<"idle" | "loading" | "error">("idle");
  const finishPromiseRef = useRef<Promise<void> | null>(null);
  const welcomeSessionRef = useRef<string | null>(null);

  const handleContinue = async () => {
    if (finishPromiseRef.current) return;

    setStatus("loading");
    const finishPromise = finishOnboarding(onContinue, welcomeSessionRef);
    finishPromiseRef.current = finishPromise;
    try {
      await finishPromise;
    } catch (error) {
      console.error("Failed to finish onboarding", error);
      setStatus("error");
    } finally {
      finishPromiseRef.current = null;
    }
  };

  return (
    <div className="flex flex-col items-start gap-2">
      <OnboardingButton
        className="px-6 py-2 text-sm disabled:cursor-wait disabled:opacity-70"
        disabled={status === "loading"}
        onClick={() => void handleContinue()}
      >
        {status === "loading" ? (
          <span className="flex items-center gap-2">
            <Loader2Icon className="size-4 animate-spin" />
            <Trans>Open Loofah</Trans>
          </span>
        ) : (
          <Trans>Open Loofah</Trans>
        )}
      </OnboardingButton>
      {status === "error" && (
        <p className="text-destructive text-sm" role="alert">
          {translate({
            id: "onboarding.finish-error",
            message: "Couldn't open Loofah. Please try again.",
          })}
        </p>
      )}
    </div>
  );
}

export async function finishOnboarding(
  onContinue?: (sessionId: string) => void,
  welcomeSessionRef?: { current: string | null },
) {
  const welcomeSessionId =
    welcomeSessionRef?.current ??
    (await getOrCreateWelcomeSession().catch((error) => {
      console.error("Failed to create welcome note", error);
      return createSession();
    }));
  if (welcomeSessionRef) {
    welcomeSessionRef.current = welcomeSessionId;
  }
  await new Promise((resolve) => setTimeout(resolve, 100));
  const result = await commands.setOnboardingNeeded(false);
  if (result.status === "error") {
    throw new Error(result.error);
  }
  await new Promise((resolve) => setTimeout(resolve, 100));
  setPendingWelcomeSession(welcomeSessionId);
  if (await flushAutomaticRelaunch()) {
    return;
  }
  setPendingWelcomeSession(null);
  onContinue?.(welcomeSessionId);
}

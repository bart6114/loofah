import { Trans } from "@lingui/react/macro";
import { useQueryClient } from "@tanstack/react-query";
import { platform } from "@tauri-apps/plugin-os";
import { useCallback, useState } from "react";

import { cn } from "@hypr/utils";

import {
  getInitialStep,
  getNextStep,
  getPrevStep,
  getStepStatus,
} from "./config";
import { FinalDescription, FinalSection, finishOnboarding } from "./final";
import { FolderLocationSection } from "./folder-location";
import { LlmProviderSection } from "./llm-provider";
import { PermissionsSection } from "./permissions";
import { OnboardingSection } from "./shared";
import { SttModelSection } from "./stt-model";

import { StandaloneWindowShell } from "~/shared/window-shell";
import { type Tab, useTabs } from "~/store/zustand/tabs";

export function TabContentOnboarding({
  tab: _tab,
}: {
  tab: Extract<Tab, { type: "onboarding" }>;
}) {
  const openCurrent = useTabs((state) => state.openCurrent);

  const handleFinish = useCallback(
    (sessionId: string) => {
      openCurrent({ type: "sessions", id: sessionId });
    },
    [openCurrent],
  );

  return <OnboardingScreen onFinish={handleFinish} />;
}

function OnboardingScreen({
  onFinish,
}: {
  onFinish: (sessionId: string) => void;
}) {
  return (
    <OnboardingScreenContent
      onFinish={onFinish}
      headerClassName="px-12 pt-4 pb-8"
      headerDragRegion
    />
  );
}

export function StandaloneOnboardingScreen({
  onFinish,
}: {
  onFinish: (sessionId: string) => void;
}) {
  return (
    <StandaloneWindowShell>
      <OnboardingScreenContent
        onFinish={onFinish}
        headerClassName="px-12 pt-4 pb-8"
        headerDragRegion
      />
    </StandaloneWindowShell>
  );
}

function OnboardingScreenContent({
  onFinish,
  headerClassName,
  headerDragRegion = false,
}: {
  onFinish: (sessionId: string) => void;
  headerClassName: string;
  headerDragRegion?: boolean;
}) {
  const queryClient = useQueryClient();
  const [currentStep, setCurrentStep] = useState(getInitialStep);
  const goNext = useCallback(() => {
    const next = getNextStep(currentStep);
    if (next) setCurrentStep(next);
  }, [currentStep]);

  const goBack = useCallback(() => {
    const prev = getPrevStep(currentStep);
    if (prev) setCurrentStep(prev);
  }, [currentStep]);

  const handleFinish = useCallback(
    (sessionId: string) => {
      void queryClient.invalidateQueries({ queryKey: ["onboarding-needed"] });
      onFinish(sessionId);
    },
    [onFinish, queryClient],
  );

  return (
    <div className="bg-card relative flex h-full min-h-0 flex-col overflow-hidden">
      <div
        data-tauri-drag-region={headerDragRegion || undefined}
        className="relative z-30 h-12 shrink-0"
      />

      <div
        data-tauri-drag-region={headerDragRegion || undefined}
        className={cn([
          "relative z-10 flex shrink-0 items-center",
          headerClassName,
        ])}
      >
        <h1 className="text-foreground text-4xl leading-none font-semibold tracking-tight">
          <Trans>Welcome to Loofah</Trans>
        </h1>
      </div>

      <div className="scroll-fade-y relative z-10 flex-1 overflow-y-auto">
        <div className="flex flex-col gap-4 px-12 pb-16">
          <OnboardingSection
            title={<Trans>Start with permissions</Trans>}
            completedTitle={
              platform() === "windows" ? (
                <Trans>Permissions</Trans>
              ) : (
                <Trans>Permissions granted</Trans>
              )
            }
            description={
              platform() === "windows" ? (
                <Trans>
                  Connect a microphone and allow desktop apps to use it in
                  Windows Settings. You can skip this step to use notes and
                  import recordings, then set up recording later.
                </Trans>
              ) : (
                <Trans>
                  Loofah needs access to your microphone and system audio to
                  record and transcribe your meetings
                </Trans>
              )
            }
            status={getStepStatus("permissions", currentStep)}
            skippable={platform() === "windows"}
            onBack={goBack}
            onNext={goNext}
          >
            <PermissionsSection onContinue={goNext} />
          </OnboardingSection>

          <OnboardingSection
            title={<Trans>Storage</Trans>}
            description={
              <Trans>Where your notes and recordings are stored</Trans>
            }
            completedTitle={<Trans>Storage configured</Trans>}
            status={getStepStatus("folder-location", currentStep)}
            onBack={goBack}
            onNext={goNext}
          >
            <FolderLocationSection onContinue={goNext} />
          </OnboardingSection>

          <OnboardingSection
            title={<Trans>Transcription model</Trans>}
            completedTitle={<Trans>Transcription model configured</Trans>}
            description={
              <Trans>
                Loofah transcribes meetings on your device. Pick a model to
                download — you can keep going while it downloads.
              </Trans>
            }
            status={getStepStatus("stt-model", currentStep)}
            onBack={goBack}
            onNext={goNext}
          >
            <SttModelSection onContinue={goNext} />
          </OnboardingSection>

          <OnboardingSection
            title={<Trans>Language model</Trans>}
            completedTitle={<Trans>Language model configured</Trans>}
            description={
              <Trans>
                Summaries and chat need a language model. Use a local server
                like Ollama, or bring an API key from your favorite provider.
              </Trans>
            }
            status={getStepStatus("llm-provider", currentStep)}
            onBack={goBack}
            onNext={goNext}
          >
            <LlmProviderSection onContinue={goNext} />
          </OnboardingSection>

          <OnboardingSection
            title={<Trans>Ready to go</Trans>}
            description={<FinalDescription />}
            status={getStepStatus("final", currentStep)}
            skippable={false}
            onBack={goBack}
            onNext={() => void finishOnboarding(handleFinish)}
          >
            <FinalSection onContinue={handleFinish} />
          </OnboardingSection>
        </div>
      </div>
    </div>
  );
}

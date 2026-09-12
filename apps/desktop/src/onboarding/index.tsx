import { Trans } from "@lingui/react/macro";
import { useQueryClient } from "@tanstack/react-query";
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
import { SummarySetupStatus, TranscriptionSetupStatus } from "./setup-status";
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
            completedTitle={<Trans>Permissions granted</Trans>}
            description={
              <Trans>
                Loofah needs access to your microphone and system audio to
                record and transcribe your meetings
              </Trans>
            }
            status={getStepStatus("permissions", currentStep)}
            skippable={false}
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
            title={<Trans>Set up transcription</Trans>}
            completedTitle={<TranscriptionSetupStatus />}
            showCompletedCheck={false}
            skipLabel={<Trans>Set up transcription later</Trans>}
            description={
              <Trans>
                Download a transcription model to turn recordings into text on
                your Mac. You can keep going while it downloads.
              </Trans>
            }
            status={getStepStatus("stt-model", currentStep)}
            onBack={goBack}
            onNext={goNext}
          >
            <SttModelSection onContinue={goNext} />
          </OnboardingSection>

          <OnboardingSection
            title={<Trans>Summaries</Trans>}
            completedTitle={<SummarySetupStatus />}
            showCompletedCheck={false}
            skipLabel={<Trans>Set up summaries later</Trans>}
            description={
              <Trans>
                Connect a service to create summaries of your meetings. You can
                also set this up later.
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

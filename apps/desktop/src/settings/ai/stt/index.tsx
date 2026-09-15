import { Trans } from "@lingui/react/macro";

import { SttSettingsProvider } from "./context";
import { SelectProviderAndModel } from "./select";
import { TranscriptionTiming } from "./timing";

import { MeetingLanguageSettings } from "~/settings/general/language-settings";
import { SettingsPageTitle } from "~/settings/page-title";

// STT is on-device only (single "fmtr" provider, no API keys/base URLs to
// configure), so there is no non-hypr provider accordion here — see
// `settings/ai/llm/configure.tsx` for the LLM equivalent.
export function STT() {
  return (
    <SttSettingsProvider>
      <div className="flex flex-col gap-6">
        <SettingsPageTitle title={<Trans>Transcription</Trans>} />
        <SelectProviderAndModel />
        <MeetingLanguageSettings />
        <TranscriptionTiming />
      </div>
    </SttSettingsProvider>
  );
}

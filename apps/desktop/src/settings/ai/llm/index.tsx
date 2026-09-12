import { Trans } from "@lingui/react/macro";

import { ConfigureProviders } from "./configure";
import { LlmSettingsProvider } from "./context";
import { SelectProviderAndModel } from "./select";
import { SummaryPromptSettings } from "./summary-prompt";

import { SummaryLanguageSettings } from "~/settings/general/language-settings";
import { SettingsPageTitle } from "~/settings/page-title";

export function LLM() {
  return (
    <LlmSettingsProvider>
      <div className="flex flex-col gap-6">
        <SettingsPageTitle title={<Trans>Summaries</Trans>} />
        <SelectProviderAndModel />
        <ConfigureProviders />
        <SummaryLanguageSettings />
        <SummaryPromptSettings />
      </div>
    </LlmSettingsProvider>
  );
}

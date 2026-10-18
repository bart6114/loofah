import { Trans } from "@lingui/react/macro";

import { ChangeLocationRow } from "./change-location";
import { CompanyStorageSettings } from "./company";
import { RebuildIndexRow } from "./rebuild-index";

import { useCompanyStatus } from "~/shared/company-vault";

export function StorageSettingsView() {
  const { data: company } = useCompanyStatus();
  return (
    <div>
      <h2 className="mb-4 font-sans text-lg font-semibold">
        <Trans>Storage</Trans>
      </h2>
      <p className="text-muted-foreground mb-3 text-xs">
        {company?.active ? (
          <Trans>
            Your notes, recordings, and attachments are stored on your company
            server.
          </Trans>
        ) : (
          <Trans>
            Your notes, recordings, and attachments are saved as files in your
            local folder.
          </Trans>
        )}
      </p>
      <div className="flex flex-col gap-3">
        <CompanyStorageSettings />
        {!company?.active && <ChangeLocationRow />}
        {!company?.active && (
          <details className="rounded-lg border px-4 py-3">
            <summary className="cursor-pointer text-sm font-medium">
              <Trans>Troubleshooting</Trans>
            </summary>
            <div className="mt-3">
              <RebuildIndexRow />
            </div>
          </details>
        )}
      </div>
    </div>
  );
}

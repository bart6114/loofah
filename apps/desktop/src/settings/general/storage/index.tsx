import { Trans } from "@lingui/react/macro";

import { ChangeLocationRow } from "./change-location";
import { RebuildIndexRow } from "./rebuild-index";

export function StorageSettingsView() {
  return (
    <div>
      <h2 className="mb-4 font-sans text-lg font-semibold">
        <Trans>Notes &amp; recordings folder</Trans>
      </h2>
      <p className="text-muted-foreground mb-3 text-xs">
        <Trans>
          Your notes, recordings, and attachments are saved as files in this
          folder.
        </Trans>
      </p>
      <div className="flex flex-col gap-3">
        <ChangeLocationRow />
        <details className="rounded-lg border px-4 py-3">
          <summary className="cursor-pointer text-sm font-medium">
            <Trans>Troubleshooting</Trans>
          </summary>
          <div className="mt-3">
            <RebuildIndexRow />
          </div>
        </details>
      </div>
    </div>
  );
}
